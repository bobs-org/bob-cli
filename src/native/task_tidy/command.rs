//! CLI shell for `bob task tidy`: scope, the dry-run report, and the
//! one-commit apply.
//!
//! Scope is every open `#task` block in every vault note except archive
//! history (`done/`), generated notes, templates, and the other
//! always-excluded directories. ROUTE narrows it to named notes. A bare run
//! is a dry run: it prints per-task before/after excerpts and takes no lock,
//! writes nothing, runs no Git, and does no sync. `-a` follows the
//! dry-run-first maintenance lifecycle `bob albums tidy` uses: the shared
//! lock, pre-sync, a fresh plan, a guarded write of every note by its full
//! preimage, a verify pass that replans nothing, one commit, and post-sync.

use std::{
    cell::RefCell,
    collections::BTreeSet,
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

use super::{content_preserved, settled, tidy_note, TaskRepair};
use crate::native::{
    capture_dependency_tasks::vault_note_paths,
    env as bob_env,
    host_catalog::{
        self, CommitInfo, FileEdit, PlannedTxn, ReadDependencies, TxnResult,
    },
    note_tasks::{self, NoteTaskSettings},
    ob,
    style::Styler,
    vault_rewrite::TxnFailure,
};

const SCHEMA_VERSION: u32 = 1;
const COMMAND_NAME: &str = "bob task tidy";
/// Archive history lives under this top-level folder and is never tidied.
const ARCHIVE_DIR: &str = "done";
/// Context lines around each changed run in a human excerpt.
const EXCERPT_CONTEXT: usize = 3;

// ---------------------------------------------------------------------------
// Vault plan
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct NoteChange {
    path: String,
    repairs: Vec<TaskRepair>,
}

impl NoteChange {
    fn moves(&self) -> usize {
        self.repairs.iter().map(|repair| repair.moves).sum()
    }

    fn notes_filed(&self) -> usize {
        self.repairs.iter().map(|repair| repair.notes_filed).sum()
    }

    fn drawers_created(&self) -> usize {
        self.repairs
            .iter()
            .filter(|repair| repair.notes_created)
            .count()
    }
}

#[derive(Debug, Clone, Default)]
struct TidyContext {
    routes: Vec<String>,
    notes_scanned: usize,
    open_tasks_scanned: usize,
    changes: Vec<NoteChange>,
    warnings: Vec<String>,
    io_errors: Vec<String>,
}

impl TidyContext {
    fn tasks(&self) -> usize {
        self.changes.iter().map(|change| change.repairs.len()).sum()
    }

    fn moves(&self) -> usize {
        self.changes.iter().map(NoteChange::moves).sum()
    }

    fn notes_filed(&self) -> usize {
        self.changes.iter().map(NoteChange::notes_filed).sum()
    }

    fn drawers_created(&self) -> usize {
        self.changes.iter().map(NoteChange::drawers_created).sum()
    }
}

struct TidyPlan {
    context: TidyContext,
    edits: Vec<FileEdit>,
    dependencies: ReadDependencies,
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

/// Every note in scope, sorted, plus walk problems.
fn scope_notes(vault: &Path) -> (Vec<PathBuf>, Vec<String>) {
    let (paths, warnings) = vault_note_paths(vault);
    let notes = paths
        .into_iter()
        .filter(|relative| !in_archive(relative))
        .collect();
    (notes, warnings)
}

fn unknown_route(route: &str) -> TxnFailure {
    TxnFailure::new(
        "unknown_route",
        format!("no note matches route `{route}`"),
        Some("name a note by its capture route (e.g. `bob`) or vault-relative path"),
    )
}

fn route_out_of_scope(route: &str, path: &str) -> TxnFailure {
    TxnFailure::new(
        "route_out_of_scope",
        format!(
            "route `{route}` names {path}, which tidy never touches (archive, generated, and template notes are out of scope)"
        ),
        Some("tidy repairs open tasks in live notes only"),
    )
}

/// The vault-relative note a ROUTE names: a capture route (`bob`, `@bob`,
/// `eat/apple`) or a vault-relative path, with or without `.md`.
fn route_path(route: &str) -> PathBuf {
    let trimmed = route.trim().trim_start_matches('@');
    let trimmed = trimmed.trim_start_matches("./");
    let stem = trimmed.strip_suffix(".md").unwrap_or(trimmed);
    PathBuf::from(format!("{stem}.md"))
}

/// Narrow the scope to the notes ROUTE names, in scope order. Unknown and
/// out-of-scope routes refuse before anything is planned.
fn select_notes(
    vault: &Path,
    notes: Vec<PathBuf>,
    routes: &[String],
) -> Result<Vec<PathBuf>, TxnFailure> {
    if routes.is_empty() {
        return Ok(notes);
    }
    let in_scope: BTreeSet<&PathBuf> = notes.iter().collect();
    let mut wanted = BTreeSet::new();
    for route in routes {
        let relative = route_path(route);
        if in_scope.contains(&relative) {
            wanted.insert(relative);
        } else if vault.join(&relative).is_file() {
            return Err(route_out_of_scope(route, &slash(&relative)));
        } else {
            return Err(unknown_route(route));
        }
    }
    Ok(notes
        .into_iter()
        .filter(|relative| wanted.contains(relative))
        .collect())
}

fn plan_vault(
    vault: &Path,
    routes: &[String],
    settings: &NoteTaskSettings,
) -> Result<TidyPlan, TxnFailure> {
    let (notes, walk_errors) = scope_notes(vault);
    let selected = select_notes(vault, notes, routes)?;
    let mut plan = TidyPlan {
        context: TidyContext {
            routes: routes.to_vec(),
            io_errors: walk_errors,
            ..TidyContext::default()
        },
        edits: Vec::new(),
        dependencies: ReadDependencies::default(),
    };
    for relative in selected {
        let path = slash(&relative);
        let contents = match fs::read_to_string(vault.join(&relative)) {
            Ok(contents) => contents,
            Err(error) => {
                plan.context
                    .io_errors
                    .push(format!("{path}: failed to read file: {error}"));
                continue;
            }
        };
        plan.context.notes_scanned += 1;
        let pass = tidy_note(&contents, settings);
        plan.context.open_tasks_scanned += pass.open_tasks;
        let Some(after) = pass.updated.clone() else {
            continue;
        };
        // Defensive: a planned write must keep every line and be settled.
        if let Err(reason) =
            content_preserved(&contents, &after, pass.drawers_created())
                .and_then(|()| settled(&after, settings))
        {
            plan.context.warnings.push(format!(
                "{path}: the planned repair is unsafe ({reason}); left unchanged"
            ));
            continue;
        }
        plan.context.changes.push(NoteChange {
            path,
            repairs: pass.repairs,
        });
        plan.edits.push(FileEdit {
            path: relative.clone(),
            before: contents.clone(),
            after,
        });
        plan.dependencies.insert(relative, Some(contents));
    }
    Ok(plan)
}

fn planned_txn(
    vault: &Path,
    plan: TidyPlan,
    settings: NoteTaskSettings,
) -> PlannedTxn<TidyContext> {
    let expected: Vec<(PathBuf, String)> = plan
        .edits
        .iter()
        .map(|edit| (edit.path.clone(), edit.after.clone()))
        .collect();
    let files = host_catalog::staged_rewrites(vault, &plan.edits);
    let verify_vault = vault.to_path_buf();
    let context = plan.context;
    let subject = format!(
        "{COMMAND_NAME} ({} in {})",
        counted(context.tasks(), "task", "tasks"),
        counted(context.changes.len(), "note", "notes"),
    );
    PlannedTxn {
        files,
        appends: Vec::new(),
        deps: plan.dependencies,
        subject,
        verify: Box::new(move || {
            verify_tidied(&verify_vault, &expected, &settings)
        }),
        context,
        kind: "task",
    }
}

/// Re-read every written note: it must hold the planned bytes and plan no
/// further repair.
fn verify_tidied(
    vault: &Path,
    expected: &[(PathBuf, String)],
    settings: &NoteTaskSettings,
) -> Result<(), String> {
    for (relative, after) in expected {
        let contents =
            fs::read_to_string(vault.join(relative)).map_err(|error| {
                format!("could not re-read {}: {error}", slash(relative))
            })?;
        if contents != *after {
            return Err(format!(
                "{} does not hold the planned repair",
                slash(relative)
            ));
        }
        settled(&contents, settings).map_err(|reason| {
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
        if self.failure.as_ref().is_some_and(|failure| {
            matches!(failure.code, "unknown_route" | "route_out_of_scope")
        }) {
            return 2;
        }
        i32::from(!self.ok())
    }

    fn to_json(&self) -> Value {
        let context = &self.context;
        let mut payload = json!({
            "schema_version": SCHEMA_VERSION,
            "ok": self.ok(),
            "command": "tidy",
            "dry_run": self.dry_run,
            "routes": context.routes,
            "counts": {
                "notes_scanned": context.notes_scanned,
                "open_tasks_scanned": context.open_tasks_scanned,
                "notes_changed": context.changes.len(),
                "tasks_changed": context.tasks(),
                "moves": context.moves(),
                "notes_filed": context.notes_filed(),
                "notes_drawers_created": context.drawers_created(),
            },
            "changes": context.changes.iter().map(|change| json!({
                "path": change.path,
                "moves": change.moves(),
                "notes_filed": change.notes_filed(),
                "tasks": change.repairs.iter().map(|repair| json!({
                    "line": repair.line_index + 1,
                    "text": repair.text,
                    "block_id": repair.block_id,
                    "moves": repair.moves,
                    "notes_filed": repair.notes_filed,
                    "notes_created": repair.notes_created,
                    "before": repair.before,
                    "after": repair.after,
                })).collect::<Vec<_>>(),
            })).collect::<Vec<_>>(),
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

    /// `{t} tasks in {n} notes · {m} moves · {s} notes filed under NOTES`.
    fn summary(&self) -> String {
        let context = &self.context;
        format!(
            "{} in {} · {} · {} filed under NOTES",
            counted(context.tasks(), "task", "tasks"),
            counted(context.changes.len(), "note", "notes"),
            counted(context.moves(), "move", "moves"),
            counted(context.notes_filed(), "note", "notes"),
        )
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
                print_note_excerpts(change, &styler);
            }
            if !context.changes.is_empty() {
                println!(
                    "{} dry run: {}; nothing was written. Rerun with -a to apply",
                    styler.yellow("!"),
                    self.summary(),
                );
            } else if self.ok() {
                self.print_clean(&styler);
            }
        } else if self.failure.is_none() {
            match &self.commit {
                Some(commit) => {
                    println!(
                        "{} tidied {} ({})",
                        styler.green("✓"),
                        self.summary(),
                        styler.dim(&commit.sha[..commit.sha.len().min(9)]),
                    );
                    for change in &context.changes {
                        println!(
                            "  {} {}",
                            styler.bold(&change.path),
                            styler.dim(&note_counts(change)),
                        );
                    }
                }
                None if context.io_errors.is_empty() => {
                    self.print_clean(&styler);
                }
                None => {}
            }
        }
        for warning in &context.warnings {
            println!("{} {warning}", styler.yellow("!"));
        }
    }

    fn print_clean(&self, styler: &Styler) {
        println!(
            "{} task blocks are tidy: {} in {} scanned, 0 tidied",
            styler.green("✓"),
            counted(self.context.open_tasks_scanned, "open task", "open tasks"),
            counted(self.context.notes_scanned, "note", "notes"),
        );
    }
}

/// `"1 task"` / `"2 tasks"`.
fn counted(count: usize, one: &str, many: &str) -> String {
    format!("{count} {}", if count == 1 { one } else { many })
}

/// `"(2 tasks · 3 moves · 1 note filed)"` for one changed note.
fn note_counts(change: &NoteChange) -> String {
    format!(
        "({} · {} · {} filed)",
        counted(change.repairs.len(), "task", "tasks"),
        counted(change.moves(), "move", "moves"),
        counted(change.notes_filed(), "note", "notes"),
    )
}

/// `"1 move · 2 notes filed under NOTES (new)"` for one repaired task.
fn repair_counts(repair: &TaskRepair) -> String {
    let mut parts = Vec::new();
    if repair.moves > 0 {
        parts.push(counted(repair.moves, "move", "moves"));
    }
    if repair.notes_filed > 0 {
        parts.push(format!(
            "{} filed under {}NOTES",
            counted(repair.notes_filed, "note", "notes"),
            if repair.notes_created { "new " } else { "" },
        ));
    }
    parts.join(" · ")
}

/// One note's header and each repaired task's colored before/after excerpt.
fn print_note_excerpts(change: &NoteChange, styler: &Styler) {
    println!(
        "{} {}",
        styler.bold(&change.path),
        styler.dim(&note_counts(change))
    );
    for repair in &change.repairs {
        println!(
            "  {} {} {}",
            styler.cyan(&format!("L{}", repair.line_index + 1)),
            repair.text,
            styler.dim(&format!("· {}", repair_counts(repair))),
        );
        print!("{}", excerpt_diff(&repair.before, &repair.after, styler));
    }
    println!();
}

/// A line diff of one task block, with a few context lines around each
/// changed run and `⋯` between distant runs.
fn excerpt_diff(
    before: &[String],
    after: &[String],
    styler: &Styler,
) -> String {
    let old = lines_text(before);
    let new = lines_text(after);
    let diff = TextDiff::from_lines(&old, &new);
    let mut out = String::new();
    for (index, group) in diff.grouped_ops(EXCERPT_CONTEXT).iter().enumerate() {
        if index > 0 {
            out.push_str(&format!("    {}\n", styler.dim("⋯")));
        }
        for op in group {
            for change in diff.iter_changes(op) {
                let text = change.value().trim_end_matches('\n');
                let line = match change.tag() {
                    ChangeTag::Delete => styler.red(&format!("    - {text}")),
                    ChangeTag::Insert => styler.green(&format!("    + {text}")),
                    ChangeTag::Equal => styler.dim(&format!("      {text}")),
                };
                out.push_str(&line);
                out.push('\n');
            }
        }
    }
    out
}

fn lines_text(lines: &[String]) -> String {
    let mut text = lines.join("\n");
    text.push('\n');
    text
}

// ---------------------------------------------------------------------------
// CLI
// ---------------------------------------------------------------------------

pub(crate) fn build_cli() -> ClapCommand {
    ClapCommand::new(COMMAND_NAME)
        .about("Repair open task blocks into canonical child order (dry run by default)")
        .long_about(
            "Repair open task blocks into the canonical child order: Cancel Log, managed links (DEPENDS ON, BLOCKS, SUB-PROJECTS, LINKS, LINKED FROM), requirements, loose notes and nested #task subtasks, drawers (NOTES first), then the Schedule, Work, and Rotten Logs. Each child moves with its whole subtree, and order inside a zone stays as authored. A task that owns a requirement keeps its loose notes in a NOTES drawer: tidy files them there, creating NOTES first among the drawers only when a note moves. A task with no requirements keeps its notes and drawers interleaved as authored. Only open tasks change; done and cancelled tasks and everything under done/ are history and are never rewritten, and _generated/ and _templates/ are skipped. ROUTE narrows the scope to named notes: a capture route such as bob or eat/apple, or a vault-relative path. Without -a the run is a dry run: it prints each repaired task's before/after excerpt grouped by note and takes no lock, writes nothing, runs no Git, and does no sync. -a takes the shared vault lock, pre-syncs, replans, guards each note by its full preimage, verifies that every rewritten note plans no further repair, makes one scoped commit, and post-syncs only when it committed. A second run reports nothing.",
        )
        .after_help(
            "Examples:\n  bob task tidy                  Preview every repair in the vault\n  bob task tidy bob              Preview repairs in bob.md only\n  bob task tidy -f json          Print the plan as JSON\n  bob task tidy -a               Apply every repair in one commit\n\nSee docs/task-requirements.md §3.4 for the tidy contract.",
        )
        .disable_help_flag(true)
        .arg(
            Arg::new("route")
                .value_name("ROUTE")
                .num_args(0..)
                .action(ArgAction::Append)
                .help("Tidy only these notes: a capture route or vault-relative path"),
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
    let routes: Vec<String> = matches
        .get_many::<String>("route")
        .map(|values| values.cloned().collect())
        .unwrap_or_default();
    let report = if matches.get_flag("apply") {
        applied_report(&vault, &routes, json_mode)
    } else {
        dry_run_report(&vault, &routes)
    };
    if json_mode {
        println!("{}", report.to_json());
    } else {
        report.print_human();
    }
    report.exit_code()
}

/// Read-only dry run: no lock, no Git, no vault-sync.
fn dry_run_report(vault: &Path, routes: &[String]) -> TidyReport {
    let settings = note_tasks::read_settings(vault);
    match plan_vault(vault, routes, &settings) {
        Ok(plan) => TidyReport {
            dry_run: true,
            context: plan.context,
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
    routes: &[String],
    json_mode: bool,
) -> TidyReport {
    // Usage errors precede the lock lifecycle so an unknown ROUTE exits 2
    // even when vault-sync would fail.
    if !routes.is_empty() {
        let (notes, _) = scope_notes(vault);
        if let Err(failure) = select_notes(vault, notes, routes) {
            return TidyReport::failed(false, failure, None);
        }
    }
    let child_env = ob::child_env();
    let captured: RefCell<Option<TidyContext>> = RefCell::new(None);
    let result =
        host_catalog::run_locked(vault, &child_env, json_mode, "task", || {
            let settings = note_tasks::read_settings(vault);
            let plan = plan_vault(vault, routes, &settings)?;
            *captured.borrow_mut() = Some(plan.context.clone());
            Ok(planned_txn(vault, plan, settings))
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

    #[test]
    fn routes_resolve_to_vault_relative_notes() {
        assert_eq!(route_path("bob"), PathBuf::from("bob.md"));
        assert_eq!(route_path("@bob"), PathBuf::from("bob.md"));
        assert_eq!(route_path("bob.md"), PathBuf::from("bob.md"));
        assert_eq!(route_path("eat/apple"), PathBuf::from("eat/apple.md"));
        assert_eq!(route_path("./eat/apple.md"), PathBuf::from("eat/apple.md"));
    }

    #[test]
    fn archive_paths_are_out_of_scope() {
        assert!(in_archive(Path::new("done/bob_done.md")));
        assert!(in_archive(Path::new("done/work/dev_done.md")));
        assert!(!in_archive(Path::new("done.md")));
        assert!(!in_archive(Path::new("projects/done/notes.md")));
    }

    #[test]
    fn repair_counts_name_moves_and_filed_notes() {
        let repair = TaskRepair {
            line_index: 0,
            text: "Ship".into(),
            block_id: None,
            moves: 1,
            notes_filed: 2,
            notes_created: true,
            before: Vec::new(),
            after: Vec::new(),
        };
        assert_eq!(
            repair_counts(&repair),
            "1 move · 2 notes filed under new NOTES"
        );
    }
}
