//! `bob albums tidy`: move each album's H1 title into a bob-owned
//! frontmatter `title` key, then drop the now-redundant H1.
//!
//! [`plan_album`] is the pure per-album pass. The command wraps it in the
//! dry-run-first maintenance lifecycle `bob records tidy` uses: a bare run
//! only prints diffs; `-a` takes the shared lock, replans, guards each album
//! by its full preimage, verifies the written bytes, and makes one commit.

use std::{
    cell::RefCell,
    ffi::OsString,
    fs, io,
    path::{Path, PathBuf},
};

use clap::{
    builder::OsStringValueParser, Arg, ArgAction, ArgMatches,
    Command as ClapCommand,
};
use serde_json::{json, Value};
use serde_yaml::{Mapping, Value as YamlValue};
use similar::TextDiff;

use super::{
    parse_album_declaration,
    registry::{AlbumEntry, AlbumRegistry, DiscoveryHealth},
    AlbumDeclaration, FrontmatterTitle,
};
use crate::native::{
    env as bob_env,
    host_catalog::{
        self, CommitInfo, FileEdit, PlannedTxn, ReadDependencies, TxnResult,
    },
    markdown::{atx_heading, closes_fence, fence_marker},
    notebooks::snapshot::VaultSnapshot,
    ob,
    records::{
        notes_section::strip_redundant_h1,
        tidy::{color_diff, KeptH1},
    },
    style::Styler,
    vault_rewrite::TxnFailure,
    yaml_spans::{
        collect_key_spans, join_lines, split_lines, yaml_string_scalar,
        yaml_text, KeySpan, Line,
    },
};

const SCHEMA_VERSION: u32 = 1;
const COMMAND_NAME: &str = "bob albums tidy";

/// Kept reason for an album whose only level-1 heading sits in a fence.
const FENCED_H1: &str = "the H1 is inside fenced code";

// ---------------------------------------------------------------------------
// Pure per-album pass
// ---------------------------------------------------------------------------

/// One album's tidy outcome. `after` is `None` when nothing changes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AlbumPass {
    /// The rewritten album, when the pass changes it.
    pub(crate) after: Option<String>,
    /// The title every reader shows, before and after the pass.
    pub(crate) title: String,
    pub(crate) title_added: bool,
    pub(crate) h1_removed: bool,
    /// Why an H1 stays in place, when one does.
    pub(crate) kept: Option<&'static str>,
    /// Why the album is left unchanged for a person to fix.
    pub(crate) refusal: Option<String>,
}

/// Plan one album: add `title` when the key is absent (with the title every
/// reader already shows), then remove the first body H1 when it is the first
/// content outside fenced code and equals that title exactly. An unusable
/// `title` or damaged frontmatter refuses the album unchanged.
pub(crate) fn plan_album(
    text: &str,
    declaration: &AlbumDeclaration,
) -> AlbumPass {
    let title = declaration.title.clone();
    let mut pass = AlbumPass {
        after: None,
        title: title.clone(),
        title_added: false,
        h1_removed: false,
        kept: None,
        refusal: None,
    };
    let mut current = text.to_owned();
    match &declaration.title_key {
        FrontmatterTitle::Invalid => {
            pass.refusal = Some(
                "frontmatter title is not one nonblank line; fix it by hand"
                    .to_owned(),
            );
            return pass;
        }
        FrontmatterTitle::Valid(_) => {}
        FrontmatterTitle::Absent => match insert_title(&current, &title) {
            Ok(inserted) => {
                current = inserted;
                pass.title_added = true;
            }
            Err(reason) => {
                pass.refusal = Some(reason);
                return pass;
            }
        },
    }
    let strip = strip_redundant_h1(&current, &title);
    if let Some(after) = strip.after {
        current = after;
        pass.h1_removed = true;
    }
    pass.kept = strip.kept;
    if !pass.h1_removed && pass.kept.is_none() && has_fenced_h1(&current) {
        pass.kept = Some(FENCED_H1);
    }
    if pass.title_added || pass.h1_removed {
        pass.after = Some(current);
    }
    pass
}

/// Insert `title: <scalar>` directly after the top-level `type` span, else
/// after `parent`, else right after the opening `---`. The new line takes
/// the line ending of the line before it; a BOM stays on line 0.
fn insert_title(text: &str, title: &str) -> Result<String, String> {
    if title.is_empty() || title != title.trim() || title.contains(['\n', '\r'])
    {
        return Err(
            "its title cannot be stored as one nonblank line; add `title` by hand"
                .to_owned(),
        );
    }
    let mut lines = split_lines(text);
    let opens = lines
        .first()
        .is_some_and(|line| line.text.trim_start_matches('\u{feff}') == "---");
    if !opens {
        return Err(
            "the frontmatter has no opening `---`; fix it by hand".to_owned()
        );
    }
    let Some(close) = lines
        .iter()
        .enumerate()
        .skip(1)
        .find_map(|(index, line)| (line.text == "---").then_some(index))
    else {
        return Err(
            "the frontmatter has no closing `---`; fix it by hand".to_owned()
        );
    };
    let spans = collect_key_spans(&lines[1..close], 1).map_err(|error| {
        format!("the frontmatter is damaged ({error}); fix it by hand")
    })?;
    if spans.iter().any(|span| span.name == "title") {
        return Err(
            "the frontmatter has a `title` key bob cannot read; fix it by hand"
                .to_owned(),
        );
    }
    let after_span = |name: &str| {
        spans
            .iter()
            .find(|span| span.name == name)
            .map(|span| settled_end(&lines, span) + 1)
    };
    let at = after_span("type")
        .or_else(|| after_span("parent"))
        .unwrap_or(1);
    let before_yaml = frontmatter_mapping(&lines[1..close]);
    let ending = lines[at - 1].ending.clone();
    lines.insert(
        at,
        Line {
            text: format!("title: {}", yaml_string_scalar(title)),
            ending,
        },
    );
    // The edit must add exactly one key and leave every other key as it was.
    if let Some(before) = before_yaml {
        let mut after =
            frontmatter_mapping(&lines[1..=close]).ok_or_else(|| {
                "inserting `title` would break the frontmatter; fix it by hand"
                    .to_owned()
            })?;
        let inserted = after.remove(YamlValue::String("title".to_owned()));
        if inserted != Some(YamlValue::String(title.to_owned()))
            || after != before
        {
            return Err(
                "inserting `title` would change other frontmatter; fix it by hand"
                    .to_owned(),
            );
        }
    }
    Ok(join_lines(&lines))
}

