//! CLI shell for `bob task rename-drawer`: scope, the dry-run report, and the
//! one-commit apply.
//!
//! Scope is every task drawer in every vault note, archive history under
//! `done/` included, plus every project note's eligible `##` sections.
//! `_generated/`, `_templates/`, and the other always-excluded directories
//! are never written; template hits are reported only. A bare run is a dry
//! run: it takes no lock, writes nothing, runs no Git, and does no sync. `-a`
//! takes the shared lock, pre-syncs, replans, writes every note guarded by
//! its full preimage, verifies that nothing replans, makes one commit,
//! post-syncs, and invalidates the drawer catalog.

use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet},
    ffi::OsString,
    fs, io,
    path::{Component, Path, PathBuf},
};

use clap::{
    builder::OsStringValueParser, Arg, ArgAction, ArgMatches,
    Command as ClapCommand,
};
use serde_json::{json, Value};
use similar::{ChangeTag, TextDiff};

use super::{
    content_preserved,
    links::{repair_heading_links, LinkRepair, LinkTargets, UnresolvedLink},
    merged, rename_note, settled, template_lookalikes, DrawerChange, Excluded,
    NoteRename, RenameSpec, SectionChange,
};
use crate::native::{
    capture_dependency_tasks::vault_note_paths,
    capture_drawers::{self, invalidate_drawer_catalog},
    env as bob_env,
    host_catalog::{
        self, CommitInfo, FileEdit, PlannedTxn, ReadDependencies, TxnResult,
    },
    note_tasks::{self, NoteTaskSettings},
    ob,
    style::Styler,
    vault_links::NoteIndex,
    vault_rewrite::TxnFailure,
};

const SCHEMA_VERSION: u32 = 1;
const COMMAND_NAME: &str = "bob task rename-drawer";
/// Archive history lives under this top-level folder.
const ARCHIVE_DIR: &str = "done";
/// Vault templates: reported, never edited.
const TEMPLATES_DIR: &str = "_templates";
/// Context lines around each changed run in a human excerpt.
const EXCERPT_CONTEXT: usize = 2;

// ---------------------------------------------------------------------------
// Vault plan
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct NoteChange {
    path: String,
    archive: bool,
    drawers: Vec<DrawerChange>,
    sections: Vec<SectionChange>,
    links: Vec<LinkRepair>,
    before: String,
    after: String,
}

impl NoteChange {
    fn drawers_renamed(&self) -> usize {
        self.drawers.iter().map(|change| change.renamed).sum()
    }

    fn sections_renamed(&self) -> usize {
        self.sections.iter().map(|change| change.renamed).sum()
    }

    fn merges(&self) -> usize {
        self.drawers
            .iter()
            .map(|change| merged(&change.titles))
            .chain(self.sections.iter().map(|change| merged(&change.titles)))
            .sum()
    }
}

#[derive(Debug, Clone)]
struct TemplateHit {
    path: String,
    line: usize,
    kind: &'static str,
    text: String,
}

#[derive(Debug, Clone, Default)]
struct RenameContext {
    notes_scanned: usize,
    changes: Vec<NoteChange>,
    refused: Vec<(String, String)>,
    excluded: Vec<(String, Excluded)>,
    unresolved: Vec<(String, UnresolvedLink)>,
    templates: Vec<TemplateHit>,
    warnings: Vec<String>,
    io_errors: Vec<String>,
}

/// Renamed task drawers by where their task lives.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct DrawerCounts {
    open: usize,
    closed: usize,
    archive: usize,
}

impl DrawerCounts {
    fn total(self) -> usize {
        self.open + self.closed + self.archive
    }
}

impl RenameContext {
    fn drawer_counts(&self) -> DrawerCounts {
        let mut counts = DrawerCounts::default();
        for change in &self.changes {
            for drawer in &change.drawers {
                let slot = if change.archive {
                    &mut counts.archive
                } else if drawer.open {
                    &mut counts.open
                } else {
                    &mut counts.closed
                };
                *slot += drawer.renamed;
            }
        }
        counts
    }

    fn sections(&self) -> usize {
        self.changes.iter().map(NoteChange::sections_renamed).sum()
    }

    fn merges(&self) -> usize {
        self.changes.iter().map(NoteChange::merges).sum()
    }

    fn links(&self) -> usize {
        self.changes.iter().map(|change| change.links.len()).sum()
    }
}

struct RenamePlan {
    context: RenameContext,
    edits: Vec<FileEdit>,
    dependencies: ReadDependencies,
    index: NoteIndex,
    renamed: BTreeSet<PathBuf>,
    refused: BTreeSet<PathBuf>,
}

