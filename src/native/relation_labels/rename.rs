//! Previewed, verified, single-commit relation-label rename.
//!
//! Only relation-label text is replaced. The LINKS edges and their order,
//! LINKED FROM mirrors, task state, and every other note byte stay intact.

use std::{
    collections::BTreeMap,
    ffi::OsString,
    fs,
    ops::Range,
    path::{Path, PathBuf},
    rc::Rc,
};

use clap::{
    builder::OsStringValueParser, Arg, ArgAction, ArgMatches,
    Command as ClapCommand,
};
use serde::Serialize;
use serde_json::json;

use crate::native::{
    capture_dependency_tasks, env as bob_env,
    markdown::fenced_lines,
    ob,
    relation_labels::{self, journal, Catalog},
    task_dependencies::blocks::has_task_tag,
    task_relations::{
        label::{inverse_phrase, normalize_relation_label, LabelError},
        parse::{
            linked_from_children_of, linked_from_label_spans,
            links_children_of, links_label_spans, RelationLabelSpan,
        },
        LinkedFromLine, LinksLine,
    },
};

const COMMAND_NAME: &str = "bob relations rename";
const SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OutputFormat {
    Human,
    Json,
}

impl OutputFormat {
    fn from_matches(matches: &ArgMatches) -> Self {
        match matches
            .get_one::<String>("format")
            .map(String::as_str)
            .unwrap_or("human")
        {
            "json" => Self::Json,
            _ => Self::Human,
        }
    }
}

#[derive(Debug, Clone)]
struct RenameFailure {
    code: &'static str,
    message: String,
    hint: Option<String>,
}