/// A key span's last value line, ignoring trailing blank lines and
/// column-0 comments, so the new key sits directly under the value.
fn settled_end(lines: &[Line], span: &KeySpan) -> usize {
    let mut end = span.end;
    while end > span.start
        && (lines[end].text.trim().is_empty()
            || lines[end].text.starts_with('#'))
    {
        end -= 1;
    }
    end
}

fn frontmatter_mapping(lines: &[Line]) -> Option<Mapping> {
    match serde_yaml::from_str::<YamlValue>(&yaml_text(lines)) {
        Ok(YamlValue::Mapping(mapping)) => Some(mapping),
        _ => None,
    }
}

/// True when a level-1 ATX heading sits inside a fenced block of the body.
fn has_fenced_h1(text: &str) -> bool {
    let lines = split_lines(text);
    let opens = lines
        .first()
        .is_some_and(|line| line.text.trim_start_matches('\u{feff}') == "---");
    if !opens {
        return false;
    }
    let Some(close) = lines
        .iter()
        .enumerate()
        .skip(1)
        .find_map(|(index, line)| (line.text == "---").then_some(index))
    else {
        return false;
    };
    let mut fence = None;
    for line in &lines[close + 1..] {
        if let Some(open) = fence {
            if closes_fence(&line.text, open) {
                fence = None;
            } else if atx_heading(&line.text)
                .is_some_and(|(level, _)| level == 1)
            {
                return true;
            }
            continue;
        }
        fence = fence_marker(&line.text);
    }
    false
}