fn slash(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn in_archive(relative: &Path) -> bool {
    matches!(
        relative.components().next(),
        Some(Component::Normal(name)) if name == ARCHIVE_DIR
    )
}

fn plan_vault(
    vault: &Path,
    spec: &RenameSpec,
    settings: &NoteTaskSettings,
) -> RenamePlan {
    let (paths, walk_errors) = vault_note_paths(vault);
    let index = NoteIndex::from_paths(paths.iter().cloned());
    let mut context = RenameContext {
        io_errors: walk_errors,
        ..RenameContext::default()
    };
    // Notes that mention OLD: their original bytes and rename pass.
    let mut candidates: BTreeMap<PathBuf, (String, NoteRename)> =
        BTreeMap::new();
    let mut renamed = BTreeSet::new();
    let mut refused = BTreeSet::new();
    for relative in &paths {
        let path = slash(relative);
        let contents = match fs::read_to_string(vault.join(relative)) {
            Ok(contents) => contents,
            Err(error) => {
                context
                    .io_errors
                    .push(format!("{path}: failed to read file: {error}"));
                continue;
            }
        };
        context.notes_scanned += 1;
        if !spec.may_match(&contents) {
            continue;
        }
        let mut pass = rename_note(&contents, spec, settings);
        context.excluded.extend(
            pass.excluded
                .iter()
                .cloned()
                .map(|item| (path.clone(), item)),
        );
        if pass.refusal.is_none()
            && let Some(after) = &pass.updated
            && let Err(reason) =
                content_preserved(&contents, after, spec, &pass)
                    .and_then(|()| settled(after, spec, settings))
        {
            pass = NoteRename {
                refusal: Some(format!(
                    "the planned rename is unsafe ({reason})"
                )),
                ..NoteRename::default()
            };
        }
        if let Some(reason) = &pass.refusal {
            context.refused.push((path.clone(), reason.clone()));
            refused.insert(relative.clone());
        } else if !pass.sections.is_empty() {
            renamed.insert(relative.clone());
        }
        candidates.insert(relative.clone(), (contents, pass));
    }
    let targets = LinkTargets {
        index: &index,
        renamed: &renamed,
        refused: &refused,
    };
    let mut edits = Vec::new();
    let mut dependencies = ReadDependencies::default();
    for (relative, (original, pass)) in candidates {
        let path = slash(&relative);
        let text = pass.updated.clone().unwrap_or_else(|| original.clone());
        let links = repair_heading_links(&text, &relative, &targets, spec);
        context.unresolved.extend(
            links
                .unresolved
                .into_iter()
                .map(|link| (path.clone(), link)),
        );
        let after = links.updated.unwrap_or(text);
        if after == original {
            continue;
        }
        context.changes.push(NoteChange {
            path,
            archive: in_archive(&relative),
            drawers: pass.drawers,
            sections: pass.sections,
            links: links.repairs,
            before: original.clone(),
            after: after.clone(),
        });
        edits.push(FileEdit {
            path: relative.clone(),
            before: original.clone(),
            after,
        });
        dependencies.insert(relative, Some(original));
    }
    context.templates = template_hits(vault, spec);
    RenamePlan {
        context,
        edits,
        dependencies,
        index,
        renamed,
        refused,
    }
}

/// OLD lookalikes in `_templates/`, reported and never edited.
fn template_hits(vault: &Path, spec: &RenameSpec) -> Vec<TemplateHit> {
    let root = vault.join(TEMPLATES_DIR);
    let mut files = Vec::new();
    collect_markdown(&root, &mut files);
    files.sort();
    let mut hits = Vec::new();
    for file in files {
        let Ok(contents) = fs::read_to_string(&file) else {
            continue;
        };
        if !spec.may_match(&contents) {
            continue;
        }
        let path = file
            .strip_prefix(vault)
            .map(slash)
            .unwrap_or_else(|_| slash(&file));
        for (line, kind, text) in template_lookalikes(&contents, spec) {
            hits.push(TemplateHit {
                path: path.clone(),
                line,
                kind,
                text,
            });
        }
    }
    hits
}

fn collect_markdown(directory: &Path, files: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_dir() {
            collect_markdown(&path, files);
        } else if kind.is_file()
            && path
                .extension()
                .is_some_and(|extension| extension.eq_ignore_ascii_case("md"))
        {
            files.push(path);
        }
    }
}

// ---------------------------------------------------------------------------
// CLI
// ---------------------------------------------------------------------------