impl RenameFailure {
    fn new(
        code: &'static str,
        message: impl Into<String>,
        hint: Option<impl Into<String>>,
    ) -> Self {
        Self {
            code,
            message: message.into(),
            hint: hint.map(Into::into),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
struct RenameSide {
    key: String,
    name: String,
}

#[derive(Debug, Clone, Serialize)]
struct LineChange {
    line: usize,
    kind: &'static str,
    before: String,
    after: String,
}

#[derive(Debug, Clone, Serialize)]
struct FilePlanView {
    path: String,
    links: usize,
    mirrors: usize,
    lines: Vec<LineChange>,
}

#[derive(Debug, Clone, Serialize)]
struct RenameCounts {
    links: usize,
    notes: usize,
    mirrors: usize,
    done_links: usize,
    done_notes: usize,
}

#[derive(Debug, Clone, Serialize)]
struct StateChange {
    hidden_before: bool,
    hidden_after: bool,
    moved: bool,
    note: String,
}

#[derive(Debug, Clone, Serialize)]
struct InverseChange {
    before: Option<String>,
    after: Option<String>,
    note: String,
}

#[derive(Debug, Clone, Serialize)]
struct SyncInfo {
    pre: String,
    post: String,
}

#[derive(Debug, Clone, Serialize)]
struct RenameReport {
    schema_version: u32,
    ok: bool,
    mode: String,
    changed: bool,
    from: RenameSide,
    to: RenameSide,
    merge: bool,
    files: Vec<FilePlanView>,
    counts: RenameCounts,
    state: StateChange,
    inverse: InverseChange,
    replacement: String,
    hide_replacement: String,
    commit: Option<serde_json::Value>,
    sync: SyncInfo,
    warnings: Vec<String>,
}

#[derive(Debug, Clone)]
struct FilePlan {
    path: String,
    links: usize,
    mirrors: usize,
    done: bool,
    lines: Vec<LineChange>,
    before_contents: String,
    after_contents: String,
}

#[derive(Debug, Clone)]
struct PreparedRename {
    old_key: String,
    new_key: String,
    new_name: String,
    via: String,
    files: Vec<FilePlan>,
    total_entries: usize,
    report: RenameReport,
}

pub(crate) fn build_cli_rename() -> ClapCommand {
    ClapCommand::new(COMMAND_NAME)
        .about("Rename a relation label everywhere in one commit")
        .long_about(concat!(
            "Rename one relation label across every task note, including done/ and the previous daily note, in one verified Git commit. ",
            "Only label spans on LINKS and LINKED FROM lines change; entries keep their order and every other byte is preserved.\n\n",
            "The preview scans the current vault. A write takes bob_sync.lock, pre-syncs unless --offline, snapshots each target and compares again before writing, ",
            "appends one relation_label.rename event, verifies the result, makes one commit, then post-syncs. Undo with git revert.\n\n",
            "Online writes require -b/--bob-dir and BOB_DIR (default ~/bob) to resolve to the same vault; a mismatch refuses before locks or sync. Set BOB_DIR to the selected vault or use --offline. Previews and offline writes do not require this match."
        ))
        .after_help(concat!(
            "Examples:\n",
            "  bob relations rename \"inspired by\" \"prompted by\" -d\n",
            "  bob relations rename \"inspired by\" \"prompted by\" -d -f json\n",
            "  bob relations rename old new --merge\n",
            "  bob relations rename old new -o\n\n",
            "Environment:\n",
            "  BOB_CONFIG_FILE         Exact Bob config file; defaults to ~/.config/bob/config.yml\n",
            "  BOB_DIR                   Bob vault root when --bob-dir is omitted\n",
            "  HOSTNAME                  Host label for the journal file; same slug vault-sync uses\n",
            "  BOB_VAULT_SYNC_LOCK_FILE  Maintenance lock; defaults to the runtime bob_sync.lock\n\n",
            "Exit codes:\n",
            "  0  renamed (or dry-run planned)\n",
            "  1  unknown label, invalid label, unchanged, label exists, malformed line, lock busy, dirty paths, not a worktree, sync failed, stale note, verify failed, or I/O failure\n",
            "  2  usage error\n\n",
            "See docs/task-relations.md for the rename contract, refusal table, and revert runbook."
        ))
        .disable_help_flag(true)
        .arg(bob_dir_arg())
        .arg(dry_run_arg())
        .arg(format_arg())
        .arg(help_arg())
        .arg(merge_arg())
        .arg(offline_arg())
        .arg(via_arg())
        .arg(old_arg())
        .arg(new_arg())
}

fn bob_dir_arg() -> Arg {
    Arg::new("bob-dir")
        .long("bob-dir")
        .short('b')
        .value_name("DIR")
        .value_parser(OsStringValueParser::new())
        .help("Bob vault root; defaults to BOB_DIR or ~/bob")
}

fn dry_run_arg() -> Arg {
    Arg::new("dry-run")
        .long("dry-run")
        .short('d')
        .action(ArgAction::SetTrue)
        .help("Preview the rename without writing, committing, or syncing")
}

fn format_arg() -> Arg {
    Arg::new("format")
        .long("format")
        .short('f')
        .value_name("FORMAT")
        .value_parser(["human", "json"])
        .default_value("human")
        .help("Output format: human or json")
}

fn help_arg() -> Arg {
    Arg::new("help")
        .long("help")
        .short('h')
        .action(ArgAction::Help)
        .help("Show help")
}

fn merge_arg() -> Arg {
    Arg::new("merge")
        .long("merge")
        .short('m')
        .action(ArgAction::SetTrue)
        .help("Merge OLD into NEW when NEW is already used")
}

fn offline_arg() -> Arg {
    Arg::new("offline")
        .long("offline")
        .short('o')
        .action(ArgAction::SetTrue)
        .help("Commit locally; skip vault sync before and after")
}

fn via_arg() -> Arg {
    Arg::new("via")
        .long("via")
        .value_name("CLIENT")
        .hide(true)
        .help("Internal client id written on the journal event")
}

fn old_arg() -> Arg {
    Arg::new("old")
        .value_name("OLD")
        .required(true)
        .help("Current relation label")
}

fn new_arg() -> Arg {
    Arg::new("new")
        .value_name("NEW")
        .required(true)
        .help("New relation label")
}

pub(crate) fn run_rename(args: Vec<OsString>) -> i32 {
    let mut command = build_cli_rename();
    let matches = match command.try_get_matches_from_mut(
        std::iter::once(OsString::from(COMMAND_NAME)).chain(args),
    ) {
        Ok(matches) => matches,
        Err(error) => {
            let exit_code = error.exit_code();
            if let Err(print_error) = error.print() {
                // A closed pipe renders nothing: stay quiet instead of
                // reporting the failed render, but keep clap's own exit
                // code so a usage error never reports success.
                if print_error.kind() != std::io::ErrorKind::BrokenPipe {
                    eprintln!("{COMMAND_NAME}: failed to print command-line error: {print_error}");
                }
            }
            return exit_code;
        }
    };
    let format = OutputFormat::from_matches(&matches);
    if matches.get_flag("dry-run") {
        return match prepare_from_matches(&matches) {
            Ok(prepared) => {
                print_success(&prepared.report, format);
                0
            }
            Err(failure) => print_failure(format, failure),
        };
    }
    run_live(&matches, format)
}

fn prepare_from_matches(
    matches: &ArgMatches,
) -> Result<PreparedRename, RenameFailure> {
    let vault = matches
        .get_one::<OsString>("bob-dir")
        .map(PathBuf::from)
        .map(|path| bob_env::expand_tilde(&path))
        .unwrap_or_else(bob_env::bob_dir);
    let old_raw = matches
        .get_one::<String>("old")
        .map(String::as_str)
        .unwrap_or("");
    let new_raw = matches
        .get_one::<String>("new")
        .map(String::as_str)
        .unwrap_or("");
    let old_name = normalize_label(old_raw, "OLD")?;
    let new_name = normalize_label(new_raw, "NEW")?;
    let via = matches
        .get_one::<String>("via")
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .unwrap_or("bob-cli")
        .to_string();
    plan_rename(
        &vault,
        &old_name,
        &new_name,
        matches.get_flag("merge"),
        &via,
        matches.get_flag("dry-run"),
    )
}

fn normalize_label(raw: &str, which: &str) -> Result<String, RenameFailure> {
    normalize_relation_label(raw).map_err(|error| {
        let detail = match error {
            LabelError::Empty => "must not be empty",
            LabelError::TooLong => "must be at most 40 characters",
            LabelError::ForbiddenCharacter => "contains a forbidden character",
            LabelError::InvalidStart => {
                "cannot begin with a middle dot or comma"
            }
        };
        RenameFailure::new(
            "invalid_label",
            format!("invalid relation label {which}: {detail}"),
            Some("choose a label accepted by bob relations hide"),
        )
    })
}

fn plan_rename(
    vault: &Path,
    old_name: &str,
    new_name: &str,
    merge: bool,
    via: &str,
    dry_run: bool,
) -> Result<PreparedRename, RenameFailure> {
    let old_key = relation_labels::relation_label_key(old_name);
    let new_key = relation_labels::relation_label_key(new_name);
    relation_labels::invalidate_scan(vault);
    let catalog = Catalog::load(vault).map_err(|error| {
        RenameFailure::new(
            "io",
            format!("read relation catalog in {}: {error}", vault.display()),
            None::<String>,
        )
    })?;
    let (paths, walk_warnings) =
        capture_dependency_tasks::vault_note_paths(vault);
    if !walk_warnings.is_empty() {
        return Err(RenameFailure::new(
            "malformed_links_line",
            format!(
                "could not scan every note in the vault: {}",
                walk_warnings.join("; ")
            ),
            Some("resolve the note-walk warning before retrying the rename"),
        ));
    }

    let mut files = Vec::new();
    let mut total_entries = 0usize;
    let mut total_links = 0usize;
    let mut total_mirrors = 0usize;
    let mut done_links = 0usize;
    let mut done_notes = 0usize;
    let warnings = catalog.warnings().to_vec();
    for relative in paths {
        let absolute = vault.join(&relative);
        let path = display_relative(&relative);
        let contents = fs::read_to_string(&absolute).map_err(|error| {
            RenameFailure::new(
                "malformed_links_line",
                format!("could not read note {path}: {error}; rename cannot confirm full coverage"),
                Some("restore access to the note and retry"),
            )
        })?;
        let (plan, entries, links, mirrors) =
            plan_note(&path, &contents, &old_key, new_name)?;
        total_entries = total_entries.saturating_add(entries);
        total_links = total_links.saturating_add(links);
        total_mirrors = total_mirrors.saturating_add(mirrors);
        if let Some(plan) = plan {
            if plan.done {
                done_links = done_links.saturating_add(plan.links);
                done_notes = done_notes.saturating_add(1);
            }
            files.push(plan);
        }
    }

    let Some(old_record) = catalog.get(&old_key) else {
        return Err(RenameFailure::new(
            "unknown_label",
            format!("unknown relation label: {old_name}"),
            Some("list labels with bob relations -a"),
        ));
    };
    if old_key != new_key && catalog.get(&new_key).is_some() && !merge {
        return Err(RenameFailure::new(
            "label_exists",
            format!(
                "{new_name} is already a relation label; pass --merge to combine {old_name} into it"
            ),
            Some("re-run with --merge to rename the labels together"),
        ));
    }

    let old_display = old_record.name.clone();
    let hidden_before = catalog.is_hidden(&old_key);
    let source_has_state = catalog.has_explicit_state(&old_key);
    let target_has_state = catalog.has_explicit_state(&new_key);
    let hidden_after = if old_key == new_key {
        hidden_before
    } else if target_has_state {
        catalog.is_hidden(&new_key)
    } else {
        hidden_before
    };
    let moved = old_key != new_key && source_has_state && !target_has_state;
    let state_note = if moved && hidden_before {
        format!("hidden state moves from {old_display} to {new_name}")
    } else if moved {
        format!("visible state moves from {old_display} to {new_name}")
    } else if hidden_before && old_key != new_key {
        format!(
            "{old_display} stays hidden; {new_name} already has explicit state"
        )
    } else if hidden_before {
        format!("{new_name} remains hidden")
    } else {
        format!("{new_name} is not hidden")
    };
    let before_inverse = inverse_phrase(&old_display).map(str::to_string);
    let after_inverse = inverse_phrase(new_name).map(str::to_string);
    let inverse_note = if before_inverse == after_inverse {
        "inverse phrase is unchanged".to_string()
    } else {
        format!(
            "inverse phrase changes from {} to {}",
            before_inverse.as_deref().unwrap_or("none"),
            after_inverse.as_deref().unwrap_or("none"),
        )
    };
    let changed = total_links > 0
        || total_mirrors > 0
        || moved
        || (old_key == new_key && old_display != new_name);
    if !changed {
        return Err(RenameFailure::new(
            "unchanged",
            format!("{old_name} is already named {new_name}"),
            Some("choose a different NEW spelling"),
        ));
    }

    let report = RenameReport {
        schema_version: SCHEMA_VERSION,
        ok: true,
        mode: if dry_run { "dry_run" } else { "write" }.to_string(),
        changed,
        from: RenameSide {
            key: old_key.clone(),
            name: old_display,
        },
        to: RenameSide {
            key: new_key.clone(),
            name: new_name.to_string(),
        },
        merge,
        files: files.iter().map(FilePlan::view).collect(),
        counts: RenameCounts {
            links: total_links,
            notes: files.len(),
            mirrors: total_mirrors,
            done_links,
            done_notes,
        },
        state: StateChange {
            hidden_before,
            hidden_after,
            moved,
            note: state_note,
        },
        inverse: InverseChange {
            before: before_inverse,
            after: after_inverse,
            note: inverse_note,
        },
        replacement:
            crate::native::task_relations::label::relation_label_quoted_form(
                new_name,
            ),
        hide_replacement:
            crate::native::task_relations::label::relation_label_hide_form(
                new_name,
            ),
        commit: None,
        sync: SyncInfo {
            pre: "skipped".to_string(),
            post: if dry_run { "not_run" } else { "pending" }.to_string(),
        },
        warnings,
    };
    Ok(PreparedRename {
        old_key,
        new_key,
        new_name: new_name.to_string(),
        via: via.to_string(),
        files,
        total_entries,
        report,
    })
}

impl FilePlan {
    fn view(&self) -> FilePlanView {
        FilePlanView {
            path: self.path.clone(),
            links: self.links,
            mirrors: self.mirrors,
            lines: self.lines.clone(),
        }
    }
}

struct LineView<'a> {
    body: &'a str,
    start: usize,
}

#[derive(Debug, Clone)]
struct StagedLine {
    kind: &'static str,
    before: String,
    edits: Vec<(Range<usize>, String)>,
}

fn plan_note(
    path: &str,
    contents: &str,
    old_key: &str,
    new_name: &str,
) -> Result<(Option<FilePlan>, usize, usize, usize), RenameFailure> {
    let views = line_views(contents);
    let lines = views.iter().map(|line| line.body).collect::<Vec<_>>();
    let fenced = fenced_lines(&lines, 0..lines.len());
    let mut total_entries = 0usize;
    let mut links = 0usize;
    let mut mirrors = 0usize;
    let mut staged = BTreeMap::<usize, StagedLine>::new();

    for (task_line, line) in lines.iter().enumerate() {
        if fenced.contains(&task_line) || !has_task_tag(line) {
            continue;
        }
        for child in links_children_of(&lines, &fenced, task_line) {
            let raw = lines[child.line_index];
            match child.parsed {
                LinksLine::Accepted { entries, .. } => {
                    total_entries = total_entries.saturating_add(entries.len());
                    if let Some(spans) = links_label_spans(raw) {
                        links += stage_spans(
                            &mut staged,
                            child.line_index,
                            "links",
                            raw,
                            &spans,
                            old_key,
                            new_name,
                        );
                    }
                }
                LinksLine::Malformed if line_might_name(raw, old_key) => {
                    return Err(malformed_line_failure(
                        path,
                        child.line_index,
                        "LINKS",
                        old_key,
                    ));
                }
                LinksLine::Malformed
                | LinksLine::Empty
                | LinksLine::NotALine => {}
            }
        }
        for child in linked_from_children_of(&lines, &fenced, task_line) {
            let raw = lines[child.line_index];
            match child.parsed {
                LinkedFromLine::Accepted { .. } => {
                    if let Some(spans) = linked_from_label_spans(raw) {
                        mirrors += stage_spans(
                            &mut staged,
                            child.line_index,
                            "linked_from",
                            raw,
                            &spans,
                            old_key,
                            new_name,
                        );
                    }
                }
                LinkedFromLine::Malformed if line_might_name(raw, old_key) => {
                    return Err(malformed_line_failure(
                        path,
                        child.line_index,
                        "LINKED FROM",
                        old_key,
                    ));
                }
                LinkedFromLine::Malformed
                | LinkedFromLine::Empty
                | LinkedFromLine::NotALine => {}
            }
        }
    }

    if staged.is_empty() {
        return Ok((None, total_entries, links, mirrors));
    }

    let mut line_changes = Vec::with_capacity(staged.len());
    let mut document_edits = Vec::<(Range<usize>, String)>::new();
    let mut link_count = 0usize;
    let mut mirror_count = 0usize;
    for (index, staged_line) in staged {
        let Some(view) = views.get(index) else {
            continue;
        };
        let after = replace_spans(&staged_line.before, &staged_line.edits);
        if staged_line.kind == "links" {
            link_count += staged_line.edits.len();
        } else {
            mirror_count += staged_line.edits.len();
        }
        for (range, replacement) in staged_line.edits {
            document_edits.push((
                view.start + range.start..view.start + range.end,
                replacement,
            ));
        }
        line_changes.push(LineChange {
            line: index + 1,
            kind: staged_line.kind,
            before: staged_line.before,
            after,
        });
    }
    line_changes.sort_by_key(|line| line.line);
    let after_contents = replace_spans(contents, &document_edits);
    let done = Path::new(path)
        .components()
        .any(|component| component.as_os_str() == "done");
    Ok((
        Some(FilePlan {
            path: path.to_string(),
            links: link_count,
            mirrors: mirror_count,
            done,
            lines: line_changes,
            before_contents: contents.to_string(),
            after_contents,
        }),
        total_entries,
        links,
        mirrors,
    ))
}

fn line_views(contents: &str) -> Vec<LineView<'_>> {
    let mut start = 0usize;
    contents
        .split_inclusive('\n')
        .map(|line| {
            let mut body = line.strip_suffix('\n').unwrap_or(line);
            body = body.strip_suffix('\r').unwrap_or(body);
            let view = LineView { body, start };
            start += line.len();
            view
        })
        .collect()
}

fn stage_spans(
    staged: &mut BTreeMap<usize, StagedLine>,
    line_index: usize,
    kind: &'static str,
    line: &str,
    spans: &[RelationLabelSpan],
    old_key: &str,
    new_name: &str,
) -> usize {
    let matched = spans
        .iter()
        .filter(|span| {
            relation_labels::relation_label_key(&span.label) == old_key
                && span.label != new_name
        })
        .collect::<Vec<_>>();
    if matched.is_empty() {
        return 0;
    }
    let row = staged.entry(line_index).or_insert_with(|| StagedLine {
        kind,
        before: line.to_string(),
        edits: Vec::new(),
    });
    row.edits.extend(
        matched
            .iter()
            .map(|span| (span.range.clone(), new_name.to_string())),
    );
    matched.len()
}

fn replace_spans(source: &str, edits: &[(Range<usize>, String)]) -> String {
    let mut ordered = edits.to_vec();
    ordered.sort_by(|left, right| right.0.start.cmp(&left.0.start));
    let mut output = source.to_string();
    for (range, replacement) in ordered {
        if range.end <= output.len() && range.start <= range.end {
            output.replace_range(range, &replacement);
        }
    }
    output
}

fn line_might_name(line: &str, old_key: &str) -> bool {
    relation_labels::relation_label_key(line).contains(old_key)
}

fn malformed_line_failure(
    path: &str,
    line_index: usize,
    kind: &str,
    old_name: &str,
) -> RenameFailure {
    RenameFailure::new(
        "malformed_links_line",
        format!(
            "malformed {kind} line {path}:{} may contain relation label {old_name}",
            line_index + 1
        ),
        Some("repair the malformed relation line and retry the rename"),
    )
}

fn display_relative(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn run_live(matches: &ArgMatches, format: OutputFormat) -> i32 {
    let child_env = ob::child_env();
    let vault = matches
        .get_one::<OsString>("bob-dir")
        .map(PathBuf::from)
        .map(|path| bob_env::expand_tilde(&path))
        .unwrap_or_else(bob_env::bob_dir);
    let offline = matches.get_flag("offline");
    let json_mode = format == OutputFormat::Json;
    let outcome = crate::native::vault_rewrite::run(
        &vault,
        &child_env,
        COMMAND_NAME,
        "relation label renames",
        offline,
        json_mode,
        || {
            let prepared =
                prepare_from_matches(matches).map_err(|failure| {
                    crate::native::vault_rewrite::TxnFailure::new(
                        failure.code,
                        failure.message,
                        failure.hint,
                    )
                })?;
            let inner = Rc::new(prepared);
            let files = inner
                .files
                .iter()
                .map(|file| crate::native::vault_rewrite::FileRewrite {
                    path: file.path.clone(),
                    before_contents: file.before_contents.clone(),
                    after_contents: file.after_contents.clone(),
                })
                .collect::<Vec<_>>();
            let subject = format!(
                "bob relations rename: {} → {}",
                inner.report.from.name, inner.new_name
            );
            let post_inner = Rc::clone(&inner);
            let post_vault = vault.clone();
            let verify_inner = Rc::clone(&inner);
            let verify_vault = vault.clone();
            let invalidate_vault = vault.clone();
            Ok(crate::native::vault_rewrite::ReadyPlan {
                files,
                subject,
                post_write: Box::new(move || {
                    append_rename_event(&post_vault, &post_inner)
                        .map(|path| vec![path])
                }),
                verify: Box::new(move || {
                    verify_renamed(&verify_vault, &verify_inner)
                }),
                on_committed: Box::new(move || {
                    relation_labels::invalidate_scan(&invalidate_vault);
                }),
                context: inner,
            })
        },
    );
    match outcome {
        Ok(outcome) => {
            let mut report = outcome.context.report.clone();
            report.mode = "write".to_string();
            report.sync.pre = outcome.sync_pre.to_string();
            report.sync.post = outcome.sync_post.to_string();
            report.commit = Some(json!({
                "sha": outcome.sha,
                "subject": outcome.subject,
            }));
            if let Some(dirty) = outcome.dirty_warning {
                report.warnings.push(dirty);
            }
            if let Some(warning) = outcome.post_sync_warning {
                report.warnings.push(warning);
            }
            print_success(&report, format);
            0
        }
        Err(failure) => print_failure(
            format,
            RenameFailure::new(failure.code, failure.message, failure.hint),
        ),
    }
}

fn append_rename_event(
    vault: &Path,
    prepared: &PreparedRename,
) -> Result<PathBuf, String> {
    let host = journal::host_label();
    let ts = journal::event_timestamp();
    let payload = journal::EventPayload::Rename {
        from: prepared.old_key.clone(),
        to: prepared.new_key.clone(),
        from_name: prepared.report.from.name.clone(),
        to_name: prepared.new_name.clone(),
    };
    let path = journal::host_journal_path(vault, &host);
    journal::append_event_at(vault, &host, &payload, &prepared.via, ts)
        .map_err(|error| {
            format!("could not append relation_label.rename: {error}")
        })?;
    Ok(path)
}

fn verify_renamed(
    vault: &Path,
    prepared: &PreparedRename,
) -> Result<(), String> {
    let paths = capture_dependency_tasks::vault_note_paths(vault).0;
    let planned = prepared
        .files
        .iter()
        .map(|file| (file.path.as_str(), file))
        .collect::<BTreeMap<_, _>>();
    let mut total_entries = 0usize;
    for relative in paths {
        let path = display_relative(&relative);
        let absolute = vault.join(&relative);
        let contents = fs::read_to_string(&absolute)
            .map_err(|error| format!("reread {path}: {error}"))?;
        if let Some(file) = planned.get(path.as_str())
            && contents != file.after_contents
        {
            return Err(format!("{path} changed during the rename"));
        }
        let lines = line_views(&contents)
            .into_iter()
            .map(|line| line.body)
            .collect::<Vec<_>>();
        let fenced = fenced_lines(&lines, 0..lines.len());
        for (task_line, line) in lines.iter().enumerate() {
            if fenced.contains(&task_line) || !has_task_tag(line) {
                continue;
            }
            for child in links_children_of(&lines, &fenced, task_line) {
                let raw = lines[child.line_index];
                match child.parsed {
                    LinksLine::Accepted { entries, .. } => {
                        total_entries =
                            total_entries.saturating_add(entries.len());
                        if let Some(spans) = links_label_spans(raw) {
                            verify_spans(
                                &path,
                                child.line_index,
                                &spans,
                                prepared,
                            )?;
                        }
                    }
                    LinksLine::Malformed
                        if line_might_name(raw, &prepared.old_key) =>
                    {
                        return Err(format!(
                            "malformed LINKS line remains in {path}:{}",
                            child.line_index + 1
                        ));
                    }
                    LinksLine::Malformed
                    | LinksLine::Empty
                    | LinksLine::NotALine => {}
                }
            }
            for child in linked_from_children_of(&lines, &fenced, task_line) {
                let raw = lines[child.line_index];
                match child.parsed {
                    LinkedFromLine::Accepted { .. } => {
                        if let Some(spans) = linked_from_label_spans(raw) {
                            verify_spans(
                                &path,
                                child.line_index,
                                &spans,
                                prepared,
                            )?;
                        }
                    }
                    LinkedFromLine::Malformed
                        if line_might_name(raw, &prepared.old_key) =>
                    {
                        return Err(format!(
                            "malformed LINKED FROM line remains in {path}:{}",
                            child.line_index + 1
                        ));
                    }
                    LinkedFromLine::Malformed
                    | LinkedFromLine::Empty
                    | LinkedFromLine::NotALine => {}
                }
            }
        }
    }
    if total_entries != prepared.total_entries {
        return Err(format!(
            "LINKS entry count changed (was {}, now {total_entries})",
            prepared.total_entries
        ));
    }
    Ok(())
}

fn verify_spans(
    path: &str,
    line_index: usize,
    spans: &[RelationLabelSpan],
    prepared: &PreparedRename,
) -> Result<(), String> {
    for span in spans {
        if relation_labels::relation_label_key(&span.label) == prepared.old_key
            && (prepared.old_key != prepared.new_key
                || span.label != prepared.new_name)
        {
            return Err(format!(
                "{} still contains the old relation label on line {}",
                path,
                line_index + 1
            ));
        }
    }
    Ok(())
}

pub(crate) fn renamed_reappeared(
    vault: &Path,
    catalog: &Catalog,
) -> Vec<serde_json::Value> {
    let Ok((events, _)) = journal::read_journal(vault) else {
        return Vec::new();
    };
    let mut latest = BTreeMap::<String, &journal::JournalEvent>::new();
    for event in &events {
        let journal::EventPayload::Rename { from, to, .. } = &event.payload
        else {
            continue;
        };
        if from == to {
            continue;
        }
        let replace = match latest.get(from) {
            Some(current) => {
                (
                    event.ts,
                    event.host.as_str(),
                    event.path.as_str(),
                    event.line,
                ) > (
                    current.ts,
                    current.host.as_str(),
                    current.path.as_str(),
                    current.line,
                )
            }
            None => true,
        };
        if replace {
            latest.insert(from.clone(), event);
        }
    }
    let mut rows = Vec::new();
    for (from, event) in latest {
        let Some(record) = catalog.get(&from) else {
            continue;
        };
        let journal::EventPayload::Rename { to, to_name, .. } = &event.payload
        else {
            continue;
        };
        rows.push(json!({
            "key": record.key,
            "name": record.name,
            "renamed_to": to,
            "renamed_to_name": to_name,
        }));
    }
    rows.sort_by(|left, right| {
        left.get("name")
            .and_then(serde_json::Value::as_str)
            .cmp(&right.get("name").and_then(serde_json::Value::as_str))
    });
    rows
}

fn print_success(report: &RenameReport, format: OutputFormat) {
    match format {
        OutputFormat::Json => match serde_json::to_string(report) {
            Ok(text) => println!("{text}"),
            Err(error) => {
                eprintln!("{COMMAND_NAME}: failed to serialize JSON: {error}")
            }
        },
        OutputFormat::Human => print_human(report),
    }
}

fn print_human(report: &RenameReport) {
    let styler = crate::native::style::Styler::detect();
    let dry = report.mode == "dry_run";
    println!(
        "{} rename {} → {}{}",
        styler.success_prefix(dry),
        report.from.name,
        report.to.name,
        if dry { " (dry run)" } else { "" },
    );
    for file in &report.files {
        println!(
            "  {}: {} link{}, {} mirror{}",
            file.path,
            file.links,
            if file.links == 1 { "" } else { "s" },
            file.mirrors,
            if file.mirrors == 1 { "" } else { "s" },
        );
        for line in &file.lines {
            println!(
                "    line {} {}: {} → {}",
                line.line, line.kind, line.before, line.after
            );
        }
    }
    println!(
        "  totals: {} links in {} notes, {} mirrors; {} links in done/",
        report.counts.links,
        report.counts.notes,
        report.counts.mirrors,
        report.counts.done_links,
    );
    println!("  state: {}", report.state.note);
    println!("  {}", report.inverse.note);
    if let Some(commit) = &report.commit {
        let sha = commit
            .get("sha")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("(unknown)");
        let subject = commit
            .get("subject")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("bob relations rename");
        println!("  committed {sha} {subject}");
    }
    if report.mode == "write" && report.sync.post == "failed" {
        println!("  sync pending");
    } else if report.mode == "write" {
        println!("  renamed locally; sync {}", report.sync.post);
    }
    for warning in &report.warnings {
        eprintln!("{}: {warning}", styler.warning_prefix());
    }
}

fn print_failure(format: OutputFormat, failure: RenameFailure) -> i32 {
    let exit = if failure.code == "usage" { 2 } else { 1 };
    match format {
        OutputFormat::Json => println!(
            "{}",
            json!({
                "schema_version": SCHEMA_VERSION,
                "ok": false,
                "code": failure.code,
                "message": failure.message,
            })
        ),
        OutputFormat::Human => {
            eprintln!("{COMMAND_NAME}: {}", failure.message);
            if let Some(hint) = failure.hint {
                eprintln!("hint: {hint}");
            }
        }
    }
    exit
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replace_spans_only_changes_the_requested_bytes() {
        let line = "  - 🪢 **LINKS:** old [[#^a]] • old [[#^b]]  ";
        let first = line.find("old").unwrap();
        let second = line.rfind("old").unwrap();
        assert_eq!(
            replace_spans(
                line,
                &[
                    (first..first + 3, "new label".to_string()),
                    (second..second + 3, "new label".to_string()),
                ],
            ),
            "  - 🪢 **LINKS:** new label [[#^a]] • new label [[#^b]]  "
        );
    }

    #[test]
    fn line_views_preserve_crlf_offsets_and_final_newline() {
        let contents = "one\r\ntwo\r\n";
        let lines = line_views(contents);
        assert_eq!(
            lines.iter().map(|line| line.body).collect::<Vec<_>>(),
            ["one", "two"]
        );
        assert_eq!(lines[1].start, 5);
    }
}