/// Whether `text` is a settled tidy result for `title`: it parses as an
/// album, reads back the same title, and a fresh pass plans no change.
fn settled(relative: &Path, text: &str, title: &str) -> Result<(), String> {
    let Some(declaration) = parse_album_declaration(relative, text) else {
        return Err("it no longer parses as an album".to_owned());
    };
    if declaration.title != title {
        return Err(format!(
            "its title reads as {:?}, not {title:?}",
            declaration.title
        ));
    }
    if plan_album(text, &declaration).after.is_some() {
        return Err("a second pass would still change it".to_owned());
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Vault plan
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct AlbumChange {
    path: String,
    title: String,
    before: String,
    after: String,
    title_added: bool,
    h1_removed: bool,
    h1_kept_reason: Option<String>,
}

impl AlbumChange {
    fn diff(&self) -> String {
        TextDiff::from_lines(&self.before, &self.after)
            .unified_diff()
            .context_radius(3)
            .header(&format!("a/{}", self.path), &format!("b/{}", self.path))
            .to_string()
    }
}

#[derive(Debug, Clone)]
struct TidyPlan {
    scanned: usize,
    changes: Vec<AlbumChange>,
    kept_h1: Vec<KeptH1>,
    edits: Vec<FileEdit>,
    dependencies: ReadDependencies,
    warnings: Vec<String>,
    io_errors: Vec<String>,
}

#[derive(Debug, Clone, Default)]
struct TidyContext {
    scanned: usize,
    changes: Vec<AlbumChange>,
    kept_h1: Vec<KeptH1>,
    warnings: Vec<String>,
    io_errors: Vec<String>,
}

impl TidyPlan {
    fn context(&self) -> TidyContext {
        TidyContext {
            scanned: self.scanned,
            changes: self.changes.clone(),
            kept_h1: self.kept_h1.clone(),
            warnings: self.warnings.clone(),
            io_errors: self.io_errors.clone(),
        }
    }
}

fn slash(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn unknown_album(stem: &str) -> TxnFailure {
    TxnFailure::new(
        "unknown_album",
        format!("unknown album `{stem}`"),
        Some("use an album stem from `bob albums list`"),
    )
}

fn incomplete_scan(errors: &[String]) -> TxnFailure {
    let first = errors
        .first()
        .map(String::as_str)
        .unwrap_or("unknown I/O error");
    let more = errors.len().saturating_sub(1);
    TxnFailure::new(
        "io",
        if more == 0 {
            format!("the vault scan was incomplete: {first}")
        } else {
            format!("the vault scan was incomplete: {first} (+{more} more)")
        },
        Some("fix unreadable vault paths, then retry; nothing was written"),
    )
}

/// The root album `<stem>.md`, matched exactly: no noun fallback, no path.
fn root_album<'a>(
    registry: &'a AlbumRegistry,
    stem: &str,
) -> Option<&'a AlbumEntry> {
    let path = format!("{stem}.md");
    registry
        .albums
        .iter()
        .find(|entry| entry.declaration.path == path)
}

fn plan_tidy(
    vault: &Path,
    registry: &AlbumRegistry,
    snapshot: &VaultSnapshot,
    stem: Option<&str>,
) -> Result<TidyPlan, TxnFailure> {
    if let DiscoveryHealth::Partial(errors) = &registry.health {
        return Err(incomplete_scan(errors));
    }
    let selected: Vec<&AlbumEntry> = match stem {
        Some(stem) => {
            vec![root_album(registry, stem)
                .ok_or_else(|| unknown_album(stem))?]
        }
        None => registry.albums.iter().collect(),
    };

    let mut plan = TidyPlan {
        scanned: 0,
        changes: Vec::new(),
        kept_h1: Vec::new(),
        edits: Vec::new(),
        dependencies: ReadDependencies::default(),
        warnings: Vec::new(),
        io_errors: Vec::new(),
    };
    for entry in selected {
        let relative = PathBuf::from(&entry.declaration.path);
        let path = slash(&relative);
        let contents = match snapshot.contents(&relative) {
            Some(contents) => contents.to_owned(),
            None => match fs::read_to_string(vault.join(&relative)) {
                Ok(contents) => contents,
                Err(error) => {
                    plan.io_errors
                        .push(format!("{path}: failed to read file: {error}"));
                    continue;
                }
            },
        };
        plan.scanned += 1;
        // Re-parse the exact bytes being edited so the declaration and the
        // rewrite can never disagree.
        let Some(declaration) = parse_album_declaration(&relative, &contents)
        else {
            plan.warnings.push(format!(
                "{path}: no longer parses as an album; left unchanged"
            ));
            continue;
        };
        let pass = plan_album(&contents, &declaration);
        if let Some(reason) = &pass.refusal {
            plan.warnings.push(format!("{path}: {reason}"));
            continue;
        }
        if let Some(reason) = pass.kept {
            plan.kept_h1.push(KeptH1 {
                path: path.clone(),
                reason: reason.to_owned(),
            });
        }
        let Some(after) = pass.after else {
            continue;
        };
        // Defensive: a planned write must itself be settled.
        if let Err(reason) = settled(&relative, &after, &pass.title) {
            plan.warnings.push(format!(
                "{path}: the planned rewrite is unsafe ({reason}); left unchanged"
            ));
            continue;
        }
        plan.changes.push(AlbumChange {
            path,
            title: pass.title,
            before: contents.clone(),
            after: after.clone(),
            title_added: pass.title_added,
            h1_removed: pass.h1_removed,
            h1_kept_reason: pass.kept.map(str::to_owned),
        });
        plan.edits.push(FileEdit {
            path: relative.clone(),
            before: contents.clone(),
            after,
        });
        plan.dependencies.insert(relative, Some(contents));
    }
    plan.changes
        .sort_by(|left, right| left.path.cmp(&right.path));
    plan.edits.sort_by(|left, right| left.path.cmp(&right.path));
    plan.kept_h1
        .sort_by(|left, right| left.path.cmp(&right.path));
    Ok(plan)
}

fn planned_txn(vault: &Path, plan: TidyPlan) -> PlannedTxn<TidyContext> {
    let expected: Vec<(PathBuf, String)> = plan
        .changes
        .iter()
        .map(|change| (PathBuf::from(&change.path), change.title.clone()))
        .collect();
    let files = host_catalog::staged_rewrites(vault, &plan.edits);
    let count = files.len();
    let verify_vault = vault.to_path_buf();
    let context = plan.context();
    PlannedTxn {
        files,
        appends: Vec::new(),
        deps: plan.dependencies,
        subject: format!("albums: tidy ({count} notes)"),
        verify: Box::new(move || verify_tidied(&verify_vault, &expected)),
        context,
        kind: "album",
    }
}

/// Re-read every written album: it must parse as an album, keep its
/// planned title, and plan no further change.
fn verify_tidied(
    vault: &Path,
    expected: &[(PathBuf, String)],
) -> Result<(), String> {
    for (relative, title) in expected {
        let contents =
            fs::read_to_string(vault.join(relative)).map_err(|error| {
                format!("could not re-read {}: {error}", slash(relative))
            })?;
        settled(relative, &contents, title).map_err(|reason| {
            format!("{} after the write: {reason}", slash(relative))
        })?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Report
// ---------------------------------------------------------------------------

#[derive(Debug)]
struct TidyReport {
    dry_run: bool,
    context: TidyContext,
    commit: Option<CommitInfo>,
    sync_pre: Option<&'static str>,
    sync_post: Option<&'static str>,
    failure: Option<TxnFailure>,
}

impl TidyReport {
    fn failed(
        dry_run: bool,
        failure: TxnFailure,
        context: Option<TidyContext>,
    ) -> Self {
        Self {
            dry_run,
            context: context.unwrap_or_default(),
            commit: None,
            sync_pre: None,
            sync_post: None,
            failure: Some(failure),
        }
    }

    fn ok(&self) -> bool {
        self.failure.is_none() && self.context.io_errors.is_empty()
    }

    fn exit_code(&self) -> i32 {
        if self
            .failure
            .as_ref()
            .is_some_and(|failure| failure.code == "unknown_album")
        {
            return 2;
        }
        i32::from(!self.ok())
    }

    fn titles_added(&self) -> usize {
        self.context
            .changes
            .iter()
            .filter(|change| change.title_added)
            .count()
    }

    fn h1s_removed(&self) -> usize {
        self.context
            .changes
            .iter()
            .filter(|change| change.h1_removed)
            .count()
    }

    fn to_json(&self) -> Value {
        let context = &self.context;
        let mut payload = json!({
            "schema_version": SCHEMA_VERSION,
            "ok": self.ok(),
            "command": "tidy",
            "dry_run": self.dry_run,
            "counts": {
                "scanned": context.scanned,
                "changed": context.changes.len(),
                "title_added": self.titles_added(),
                "h1_removed": self.h1s_removed(),
                "h1_kept": context.kept_h1.len(),
            },
            "changes": context.changes.iter().map(|change| {
                let mut item = json!({
                    "path": change.path,
                    "title": change.title,
                    "title_added": change.title_added,
                    "h1_removed": change.h1_removed,
                    "h1_kept_reason": change.h1_kept_reason,
                });
                if self.dry_run {
                    item["diff"] = Value::String(change.diff());
                }
                item
            }).collect::<Vec<_>>(),
            "kept_h1": context.kept_h1.iter().map(|kept| {
                json!({"path": kept.path, "reason": kept.reason})
            }).collect::<Vec<_>>(),
            "commit": match &self.commit {
                Some(commit) => json!({"sha": commit.sha, "subject": commit.subject}),
                None => Value::Null,
            },
            "sync": match (self.sync_pre, self.sync_post) {
                (Some(pre), Some(post)) => json!({"pre": pre, "post": post}),
                _ => Value::Null,
            },
            "warnings": context.warnings,
            "io_errors": context.io_errors,
        });
        if let Some(failure) = &self.failure {
            payload["error"] = json!({
                "code": failure.code,
                "message": failure.message,
                "hint": failure.hint,
            });
        }
        payload
    }

    fn print_human(&self) {
        let styler = Styler::detect();
        let context = &self.context;
        if let Some(failure) = &self.failure {
            eprintln!("{} {}", styler.red("✗"), failure.message);
            if let Some(hint) = &failure.hint {
                eprintln!("  {}", styler.dim(hint));
            }
        }
        for error in &context.io_errors {
            eprintln!("{} {error}", styler.red("✗"));
        }
        if self.dry_run {
            for change in &context.changes {
                print!("{}", color_diff(&change.diff(), &styler));
            }
            if !context.changes.is_empty() {
                println!(
                    "{} dry run: {} would change ({}; {}); nothing was written. Rerun with -a to apply",
                    styler.yellow("!"),
                    counted(context.changes.len(), "album", "albums"),
                    counted(self.titles_added(), "title added", "titles added"),
                    counted(self.h1s_removed(), "redundant H1", "redundant H1s"),
                );
            } else if self.ok() {
                self.print_clean(&styler);
            }
        } else if self.failure.is_none() {
            match &self.commit {
                Some(commit) => {
                    println!(
                        "{} tidied {} ({}): {} scanned, {}, {}",
                        styler.green("✓"),
                        counted(context.changes.len(), "album", "albums"),
                        &commit.sha[..commit.sha.len().min(9)],
                        context.scanned,
                        counted(
                            self.titles_added(),
                            "title added",
                            "titles added"
                        ),
                        counted(
                            self.h1s_removed(),
                            "redundant H1",
                            "redundant H1s"
                        ),
                    );
                    for change in &context.changes {
                        println!(
                            "  {} ({})",
                            change.path,
                            change_names(change)
                        );
                    }
                }
                None if context.io_errors.is_empty() => {
                    self.print_clean(&styler);
                }
                None => {}
            }
        }
        self.print_kept(&styler);
        for warning in &context.warnings {
            println!("{} {warning}", styler.yellow("!"));
        }
    }

    fn print_clean(&self, styler: &Styler) {
        println!(
            "{} albums are tidy: {} scanned, 0 tidied",
            styler.green("✓"),
            self.context.scanned
        );
    }

    fn print_kept(&self, styler: &Styler) {
        let kept = &self.context.kept_h1;
        if kept.is_empty() {
            return;
        }
        let header = format!(
            "{} {} H1:",
            counted(kept.len(), "album keeps", "albums keep"),
            if kept.len() == 1 { "its" } else { "their" },
        );
        println!("{}", styler.dim(&header));
        for entry in kept {
            println!("  {} ({})", entry.path, styler.dim(&entry.reason));
        }
    }
}

/// `"1 album"` / `"2 albums"`.
fn counted(count: usize, one: &str, many: &str) -> String {
    format!("{count} {}", if count == 1 { one } else { many })
}

/// `"title added, redundant H1"` naming one changed album's edits.
fn change_names(change: &AlbumChange) -> String {
    let mut kinds = Vec::new();
    if change.title_added {
        kinds.push("title added");
    }
    if change.h1_removed {
        kinds.push("redundant H1");
    }
    kinds.join(", ")
}

// ---------------------------------------------------------------------------
// CLI
// ---------------------------------------------------------------------------

pub(crate) fn build_cli_tidy() -> ClapCommand {
    ClapCommand::new(COMMAND_NAME)
        .about("Move album H1 titles into frontmatter `title` (dry run by default)")
        .long_about(
            "Give each root album a bob-owned frontmatter `title` and drop its redundant H1. An album without `title` gains `title: <its current title>`, inserted after `type`: the first H1 before a bullet or `##` (wikilinks to display text), else the first alias, else the humanized stem. Walk lines, `[!records]` indexes, and `[!albums]` catalogs therefore keep the same text. The first body H1 is then removed when it is the first body content outside fenced code and its text equals that title exactly. A differing H1, an H1 after other content, a later H1, and an H1 inside fenced code are kept and reported. An unusable `title` or damaged frontmatter leaves the album unchanged with a warning. Without `-a` the run is a dry run: it prints unified diffs and takes no lock, writes nothing, runs no Git, and does no sync. `-a` takes the shared vault lock, pre-syncs, replans, guards each album by its full preimage, verifies that each rewritten album keeps its title and plans no further change, makes one scoped `albums: tidy (N notes)` commit, and post-syncs only when it committed. STEM selects one root album by its exact stem. Caution: an older bob reads album titles only from the H1, so its `bob albums sync` would rewrite walk lines and catalog rows to the humanized stem; apply only after every host that runs album sync has this bob.",
        )
        .after_help(
            "Examples:\n  bob albums tidy\n  bob albums tidy eat\n  bob albums tidy -f json\n  bob albums tidy -a\n\nSee docs/records.md for the album title rule.",
        )
        .disable_help_flag(true)
        .arg(
            Arg::new("stem")
                .value_name("STEM")
                .required(false)
                .help("Album stem; selects the root <stem>.md note"),
        )
        .arg(
            Arg::new("apply")
                .long("apply")
                .short('a')
                .action(ArgAction::SetTrue)
                .help("Write the plan and make one verified commit"),
        )
        .arg(
            Arg::new("bob-dir")
                .long("bob-dir")
                .short('b')
                .value_name("DIR")
                .value_parser(OsStringValueParser::new())
                .help("Bob vault root; defaults to BOB_DIR or ~/bob"),
        )
        .arg(
            Arg::new("format")
                .long("format")
                .short('f')
                .value_name("FORMAT")
                .default_value("human")
                .value_parser(["human", "json"])
                .help("Output format: human or json"),
        )
        .arg(
            Arg::new("help")
                .long("help")
                .short('h')
                .action(ArgAction::Help)
                .help("Show help"),
        )
}

pub(crate) fn run_tidy(args: Vec<OsString>) -> i32 {
    let mut command = build_cli_tidy();
    let name = command.get_name().to_owned();
    let matches = match command.try_get_matches_from_mut(
        std::iter::once(OsString::from(name)).chain(args),
    ) {
        Ok(matches) => matches,
        Err(error) => {
            let code = error.exit_code();
            if let Err(print_error) = error.print()
                && print_error.kind() != io::ErrorKind::BrokenPipe
            {
                eprintln!(
                    "{COMMAND_NAME}: failed to print command-line error: {print_error}"
                );
            }
            return code;
        }
    };
    execute(&matches)
}

fn execute(matches: &ArgMatches) -> i32 {
    let json_mode = matches
        .get_one::<String>("format")
        .is_some_and(|format| format == "json");
    let vault = matches
        .get_one::<OsString>("bob-dir")
        .map(PathBuf::from)
        .map(|path| bob_env::expand_tilde(&path))
        .unwrap_or_else(bob_env::bob_dir);
    let stem = matches.get_one::<String>("stem").map(String::as_str);
    let report = if matches.get_flag("apply") {
        applied_report(&vault, stem, json_mode)
    } else {
        dry_run_report(&vault, stem)
    };
    if json_mode {
        println!("{}", report.to_json());
    } else {
        report.print_human();
    }
    report.exit_code()
}

/// Read-only dry run: no lock, no Git, no vault-sync.
fn dry_run_report(vault: &Path, stem: Option<&str>) -> TidyReport {
    let snapshot = VaultSnapshot::load(vault);
    let registry = AlbumRegistry::from_snapshot(vault, &snapshot);
    match plan_tidy(vault, &registry, &snapshot, stem) {
        Ok(plan) => TidyReport {
            dry_run: true,
            context: plan.context(),
            commit: None,
            sync_pre: None,
            sync_post: None,
            failure: None,
        },
        Err(failure) => TidyReport::failed(true, failure, None),
    }
}

/// `-a`: the shared lock, pre-sync, a fresh plan, one guarded transaction,
/// and a post-sync only when it committed.
fn applied_report(
    vault: &Path,
    stem: Option<&str>,
    json_mode: bool,
) -> TidyReport {
    // Usage errors precede the lock lifecycle so an unknown STEM exits 2
    // even when vault-sync would fail.
    if let Some(stem) = stem {
        let snapshot = VaultSnapshot::load(vault);
        let registry = AlbumRegistry::from_snapshot(vault, &snapshot);
        if !matches!(registry.health, DiscoveryHealth::Partial(_))
            && root_album(&registry, stem).is_none()
        {
            return TidyReport::failed(false, unknown_album(stem), None);
        }
    }
    let child_env = ob::child_env();
    let captured: RefCell<Option<TidyContext>> = RefCell::new(None);
    let result =
        host_catalog::run_locked(vault, &child_env, json_mode, "album", || {
            let snapshot = VaultSnapshot::load(vault);
            let registry = AlbumRegistry::from_snapshot(vault, &snapshot);
            let plan = plan_tidy(vault, &registry, &snapshot, stem)?;
            *captured.borrow_mut() = Some(plan.context());
            Ok(planned_txn(vault, plan))
        });
    match result {
        Ok(result) => applied_of(result),
        Err(failure) => {
            TidyReport::failed(false, failure, captured.into_inner())
        }
    }
}

fn applied_of(result: TxnResult<TidyContext>) -> TidyReport {
    let mut context = result.context;
    if let Some(warning) = result.post_sync_warning {
        context.warnings.push(warning);
    }
    TidyReport {
        dry_run: false,
        context,
        commit: result.commit,
        sync_pre: Some(result.sync_pre),
        sync_post: Some(result.sync_post),
        failure: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn declaration(text: &str) -> AlbumDeclaration {
        parse_album_declaration(Path::new("eat.md"), text)
            .expect("album declaration")
    }

    fn pass(text: &str) -> AlbumPass {
        plan_album(text, &declaration(text))
    }

    /// A fresh pass over a planned rewrite changes nothing.
    fn assert_settled(text: &str) {
        let again = pass(text);
        assert_eq!(again.after, None, "second pass changes:\n{text}");
        assert_eq!(again.refusal, None);
    }

    const HEAD: &str = "---\nparent: \"[[body]]\"\ntype: \"[[album]]\"\nsummary: Places.\nrecord:\n  noun: restaurant\n---\n";

    #[test]
    fn absent_title_with_matching_h1_adds_title_and_removes_h1() {
        let text = format!("{HEAD}\n# Restaurants\n\nTrack restaurants.\n");
        let result = pass(&text);
        assert_eq!(result.title, "Restaurants");
        assert!(result.title_added);
        assert!(result.h1_removed);
        assert_eq!(result.kept, None);
        let after = result.after.expect("changes");
        assert_eq!(
            after,
            "---\nparent: \"[[body]]\"\ntype: \"[[album]]\"\ntitle: Restaurants\nsummary: Places.\nrecord:\n  noun: restaurant\n---\n\nTrack restaurants.\n"
        );
        assert_settled(&after);
    }

    #[test]
    fn created_shape_album_ends_at_frontmatter_then_intro() {
        let text = format!(
            "{HEAD}\n# Eat\n\nTrack restaurants with `Name @eat/`.\n\n## Records\n"
        );
        let after = pass(&text).after.expect("changes");
        assert!(after.contains("---\n\nTrack restaurants with"), "{after}");
        assert!(!after.contains("# Eat"));
    }

    #[test]
    fn present_title_removes_an_equal_h1_and_keeps_a_differing_one() {
        let equal = "---\ntype: \"[[album]]\"\ntitle: Eat\nrecord: {noun: restaurant}\n---\n\n# Eat\n\n- a\n";
        let result = pass(equal);
        assert!(!result.title_added);
        assert!(result.h1_removed);
        assert_eq!(
            result.after.as_deref(),
            Some("---\ntype: \"[[album]]\"\ntitle: Eat\nrecord: {noun: restaurant}\n---\n\n- a\n")
        );

        let differing = "---\ntype: \"[[album]]\"\ntitle: Eat\nrecord: {noun: restaurant}\n---\n\n# Restaurants\n";
        let result = pass(differing);
        assert_eq!(result.after, None);
        assert_eq!(
            result.kept,
            Some("the H1 differs from the frontmatter title")
        );
    }

    #[test]
    fn wikilinked_h1_is_kept_as_differing_from_its_cleaned_title() {
        let text = format!("{HEAD}\n# [[eat|Restaurants]]\n");
        let result = pass(&text);
        assert_eq!(result.title, "Restaurants");
        assert!(result.title_added);
        assert!(!result.h1_removed);
        assert_eq!(
            result.kept,
            Some("the H1 differs from the frontmatter title")
        );
        assert_settled(&result.after.expect("title added"));
    }

    #[test]
    fn h1_after_prose_and_later_h1s_are_kept() {
        let prose = format!("{HEAD}\nIntro.\n\n# Restaurants\n");
        let result = pass(&prose);
        assert_eq!(result.title, "Restaurants");
        assert!(result.title_added);
        assert!(!result.h1_removed);
        assert_eq!(result.kept, Some("the H1 is not the first body content"));

        let later = format!("{HEAD}\n# Restaurants\n\nText.\n\n# Other\n");
        let result = pass(&later);
        assert!(result.title_added);
        assert!(result.h1_removed);
        assert_eq!(result.kept, Some("a later H1 is kept"));
        let after = result.after.expect("changes");
        assert!(after.contains("# Other"));
        assert_settled(&after);
    }

    #[test]
    fn fenced_only_h1_takes_the_alias_or_stem_title() {
        let fenced = "---\ntype: \"[[album]]\"\naliases: [Food]\nrecord: {noun: restaurant}\n---\n\n```md\n# Not A Title\n```\n";
        let result = pass(fenced);
        assert_eq!(result.title, "Food");
        assert!(result.title_added);
        assert!(!result.h1_removed);
        assert_eq!(result.kept, Some(FENCED_H1));
        let after = result.after.expect("title added");
        assert!(after.contains("type: \"[[album]]\"\ntitle: Food\naliases"));
        assert_settled(&after);

        let bare = "---\ntype: \"[[album]]\"\nrecord: {noun: restaurant}\n---\n\nIntro.\n";
        let result = pass(bare);
        assert_eq!(result.title, "eat");
        assert_eq!(result.kept, None);
        assert!(result
            .after
            .expect("title added")
            .contains("type: \"[[album]]\"\ntitle: eat\n"));
    }

    #[test]
    fn invalid_title_and_duplicate_keys_refuse_without_change() {
        let invalid = "---\ntype: \"[[album]]\"\ntitle: 42\nrecord: {noun: restaurant}\n---\n\n# Eat\n";
        let result = pass(invalid);
        assert_eq!(result.after, None);
        assert_eq!(
            result.refusal.as_deref(),
            Some("frontmatter title is not one nonblank line; fix it by hand")
        );

        let duplicate = "---\ntype: \"[[album]]\"\nsummary: One\nsummary: Two\nrecord: {noun: restaurant}\n---\n\n# Eat\n";
        let result = pass(duplicate);
        assert_eq!(result.after, None);
        let refusal = result.refusal.expect("refused");
        assert!(refusal.contains("duplicate frontmatter key"), "{refusal}");

        let quoted_key = "---\ntype: \"[[album]]\"\n\"title\": Eat\nrecord: {noun: restaurant}\n---\n\n# Eat\n";
        let result = pass(quoted_key);
        assert_eq!(result.after, None);
        assert!(result.refusal.expect("refused").contains("`title` key"));
    }

    #[test]
    fn missing_closing_delimiter_refuses() {
        let text = format!("{HEAD}\n# Eat\n");
        let declaration = declaration(&text);
        let broken = "---\ntype: \"[[album]]\"\n\n# Eat\n";
        let result = plan_album(broken, &declaration);
        assert_eq!(result.after, None);
        assert!(result.refusal.expect("refused").contains("closing `---`"));
    }

    #[test]
    fn crlf_and_bom_bytes_survive() {
        let crlf = "---\r\ntype: \"[[album]]\"\r\nrecord: {noun: restaurant}\r\n---\r\n\r\n# Eat Here\r\n\r\n- a\r\n";
        let after = pass(crlf).after.expect("changes");
        assert_eq!(
            after,
            "---\r\ntype: \"[[album]]\"\r\ntitle: Eat Here\r\nrecord: {noun: restaurant}\r\n---\r\n\r\n- a\r\n"
        );

        // BOM albums do not register, so plan with the BOM-free declaration.
        let bom = format!("\u{feff}{crlf}");
        let result = plan_album(&bom, &declaration(crlf));
        let after = result.after.expect("changes");
        assert!(after.starts_with("\u{feff}---\r\ntype"));
        assert!(after.contains("title: Eat Here\r\n"));
        assert!(!after.contains("# Eat Here"));
    }

    #[test]
    fn insertion_follows_type_span_then_parent_then_opening() {
        let list = "---\ntype:\n  - \"[[area]]\"\n  - \"[[album]]\"\n\nparent: \"[[body]]\"\nrecord: {noun: restaurant}\n---\n\n# Eat\n";
        let after = pass(list).after.expect("changes");
        assert!(
            after.starts_with(
                "---\ntype:\n  - \"[[area]]\"\n  - \"[[album]]\"\ntitle: Eat\n\nparent"
            ),
            "{after}"
        );

        let parent_after_type = "---\ntype: \"[[album]]\"\nparent: \"[[body]]\"\nrecord: {noun: restaurant}\n---\n\n# Eat\n";
        let after = pass(parent_after_type).after.expect("changes");
        assert!(after.starts_with(
            "---\ntype: \"[[album]]\"\ntitle: Eat\nparent: \"[[body]]\"\n"
        ));

        let declaration = declaration(parent_after_type);
        let no_type = "---\nparent: \"[[body]]\"\nsummary: S\n---\n\n# Eat\n";
        let after = plan_album(no_type, &declaration).after.expect("changes");
        assert!(after.starts_with(
            "---\nparent: \"[[body]]\"\ntitle: Eat\nsummary: S\n"
        ));
        let neither = "---\nsummary: S\n---\n\n# Eat\n";
        let after = plan_album(neither, &declaration).after.expect("changes");
        assert!(after.starts_with("---\ntitle: Eat\nsummary: S\n"));
    }

    #[test]
    fn titles_needing_quotes_are_quoted_and_read_back() {
        for title in ["Eat & Drink", "2026", "yes", "a: b", "Ma \"Pa\" Deli"] {
            let text = format!("{HEAD}\n# {title}\n");
            let result = pass(&text);
            assert_eq!(result.title, title);
            let after = result.after.expect("changes");
            assert_eq!(declaration(&after).title, title, "{after}");
            assert_settled(&after);
        }
    }

    // -- vault fixtures ------------------------------------------------------

    fn git(vault: &Path, args: &[&str]) -> String {
        let output = std::process::Command::new("git")
            .arg("-C")
            .arg(vault)
            .args(args)
            .output()
            .expect("git runs");
        assert!(output.status.success(), "git {args:?} failed");
        String::from_utf8_lossy(&output.stdout).trim().to_owned()
    }

    fn fixture(albums: &[(&str, &str)]) -> (tempfile::TempDir, PathBuf) {
        let temp = tempfile::tempdir().expect("tempdir");
        let vault = temp.path().join("vault");
        fs::create_dir_all(&vault).expect("vault");
        fs::write(
            vault.join("body.md"),
            "---\ntype: [[area]]\n---\n\n# Body\n",
        )
        .expect("body");
        for (path, contents) in albums {
            fs::write(vault.join(path), contents).expect("album");
        }
        git(&vault, &["init", "-q"]);
        git(&vault, &["config", "user.email", "t@t"]);
        git(&vault, &["config", "user.name", "t"]);
        git(&vault, &["add", "-A"]);
        git(&vault, &["commit", "-qm", "init"]);
        (temp, vault)
    }

    fn album(noun: &str, title_line: &str, body: &str) -> String {
        format!(
            "---\ntype: \"[[album]]\"\nparent: \"[[body]]\"\n{title_line}summary: S.\nrecord:\n  noun: {noun}\n---\n{body}"
        )
    }

    fn plan_over(
        vault: &Path,
        stem: Option<&str>,
    ) -> Result<TidyPlan, TxnFailure> {
        let snapshot = VaultSnapshot::load(vault);
        let registry = AlbumRegistry::from_snapshot(vault, &snapshot);
        plan_tidy(vault, &registry, &snapshot, stem)
    }

    #[test]
    fn dry_run_plans_every_album_and_writes_nothing() {
        let (_temp, vault) = fixture(&[
            ("eat.md", &album("restaurant", "", "\n# Restaurants\n")),
            (
                "movies.md",
                &album("movie", "title: Movies\n", "\n# Movies\n"),
            ),
            ("bad.md", &album("bad_thing", "title: 42\n", "\n# Bad\n")),
        ]);
        let head = git(&vault, &["rev-parse", "HEAD"]);
        let before = fs::read(vault.join("eat.md")).expect("eat");
        let report = dry_run_report(&vault, None);
        assert!(report.failure.is_none(), "{:?}", report.failure);
        let payload = report.to_json();
        assert_eq!(payload["dry_run"], true);
        assert_eq!(payload["counts"]["scanned"], 3);
        assert_eq!(payload["counts"]["changed"], 2);
        assert_eq!(payload["counts"]["title_added"], 1);
        assert_eq!(payload["counts"]["h1_removed"], 2);
        assert_eq!(payload["counts"]["h1_kept"], 0);
        assert_eq!(payload["changes"][0]["path"], "eat.md");
        assert_eq!(payload["changes"][0]["title"], "Restaurants");
        assert!(payload["changes"][0]["diff"]
            .as_str()
            .is_some_and(|diff| diff.contains("+title: Restaurants")));
        assert_eq!(payload["changes"][1]["title_added"], false);
        assert_eq!(
            payload["warnings"][0],
            "bad.md: frontmatter title is not one nonblank line; fix it by hand"
        );
        assert!(payload["commit"].is_null());
        assert!(payload["sync"].is_null());
        assert_eq!(report.exit_code(), 0);
        assert_eq!(git(&vault, &["rev-parse", "HEAD"]), head);
        assert_eq!(fs::read(vault.join("eat.md")).expect("eat"), before);
    }

    #[test]
    fn second_plan_is_empty_and_stem_selects_one_album() {
        let (_temp, vault) = fixture(&[
            ("eat.md", &album("restaurant", "", "\n# Restaurants\n")),
            ("movies.md", &album("movie", "", "\n# Movies\n")),
        ]);
        let plan = plan_over(&vault, Some("eat")).expect("plans");
        assert_eq!(plan.scanned, 1);
        assert_eq!(plan.changes.len(), 1);
        assert_eq!(plan.changes[0].path, "eat.md");
        for edit in &plan.edits {
            fs::write(vault.join(&edit.path), &edit.after).expect("write");
        }
        let again = plan_over(&vault, Some("eat")).expect("replans");
        assert!(again.changes.is_empty());
        assert!(again.warnings.is_empty());
        let all = plan_over(&vault, None).expect("plans all");
        assert_eq!(all.scanned, 2);
        assert_eq!(
            all.changes
                .iter()
                .map(|c| c.path.as_str())
                .collect::<Vec<_>>(),
            ["movies.md"]
        );
    }

    #[test]
    fn unknown_stem_and_partial_scans_refuse() {
        let (_temp, vault) =
            fixture(&[("eat.md", &album("restaurant", "", "\n# Eat\n"))]);
        let failure = plan_over(&vault, Some("restaurant"))
            .expect_err("nouns never select");
        assert_eq!(failure.code, "unknown_album");
        let report = dry_run_report(&vault, Some("nope"));
        assert_eq!(report.exit_code(), 2);
        assert_eq!(report.to_json()["error"]["code"], "unknown_album");

        let snapshot = VaultSnapshot::load(&vault);
        let mut registry = AlbumRegistry::from_snapshot(&vault, &snapshot);
        registry.health =
            DiscoveryHealth::Partial(vec!["eat/x.md: denied".to_owned()]);
        let failure = plan_tidy(&vault, &registry, &snapshot, None)
            .expect_err("partial scans refuse");
        assert_eq!(failure.code, "io");
        assert!(failure.message.contains("the vault scan was incomplete"));
    }

    #[test]
    fn transaction_subject_and_verify_guard_the_written_bytes() {
        let (_temp, vault) = fixture(&[(
            "eat.md",
            &album("restaurant", "", "\n# Restaurants\n"),
        )]);
        let plan = plan_over(&vault, None).expect("plans");
        let after = plan.edits[0].after.clone();
        let txn = planned_txn(&vault, plan);
        assert_eq!(txn.subject, "albums: tidy (1 notes)");
        assert_eq!(txn.kind, "album");

        let expected = [(PathBuf::from("eat.md"), "Restaurants".to_owned())];
        fs::write(vault.join("eat.md"), &after).expect("write");
        assert!(verify_tidied(&vault, &expected).is_ok());

        // A write that keeps the redundant H1 still plans a change. The
        // tidied body is empty, so the album ends at its closing `---`.
        assert!(after.ends_with("\n---\n"), "{after}");
        let redundant = format!("{after}\n# Restaurants\n");
        fs::write(vault.join("eat.md"), &redundant).expect("write");
        let error = verify_tidied(&vault, &expected).expect_err("redundant");
        assert!(error.contains("second pass"), "{error}");

        // A write whose title drifted is rejected.
        fs::write(
            vault.join("eat.md"),
            after.replace("title: Restaurants", "title: Other"),
        )
        .expect("write");
        let error = verify_tidied(&vault, &expected).expect_err("title");
        assert!(error.contains("\"Other\""), "{error}");
    }

    #[test]
    fn applied_run_commits_once_and_reruns_clean() {
        let (_temp, vault) = fixture(&[(
            "eat.md",
            &album("restaurant", "", "\n# Restaurants\n"),
        )]);
        let plan = plan_over(&vault, None).expect("plans");
        let (commit, context) = host_catalog::apply_and_commit(
            &vault,
            &Vec::new(),
            planned_txn(&vault, plan),
        )
        .expect("applies");
        let commit = commit.expect("commits");
        assert_eq!(commit.subject, "albums: tidy (1 notes)");
        assert_eq!(context.changes.len(), 1);
        assert_eq!(
            git(&vault, &["log", "-1", "--format=%s"]),
            "albums: tidy (1 notes)"
        );
        let plan = plan_over(&vault, None).expect("replans");
        assert!(plan.changes.is_empty());
        let (commit, _) = host_catalog::apply_and_commit(
            &vault,
            &Vec::new(),
            planned_txn(&vault, plan),
        )
        .expect("no-op");
        assert!(commit.is_none());
    }
}