pub(crate) fn build_cli() -> ClapCommand {
    ClapCommand::new(COMMAND_NAME)
        .about("Rename a task drawer and its project sections everywhere (dry run by default)")
        .long_about(
            "Rename the task drawer OLD to NEW everywhere: every bold drawer bullet under a #task, whether the task is open, done, cancelled, or archived under done/, plus every eligible ## section of a project note, which is its ^prj task's drawers. OLD matches by drawer key, so case and spacing do not matter. NEW is written verbatim as a bold task drawer title and a project ## section title. A same-key spelling change respells each matching header in place without merging duplicate identities. For different keys, where an owner already holds NEW or holds OLD more than once, the dry-run report shows the merge; apply keeps the earliest position and all body content in order. Heading links into renamed sections ([[note#Old]], ![[note#Old]], and Markdown note.md#Old) are repaired; block links never change. A project note whose distinct-name rename would leave a heading ambiguous is refused with the reason. Ordinary headings in other notes, fenced code, and quotes are out of scope and listed as excluded; _templates/ hits are reported and never edited. Without -a the run is a dry run: it prints each change with a colored excerpt and takes no lock, writes nothing, runs no Git, and does no sync. -a takes the shared vault lock, pre-syncs, replans, guards each note by its full preimage, verifies that nothing replans, makes one commit, post-syncs only when it committed, and refreshes the drawer catalog. A second run reports nothing.",
        )
        .after_help(
            "Examples:\n  bob task rename-drawer REQUIREMENTS Notes       Preview a true-case respelling\n  bob task rename-drawer requirements 'Future Work' -a  Apply it in one commit\n  bob task rename-drawer todo later -f json       Print the plan as JSON\n\nSee docs/task-drawers.md for the drawer contract.",
        )
        .disable_help_flag(true)
        .arg(
            Arg::new("old")
                .value_name("OLD")
                .required(true)
                .help("Drawer to rename, matched by drawer key (case and spacing ignored)"),
        )
        .arg(
            Arg::new("new")
                .value_name("NEW")
                .required(true)
                .help("New drawer title, written verbatim"),
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

pub(crate) fn run(args: Vec<OsString>) -> i32 {
    let mut command = build_cli();
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
    let old = matches
        .get_one::<String>("old")
        .cloned()
        .unwrap_or_default();
    let new = matches
        .get_one::<String>("new")
        .cloned()
        .unwrap_or_default();
    let dry_run = !matches.get_flag("apply");
    let mut report = RenameReport {
        dry_run,
        spec: None,
        old_input: old.clone(),
        new_input: new.clone(),
        context: RenameContext::default(),
        commit: None,
        sync_pre: None,
        sync_post: None,
        failure: None,
    };
    match RenameSpec::parse(&old, &new) {
        Ok(spec) => {
            report.spec = Some(spec.clone());
            if dry_run {
                let settings = note_tasks::read_settings(&vault);
                report.context = plan_vault(&vault, &spec, &settings).context;
            } else {
                apply(&vault, spec, json_mode, &mut report);
            }
        }
        Err(refusal) => {
            report.failure = Some(TxnFailure::new(
                refusal.code,
                refusal.message,
                Some(refusal.hint),
            ));
        }
    }
    if json_mode {
        println!("{}", report.to_json());
    } else {
        report.print_human();
    }
    report.exit_code()
}

/// `-a`: the shared lock, pre-sync, a fresh plan, one guarded transaction,
/// a post-sync only when it committed, and a drawer-catalog refresh.
fn apply(
    vault: &Path,
    spec: RenameSpec,
    json_mode: bool,
    report: &mut RenameReport,
) {
    let child_env = ob::child_env();
    let captured: RefCell<Option<RenameContext>> = RefCell::new(None);
    let result =
        host_catalog::run_locked(vault, &child_env, json_mode, "task", || {
            let settings = note_tasks::read_settings(vault);
            let plan = plan_vault(vault, &spec, &settings);
            *captured.borrow_mut() = Some(plan.context.clone());
            Ok(planned_txn(vault, plan, spec.clone(), settings))
        });
    match result {
        Ok(result) => applied(result, vault, report),
        Err(failure) => {
            report.context = captured.into_inner().unwrap_or_default();
            report.failure = Some(failure);
        }
    }
}

fn applied(
    result: TxnResult<RenameContext>,
    vault: &Path,
    report: &mut RenameReport,
) {
    let mut context = result.context;
    if let Some(warning) = result.post_sync_warning {
        context.warnings.push(warning);
    }
    if result.commit.is_some() {
        invalidate_drawer_catalog(vault);
    }
    report.context = context;
    report.commit = result.commit;
    report.sync_pre = Some(result.sync_pre);
    report.sync_post = Some(result.sync_post);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn archive_paths_are_counted_apart() {
        assert!(in_archive(Path::new("done/bob_done.md")));
        assert!(!in_archive(Path::new("projects/done/notes.md")));
    }

    #[test]
    fn retitling_names_merges() {
        let titles = vec!["REQUIREMENTS".to_string(), "NOTES".to_string()];
        assert_eq!(
            retitling(&titles, "", "NOTES"),
            "REQUIREMENTS + NOTES → NOTES (merged)"
        );
        assert_eq!(
            retitling(&titles[..1], "## ", "Notes"),
            "## REQUIREMENTS → ## Notes"
        );
    }

    #[test]
    fn commit_subject_names_the_rename() {
        let spec = RenameSpec::parse("requirements", "Notes").expect("spec");
        assert_eq!(
            commit_subject(&spec, 2),
            "bob task rename-drawer: REQUIREMENTS → Notes (2 notes)"
        );
    }
}
