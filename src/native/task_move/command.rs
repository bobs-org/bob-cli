//! Live command adapter for the snapshot-only task-move planner.

use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::OsString,
    iter,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use clap::{Arg, ArgAction, ArgGroup, ArgMatches, Command as ClapCommand};
use serde_json::{json, Value};

use crate::native::{
    capture_pomodoros::{self},
    collect_done::link_repair_markdown_files,
    config::{self},
    env as bob_env,
    ob::ChildEnv,
    pomodoro,
    pomodoro_homes::{DiskHomeSource, HomeResolver},
    pomodoro_themes::{self, Catalog},
    style::Styler,
    task_command_shell::{self, TaskCommandShell},
    task_status_hooks_write::{
        apply_plan, capture_required, planned_write, ApplyOutcome,
        ApplySession, CaptureError, InputKind, InputSnapshot, ReasonCode,
        WritePlan,
    },
    task_theme::parse_task_theme,
    vault_links::NoteIndex,
    vault_sync::CycleReport,
};

use super::{
    plan_task_move, IdCollisionPolicy, MovePlacement, MovePlan, MoveRequest,
    MoveSelector, VaultSnapshot,
};

use crate::native::task_locator::TaskLocator;

const COMMAND_NAME: &str = "bob task move";
const DEFAULT_RETRY_TIMEOUT_SECS: u64 = 60;
const TOOL_NAME: &str = "task-move";

pub(crate) fn run(args: Vec<OsString>) -> i32 {
    let mut command = build_cli();
    let matches = match command.try_get_matches_from_mut(
        iter::once(OsString::from(COMMAND_NAME)).chain(args),
    ) {
        Ok(matches) => matches,
        Err(error) => return print_clap_error(error),
    };

    let dry_run = matches.get_flag("dry-run");
    let offline = matches.get_flag("offline");
    let retry_secs = matches
        .get_one::<u64>("retry-timeout")
        .copied()
        .unwrap_or(DEFAULT_RETRY_TIMEOUT_SECS);
    let format = OutputFormat::from_matches(&matches);
    let mut report = Report::new(format, dry_run, offline, retry_secs);
    if let Some(dest) = matches.get_one::<String>("to").cloned() {
        let tasks = many_tasks_from_matches(&matches);
        if tasks.is_empty() {
            eprintln!(
                "{COMMAND_NAME}: the following required arguments were not provided:\n  <TASK>..."
            );
            return 2;
        }
        report.many = true;
        report.many_dest = Some(dest.clone());
        let exit = execute_many(&mut report, &dest, &tasks);
        print_report(&report);
        return exit;
    }
    let source = matches.get_one::<String>("source").cloned();
    let dest = matches.get_one::<String>("dest").cloned();
    let extra: Vec<String> = matches
        .get_many::<String>("extra")
        .map(|values| values.cloned().collect())
        .unwrap_or_default();
    let (Some(source), Some(dest)) = (source, dest) else {
        eprintln!(
            "{COMMAND_NAME}: the following required arguments were not provided:\n  <SOURCE> <DEST>"
        );
        return 2;
    };
    if !extra.is_empty() {
        eprintln!(
            "{COMMAND_NAME}: unexpected argument '{}' found\n\nFor more information, try '--help'.",
            extra[0]
        );
        return 2;
    }
    let selectors = selectors_from_matches(&matches);
    if selectors.is_empty() {
        eprintln!(
            "{COMMAND_NAME}: the following required arguments were not provided:\n  (-b <ID> | -m <TEXT>)..."
        );
        return 2;
    }
    let request = MoveRequest {
        source,
        dest,
        selectors,
        allow_closed: false,
        placement: MovePlacement::Append,
        id_collision: IdCollisionPolicy::Rename,
    };
    let exit = execute(&mut report, &request);
    print_report(&report);
    exit
}

fn many_tasks_from_matches(matches: &ArgMatches) -> Vec<String> {
    let mut tasks = Vec::new();
    if let Some(source) = matches.get_one::<String>("source") {
        tasks.push(source.clone());
    }
    if let Some(dest) = matches.get_one::<String>("dest") {
        tasks.push(dest.clone());
    }
    if let Some(extra) = matches.get_many::<String>("extra") {
        tasks.extend(extra.cloned());
    }
    tasks
}

fn selectors_from_matches(matches: &ArgMatches) -> Vec<MoveSelector> {
    let mut selectors = Vec::new();
    if let Some(blocks) = matches.get_many::<String>("block") {
        selectors.extend(blocks.cloned().map(MoveSelector::Block));
    }
    if let Some(matches) = matches.get_many::<String>("match") {
        selectors.extend(matches.cloned().map(MoveSelector::Match));
    }
    selectors
}

fn print_clap_error(error: clap::Error) -> i32 {
    let code = error.exit_code();
    if let Err(print_error) = error.print() {
        // A closed pipe renders nothing: stay quiet instead of
        // reporting the failed render, but keep clap's own exit
        // code so a usage error never reports success.
        if print_error.kind() != std::io::ErrorKind::BrokenPipe {
            eprintln!(
                "{COMMAND_NAME}: failed to print command-line error: {print_error}"
            );
        }
    }
    code
}

pub(crate) fn build_cli() -> ClapCommand {
    ClapCommand::new(COMMAND_NAME)
        .about("Move open tasks between notes and repair vault links")
        .long_about("Move selected open task blocks to another eligible note, preserving task state while repairing block links and dependency identities throughout the vault.\n\nA live run holds the shared maintenance lock, syncs before and after the move, and commits exactly the changed Markdown files. Use --dry-run to inspect the same plan without locking, syncing, probing Git, or writing. Use --offline to skip both sync cycles while keeping the lock and scoped local commit.")
        .after_help("Examples:\n  bob task move --dry-run old-note new-note -b work-item\n                                 Preview a move selected by block id\n  bob task move old-note new-note -m 'Prepare release'\n                                 Move a task selected by literal line text\n  bob task move -t db inbox:bench work:pgdump\n                                 Move many tasks into one note\n  bob task move -d -f json old-note new-note -b work-item\n                                 Print a machine-readable preview\n\nRun 'bob task reconcile' after a move when the report says\nreconciliation is pending.")
        .override_usage("bob task move [OPTIONS] <SOURCE> <DEST> (-b ID | -m TEXT)...\n       bob task move [OPTIONS] -t <NOTE> <TASK>...")
        .disable_help_flag(true)
        .arg(Arg::new("source").value_name("SOURCE").required(false).index(1).help("Source note stem or vault-relative path"))
        .arg(Arg::new("dest").value_name("DEST").required(false).index(2).help("Destination note stem or vault-relative path"))
        .arg(Arg::new("extra").value_name("TASK").required(false).num_args(1..).index(3).help("Additional tasks for --to mode"))
        .arg(Arg::new("block").long("block").short('b').value_name("ID").action(ArgAction::Append).conflicts_with("to").help("Select a task by trailing block id without ^; repeatable"))
        .arg(Arg::new("dry-run").long("dry-run").short('d').action(ArgAction::SetTrue).help("Plan and print only; do not lock, sync, probe Git, or write"))
        .arg(Arg::new("format").long("format").short('f').value_name("FORMAT").value_parser(["human", "json"]).default_value("human").help("Output format: human or json"))
        .arg(Arg::new("help").long("help").short('h').action(ArgAction::Help).help("Show help"))
        .arg(Arg::new("match").long("match").short('m').value_name("TEXT").action(ArgAction::Append).conflicts_with("to").help("Select the task whose line contains this literal text; repeatable"))
        .arg(Arg::new("offline").long("offline").short('o').action(ArgAction::SetTrue).help("Skip both sync cycles; commit locally without pushing"))
        .arg(Arg::new("retry-timeout").long("retry-timeout").short('r').value_name("SECONDS").value_parser(clap::value_parser!(u64)).default_value("60").help("Lock wait and re-plan budget after concurrent edits; 0 fails fast"))
        .arg(Arg::new("to").long("to").short('t').value_name("NOTE").help("Many-task mode: move each TASK locator into NOTE"))
        .group(
            ArgGroup::new("selector")
                .args(["block", "match"])
                .required(false)
                .multiple(true),
        )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OutputFormat {
    Human,
    Json,
}

impl OutputFormat {
    fn from_matches(matches: &ArgMatches) -> Self {
        if matches.get_one::<String>("format").map(String::as_str)
            == Some("json")
        {
            Self::Json
        } else {
            Self::Human
        }
    }
}

#[derive(Debug, Clone)]
struct StageError {
    stage: &'static str,
    code: String,
    message: String,
    hint: Option<String>,
}

#[derive(Debug, Clone, Default)]
struct SyncSection {
    ok: bool,
    files_committed: usize,
    pushed: bool,
    conflicts: Vec<String>,
    error: Option<String>,
}

impl SyncSection {
    fn from_report(report: &CycleReport, pre_sync: bool) -> Self {
        Self {
            ok: report.ok,
            files_committed: if pre_sync { report.files_committed } else { 0 },
            pushed: !pre_sync && report.pushed,
            conflicts: report.conflicts.clone(),
            error: report.error.clone(),
        }
    }
}

#[derive(Debug, Clone, Default)]
struct CommitSection {
    sha: Option<String>,
    subject: String,
    paths: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GitMode {
    Sync,
    Offline,
    NotAWorktree,
    DryRun,
}

impl GitMode {
    fn as_str(self) -> &'static str {
        match self {
            Self::Sync => "sync",
            Self::Offline => "offline",
            Self::NotAWorktree => "not_a_worktree",
            Self::DryRun => "dry_run",
        }
    }
}

struct Report {
    format: OutputFormat,
    dry_run: bool,
    offline: bool,
    retry_secs: u64,
    vault: PathBuf,
    mode: GitMode,
    plan: Option<MovePlan>,
    warnings: Vec<String>,
    pre_sync: Option<SyncSection>,
    commit: Option<CommitSection>,
    post_sync: Option<SyncSection>,
    applied_paths: Vec<String>,
    deferred_paths: Vec<String>,
    recovery_directory: Option<String>,
    error: Option<StageError>,
    relocated: Vec<RelocatedInfo>,
    many: bool,
    many_dest: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
struct RelocatedInfo {
    route: String,
    block_id: String,
    from: Option<String>,
    to: String,
    #[serde(rename = "type")]
    type_name: String,
}

impl Report {
    fn new(
        format: OutputFormat,
        dry_run: bool,
        offline: bool,
        retry_secs: u64,
    ) -> Self {
        Self {
            format,
            dry_run,
            offline,
            retry_secs,
            vault: bob_env::bob_dir(),
            mode: if dry_run {
                GitMode::DryRun
            } else if offline {
                GitMode::Offline
            } else {
                GitMode::Sync
            },
            plan: None,
            warnings: Vec::new(),
            pre_sync: None,
            commit: None,
            post_sync: None,
            applied_paths: Vec::new(),
            deferred_paths: Vec::new(),
            recovery_directory: None,
            error: None,
            relocated: Vec::new(),
            many: false,
            many_dest: None,
        }
    }

    fn fail(
        &mut self,
        stage: &'static str,
        code: &str,
        message: impl Into<String>,
        hint: Option<&str>,
    ) {
        let message = message.into();
        if self.error.is_none() {
            self.error = Some(StageError {
                stage,
                code: code.to_string(),
                message,
                hint: hint.map(str::to_string),
            });
            return;
        }
        self.warn(format!("also failed during {stage}: {message}"));
    }

    fn warn(&mut self, warning: impl Into<String>) {
        let warning = warning.into();
        if self.format == OutputFormat::Json {
            eprintln!("warning: {warning}");
        }
        self.warnings.push(warning);
    }

    fn progress(&self, message: &str) {
        if self.format == OutputFormat::Json {
            eprintln!("{message}");
        }
    }
}

struct Scan {
    snapshot: VaultSnapshot,
    inputs: Vec<InputSnapshot>,
    scan_paths: Vec<PathBuf>,
}

#[derive(Debug)]
enum ScanError {
    Retryable(String),
    Fatal(String),
}

impl ScanError {
    fn message(self) -> String {
        match self {
            Self::Retryable(message) | Self::Fatal(message) => message,
        }
    }

    fn from_capture(relative: &str, error: CaptureError) -> Self {
        let message = format!("read note {relative}: {}", error.message());
        match error {
            CaptureError::Unstable(_) => Self::Retryable(message),
            _ => Self::Fatal(message),
        }
    }
}

fn execute(report: &mut Report, request: &MoveRequest) -> i32 {
    if report.dry_run {
        match plan_once(&report.vault, request) {
            Ok((scan, plan)) => {
                report.plan = Some(plan.clone());
                match compute_move_relocation(
                    &report.vault,
                    &scan.snapshot,
                    &plan,
                ) {
                    Ok((_, relocated)) => {
                        report.relocated = relocated;
                    }
                    Err(error) => {
                        report.fail(
                            "plan",
                            "move_relocation_failed",
                            error,
                            None,
                        );
                    }
                }
            }
            Err((code, error)) => report.fail("plan", &code, error, None),
        }
        return if report.error.is_some() { 1 } else { 0 };
    }

    let retry_timeout = Duration::from_secs(report.retry_secs);
    let shell = match TaskCommandShell::start(
        &report.vault,
        retry_timeout,
        report.offline,
        report.format == OutputFormat::Json,
    ) {
        Ok(shell) => shell,
        Err(error) => {
            report.fail("lock", "maintenance_lock", error.to_string(), Some("wait for the current maintenance run to finish, then retry"));
            return 1;
        }
    };
    let child_env = shell.child_env.clone();
    let worktree = shell.worktree;
    if let Some(warning) = shell.warning.as_deref() {
        report.mode = GitMode::NotAWorktree;
        report.warn(warning);
    } else if report.offline {
        report.mode = GitMode::Offline;
    } else {
        report.mode = GitMode::Sync;
    }

    if let Some(cycle) = &shell.pre_sync {
        let section = SyncSection::from_report(cycle, true);
        report.progress(&pre_sync_detail(&section));
        report.pre_sync = Some(section);
        if !cycle.ok {
            report.fail(
                "pre_sync",
                "vault_sync_failed",
                format!(
                    "pre-sync failed: {}",
                    cycle.error.clone().unwrap_or_else(|| {
                        "vault sync reported failure".to_string()
                    })
                ),
                Some("resolve the sync problem before retrying the move"),
            );
            return 1;
        }
    }
    if report.offline {
        report.progress("offline: skipped vault sync");
    }

    let (applied, partial) =
        match plan_and_apply(report, request, &retry_timeout) {
            Some(outcome) => outcome,
            None => return 1,
        };
    if applied.is_empty() {
        return if report.error.is_some() { 1 } else { 0 };
    }

    finish_after_writes(report, &applied, partial, worktree, &child_env)
}

fn execute_many(report: &mut Report, dest_raw: &str, tasks: &[String]) -> i32 {
    if TaskLocator::parse(dest_raw).is_some() {
        report.fail(
            "plan",
            "move_to_task",
            format!(
                "to fold tasks into {dest_raw}, use `bob task merge -i {dest_raw} …`"
            ),
            None,
        );
        return 1;
    }
    if report.dry_run {
        match plan_many_once(&report.vault, dest_raw, tasks) {
            Ok((scan, plan, dest_display)) => {
                report.many_dest = Some(dest_display);
                report.plan = Some(plan.clone());
                match compute_move_relocation(
                    &report.vault,
                    &scan.snapshot,
                    &plan,
                ) {
                    Ok((_, relocated)) => {
                        report.relocated = relocated;
                    }
                    Err(error) => {
                        report.fail(
                            "plan",
                            "move_relocation_failed",
                            error,
                            None,
                        );
                    }
                }
            }
            Err((code, error)) => report.fail("plan", &code, error, None),
        }
        return if report.error.is_some() { 1 } else { 0 };
    }

    let retry_timeout = Duration::from_secs(report.retry_secs);
    let shell = match TaskCommandShell::start(
        &report.vault,
        retry_timeout,
        report.offline,
        report.format == OutputFormat::Json,
    ) {
        Ok(shell) => shell,
        Err(error) => {
            report.fail(
                "lock",
                "maintenance_lock",
                error.to_string(),
                Some(
                    "wait for the current maintenance run to finish, then retry",
                ),
            );
            return 1;
        }
    };
    let child_env = shell.child_env.clone();
    let worktree = shell.worktree;
    if let Some(warning) = shell.warning.as_deref() {
        report.mode = GitMode::NotAWorktree;
        report.warn(warning);
    } else if report.offline {
        report.mode = GitMode::Offline;
    } else {
        report.mode = GitMode::Sync;
    }

    if let Some(cycle) = &shell.pre_sync {
        let section = SyncSection::from_report(cycle, true);
        report.progress(&pre_sync_detail(&section));
        report.pre_sync = Some(section);
        if !cycle.ok {
            report.fail(
                "pre_sync",
                "vault_sync_failed",
                format!(
                    "pre-sync failed: {}",
                    cycle.error.clone().unwrap_or_else(|| {
                        "vault sync reported failure".to_string()
                    })
                ),
                Some("resolve the sync problem before retrying the move"),
            );
            return 1;
        }
    }
    if report.offline {
        report.progress("offline: skipped vault sync");
    }

    let (applied, partial) =
        match plan_and_apply_many(report, dest_raw, tasks, &retry_timeout) {
            Some(outcome) => outcome,
            None => return 1,
        };
    if applied.is_empty() {
        return if report.error.is_some() { 1 } else { 0 };
    }

    finish_after_writes(report, &applied, partial, worktree, &child_env)
}

fn plan_many_once(
    vault: &Path,
    dest_raw: &str,
    tasks: &[String],
) -> Result<(Scan, MovePlan, String), (String, String)> {
    let scan = scan_vault(vault)
        .map_err(|error| ("move_scan_failed".to_string(), error.message()))?;
    let settings = crate::native::note_tasks::read_settings(vault);
    let (plan, dest_display) =
        plan_many_from_scan(&scan, dest_raw, tasks, &settings)?;
    Ok((scan, plan, dest_display))
}

fn plan_many_from_scan(
    scan: &Scan,
    dest_raw: &str,
    tasks: &[String],
    settings: &crate::native::note_tasks::NoteTaskSettings,
) -> Result<(MovePlan, String), (String, String)> {
    use crate::native::vault_links::LinkResolution;
    let mut staged = scan.snapshot.notes.clone();
    let note_index = NoteIndex::from_paths(scan.snapshot.notes.keys().cloned());
    let dest_path = match note_index.resolve_detailed(None, dest_raw) {
        LinkResolution::Found(path) => path,
        LinkResolution::Ambiguous => {
            return Err((
                "move_ambiguous_note".to_string(),
                format!("note {dest_raw:?} is ambiguous"),
            ));
        }
        LinkResolution::Missing => {
            return Err((
                "move_missing_note".to_string(),
                format!("note {dest_raw:?} was not found in the vault"),
            ));
        }
    };
    let dest_display = dest_path.display().to_string();
    let mut locators = Vec::with_capacity(tasks.len());
    for raw in tasks {
        let locator = TaskLocator::parse(raw).ok_or_else(|| {
            (
                "task_not_found".to_string(),
                format!(
                    "`{raw}` is not a task locator; use note:block-id or [[note#^block-id]]"
                ),
            )
        })?;
        locators.push(locator);
    }
    let mut seen: BTreeSet<(PathBuf, String)> = BTreeSet::new();
    let mut combined_postimages: BTreeMap<PathBuf, String> = BTreeMap::new();
    let mut combined_moves = Vec::new();
    let mut combined_renamed = Vec::new();
    let mut combined_repaired: BTreeMap<PathBuf, (usize, usize)> =
        BTreeMap::new();
    let mut needs_reconcile = false;
    for locator in &locators {
        let locator_text = format!("{}:{}", locator.note, locator.block_id);
        let located = crate::native::task_locator::resolve_task(
            &staged,
            &locator_text,
            settings,
        )
        .map_err(|error| (error.code.to_string(), error.message))?;
        if located.path == dest_path {
            return Err((
                "move_same_note".to_string(),
                format!(
                    "task ^{} is already in {}",
                    locator.block_id,
                    dest_path.display()
                ),
            ));
        }
        let key = (
            located.path.clone(),
            located.task.block_id.clone().unwrap_or_default(),
        );
        if !seen.insert(key) {
            continue;
        }
        let request = MoveRequest {
            source: located.path.to_string_lossy().into_owned(),
            dest: dest_path.to_string_lossy().into_owned(),
            selectors: vec![MoveSelector::Block(locator.block_id.clone())],
            allow_closed: false,
            placement: MovePlacement::Append,
            id_collision: IdCollisionPolicy::Rename,
        };
        let staged_snapshot = VaultSnapshot::from_notes(
            staged
                .iter()
                .map(|(path, text)| (path.clone(), text.clone())),
        );
        let plan = plan_task_move(&staged_snapshot, &request)
            .map_err(|error| (error.code().to_string(), error.to_string()))?;
        for (path, contents) in &plan.postimages {
            staged.insert(path.clone(), contents.clone());
            combined_postimages.insert(path.clone(), contents.clone());
        }
        combined_moves.extend(plan.moves);
        combined_renamed.extend(plan.renamed_block_ids);
        for file in &plan.repaired_files {
            let entry =
                combined_repaired.entry(file.path.clone()).or_insert((0, 0));
            entry.0 += file.link_count;
            entry.1 += file.dependency_token_count;
        }
        needs_reconcile |= plan.needs_reconcile;
    }
    if combined_moves.is_empty() {
        return Err((
            "move_selector_zero".to_string(),
            "no movable tasks selected".to_string(),
        ));
    }
    let repaired_files = combined_repaired
        .into_iter()
        .map(|(path, (link_count, dependency_token_count))| {
            crate::native::task_move::transplant::RepairedFile {
                path,
                link_count,
                dependency_token_count,
            }
        })
        .collect();
    Ok((
        MovePlan {
            postimages: combined_postimages,
            moves: combined_moves,
            renamed_block_ids: combined_renamed,
            repaired_files,
            needs_reconcile,
        },
        dest_display,
    ))
}

fn plan_and_apply_many(
    report: &mut Report,
    dest_raw: &str,
    tasks: &[String],
    retry_timeout: &Duration,
) -> Option<(Vec<PathBuf>, bool)> {
    let started = Instant::now();
    loop {
        let scan = match scan_vault(&report.vault) {
            Ok(scan) => scan,
            Err(ScanError::Retryable(message))
                if started.elapsed() < *retry_timeout =>
            {
                report.progress(
                    "vault changed during scan; rescanning and retrying",
                );
                let _ = message;
                continue;
            }
            Err(error) => {
                report.fail("plan", "move_scan_failed", error.message(), None);
                return None;
            }
        };
        let settings = crate::native::note_tasks::read_settings(&report.vault);
        let mut plan =
            match plan_many_from_scan(&scan, dest_raw, tasks, &settings) {
                Ok((plan, dest_display)) => {
                    report.many_dest = Some(dest_display);
                    plan
                }
                Err((code, error)) => {
                    report.fail("plan", &code, error, None);
                    return None;
                }
            };
        match compute_move_relocation(&report.vault, &scan.snapshot, &plan) {
            Ok((daily_edit, relocated)) => {
                report.relocated = relocated;
                if let Some((rel_path, new_contents)) = daily_edit {
                    plan.postimages.insert(rel_path, new_contents);
                }
            }
            Err(error) => {
                report.fail("plan", "move_relocation_failed", error, None);
                return None;
            }
        }
        report.plan = Some(plan.clone());
        let write_plan = match build_write_plan(report, &scan, &plan) {
            Ok(write_plan) => write_plan,
            Err(error) => {
                report.fail("apply", "write_plan_failed", error, None);
                return None;
            }
        };
        let vault = report.vault.clone();
        let rescan_vault = vault.clone();
        let mut session = ApplySession::production(Box::new(move || {
            link_repair_markdown_files(&rescan_vault)
        }));
        session.tool = TOOL_NAME;
        match apply_plan(&write_plan, &session) {
            Ok(ApplyOutcome::NoOp) => return Some((Vec::new(), false)),
            Ok(ApplyOutcome::Applied {
                applied_files,
                recovery_directory,
            }) => {
                report.applied_paths = display_paths(&vault, &applied_files);
                report.recovery_directory =
                    Some(recovery_directory.display().to_string());
                report.progress(&format!(
                    "applied {} note(s)",
                    applied_files.len()
                ));
                return Some((applied_files, false));
            }
            Err(error) => {
                let retryable = matches!(
                    error.reason,
                    ReasonCode::VaultChanged
                        | ReasonCode::QuietPeriod
                        | ReasonCode::UnstableRead
                ) && error.applied_files.is_empty();
                if retryable && started.elapsed() < *retry_timeout {
                    report.progress("vault changed during planning; rescanning and retrying");
                    continue;
                }
                report.applied_paths =
                    display_paths(&vault, &error.applied_files);
                report.deferred_paths =
                    display_paths(&vault, &error.deferred_files);
                if let Some(recovery) = &error.recovery_directory {
                    report.recovery_directory =
                        Some(recovery.display().to_string());
                }
                if error.applied_files.is_empty() {
                    report.fail("apply", "guarded_write_failed", error.message, Some("no move files were written; resolve the reported condition and retry"));
                    return None;
                }
                report.fail(
                    "apply",
                    "partial_write",
                    format!("{}; applied [{}], deferred [{}]", error.message, report.applied_paths.join(", "), report.deferred_paths.join(", ")),
                    Some("inspect the recovery directory and applied files before retrying; selectors are not replayed automatically"),
                );
                return Some((error.applied_files, true));
            }
        }
    }
}

fn finish_after_writes(
    report: &mut Report,
    applied: &[PathBuf],
    partial: bool,
    worktree: bool,
    child_env: &ChildEnv,
) -> i32 {
    if worktree {
        match commit_applied(report, applied, child_env) {
            Ok(commit) => report.commit = commit,
            Err(error) => report.fail(
                "commit",
                "git_commit_failed",
                format!("commit moved notes: {error}"),
                Some(
                    "the local files are written; resolve the Git failure before retrying",
                ),
            ),
        }
    }
    if worktree && !report.offline && (report.error.is_none() || partial) {
        let _ = post_sync(report, child_env);
    }
    if report.error.is_some() {
        1
    } else {
        0
    }
}

fn plan_once(
    vault: &Path,
    request: &MoveRequest,
) -> Result<(Scan, MovePlan), (String, String)> {
    let scan = scan_vault(vault)
        .map_err(|error| ("move_scan_failed".to_string(), error.message()))?;
    plan_from_scan(&scan, request).map(|plan| (scan, plan))
}

fn plan_from_scan(
    scan: &Scan,
    request: &MoveRequest,
) -> Result<MovePlan, (String, String)> {
    plan_task_move(&scan.snapshot, request)
        .map_err(|error| (error.code().to_string(), error.to_string()))
}

fn scan_vault(vault: &Path) -> Result<Scan, ScanError> {
    let files = link_repair_markdown_files(vault).map_err(|error| {
        ScanError::Fatal(format!("scan vault {}: {error}", vault.display()))
    })?;
    let mut notes = Vec::with_capacity(files.len());
    let mut inputs = Vec::with_capacity(files.len());
    let mut scan_paths = Vec::with_capacity(files.len());
    for path in files {
        let relative = relative_display(vault, &path);
        let input = match capture_required(&path, InputKind::Note) {
            Ok(input) => input,
            Err(error) => {
                return Err(ScanError::from_capture(&relative, error))
            }
        };
        let contents = match input.utf8_contents() {
            Ok(Some(contents)) => contents,
            Ok(None) => {
                return Err(ScanError::Fatal(format!(
                    "read note {relative}: file does not exist"
                )));
            }
            Err(error) => {
                return Err(ScanError::from_capture(&relative, error));
            }
        };
        let relative_path = path.strip_prefix(vault).map_err(|error| {
            ScanError::Fatal(format!(
                "note {} is outside vault {}: {error}",
                path.display(),
                vault.display()
            ))
        })?;
        notes.push((relative_path.to_path_buf(), contents));
        scan_paths.push(path.clone());
        inputs.push(input);
    }
    Ok(Scan {
        snapshot: VaultSnapshot::from_notes(notes),
        inputs,
        scan_paths,
    })
}

fn plan_and_apply(
    report: &mut Report,
    request: &MoveRequest,
    retry_timeout: &Duration,
) -> Option<(Vec<PathBuf>, bool)> {
    let mut session_factory = |vault: &Path| {
        let rescan_vault = vault.to_path_buf();
        ApplySession::production(Box::new(move || {
            link_repair_markdown_files(&rescan_vault)
        }))
    };
    plan_and_apply_with_hooks(
        report,
        request,
        retry_timeout,
        &mut session_factory,
        &mut scan_vault,
    )
}

#[cfg(test)]
fn plan_and_apply_with_session_factory(
    report: &mut Report,
    request: &MoveRequest,
    retry_timeout: &Duration,
    session_factory: &mut impl FnMut(&Path) -> ApplySession,
) -> Option<(Vec<PathBuf>, bool)> {
    plan_and_apply_with_hooks(
        report,
        request,
        retry_timeout,
        session_factory,
        &mut scan_vault,
    )
}

fn plan_and_apply_with_hooks(
    report: &mut Report,
    request: &MoveRequest,
    retry_timeout: &Duration,
    session_factory: &mut impl FnMut(&Path) -> ApplySession,
    scan_factory: &mut impl FnMut(&Path) -> Result<Scan, ScanError>,
) -> Option<(Vec<PathBuf>, bool)> {
    let started = Instant::now();
    loop {
        let scan = match scan_factory(&report.vault) {
            Ok(scan) => scan,
            Err(ScanError::Retryable(message))
                if started.elapsed() < *retry_timeout =>
            {
                report.progress(
                    "vault changed during scan; rescanning and retrying",
                );
                let _ = message;
                continue;
            }
            Err(error) => {
                report.fail("plan", "move_scan_failed", error.message(), None);
                return None;
            }
        };
        let mut plan = match plan_from_scan(&scan, request) {
            Ok(plan) => plan,
            Err((code, error)) => {
                report.fail("plan", &code, error, None);
                return None;
            }
        };
        match compute_move_relocation(&report.vault, &scan.snapshot, &plan) {
            Ok((daily_edit, relocated)) => {
                report.relocated = relocated;
                if let Some((rel_path, new_contents)) = daily_edit {
                    plan.postimages.insert(rel_path, new_contents);
                }
            }
            Err(error) => {
                report.fail("plan", "move_relocation_failed", error, None);
                return None;
            }
        }
        report.plan = Some(plan.clone());
        let write_plan = match build_write_plan(report, &scan, &plan) {
            Ok(write_plan) => write_plan,
            Err(error) => {
                report.fail("apply", "write_plan_failed", error, None);
                return None;
            }
        };
        let vault = report.vault.clone();
        let mut session = session_factory(&vault);
        session.tool = TOOL_NAME;
        match apply_plan(&write_plan, &session) {
            Ok(ApplyOutcome::NoOp) => return Some((Vec::new(), false)),
            Ok(ApplyOutcome::Applied {
                applied_files,
                recovery_directory,
            }) => {
                report.applied_paths = display_paths(&vault, &applied_files);
                report.recovery_directory =
                    Some(recovery_directory.display().to_string());
                report.progress(&format!(
                    "applied {} note(s)",
                    applied_files.len()
                ));
                return Some((applied_files, false));
            }
            Err(error) => {
                let retryable = matches!(
                    error.reason,
                    ReasonCode::VaultChanged
                        | ReasonCode::QuietPeriod
                        | ReasonCode::UnstableRead
                ) && error.applied_files.is_empty();
                if retryable && started.elapsed() < *retry_timeout {
                    report.progress("vault changed during planning; rescanning and retrying");
                    continue;
                }
                report.applied_paths =
                    display_paths(&vault, &error.applied_files);
                report.deferred_paths =
                    display_paths(&vault, &error.deferred_files);
                if let Some(recovery) = &error.recovery_directory {
                    report.recovery_directory =
                        Some(recovery.display().to_string());
                }
                if error.applied_files.is_empty() {
                    report.fail("apply", "guarded_write_failed", error.message, Some("no move files were written; resolve the reported condition and retry"));
                    return None;
                }
                report.fail(
                    "apply",
                    "partial_write",
                    format!("{}; applied [{}], deferred [{}]", error.message, report.applied_paths.join(", "), report.deferred_paths.join(", ")),
                    Some("inspect the recovery directory and applied files before retrying; selectors are not replayed automatically"),
                );
                return Some((error.applied_files, true));
            }
        }
    }
}

fn compute_move_relocation(
    vault: &Path,
    snapshot: &VaultSnapshot,
    plan: &MovePlan,
) -> Result<(Option<(PathBuf, String)>, Vec<RelocatedInfo>), String> {
    if plan.moves.is_empty() {
        return Ok((None, Vec::new()));
    }
    let day_abs = pomodoro::day_file_for(vault);
    let Ok(day_rel) = day_abs.strip_prefix(vault).map(Path::to_path_buf) else {
        return Ok((None, Vec::new()));
    };
    let mut day_contents = if let Some(post) = plan.postimages.get(&day_rel) {
        post.clone()
    } else if let Some(original) = snapshot.notes.get(&day_rel) {
        original.clone()
    } else {
        match std::fs::read_to_string(&day_abs) {
            Ok(contents) => contents,
            Err(_) => return Ok((None, Vec::new())),
        }
    };
    // No Pomodoros section means no live links.
    if !capture_pomodoros::scan(&day_contents).has_section {
        return Ok((None, Vec::new()));
    }
    let today = super::super::capture_pomodoros_agenda::agenda_date(&day_abs);
    let catalog = match Catalog::load(vault, today, false) {
        Ok(catalog) => catalog,
        Err(_) => return Ok((None, Vec::new())),
    };
    let cfg = match config::load_plan_config(&config::config_path()) {
        Ok(cfg) => cfg,
        Err(_) => return Ok((None, Vec::new())),
    };
    let mut resolver = HomeResolver::new(DiskHomeSource::new(vault));
    let sources = catalog.theme_sources();
    let mut relocated = Vec::new();
    let mut changed = false;
    for mv in &plan.moves {
        let Some(source_text) = snapshot.notes.get(&mv.source_path) else {
            continue;
        };
        let Some(task_line) = source_text
            .lines()
            .nth(mv.source_line.saturating_sub(1))
            .map(str::to_string)
        else {
            continue;
        };
        let field = parse_task_theme(&task_line);
        if !matches!(field, crate::native::task_theme::TaskThemeState::None) {
            continue;
        }
        let before = pomodoro_themes::suggest::suggest_for_task(
            &mut resolver,
            &catalog,
            &cfg,
            &mv.source_path,
            &task_line,
        );
        let after = pomodoro_themes::suggest::suggest_for_task(
            &mut resolver,
            &catalog,
            &cfg,
            &mv.dest_path,
            &task_line,
        );
        let before_key =
            before.suggestion.as_ref().map(|s| s.theme.key.clone());
        let after_suggestion = match after.suggestion {
            Some(suggestion) => suggestion,
            None => continue,
        };
        let after_key = after_suggestion.theme.key.clone();
        let after_name = after_suggestion.theme.name.clone();
        if before_key.as_deref() == Some(after_key.as_str()) {
            continue;
        }
        let block_id =
            mv.new_block_id.clone().or_else(|| mv.old_block_id.clone());
        let Some(block_id) = block_id else {
            continue;
        };
        // Shared D8 applier keeps the live link agreeing with the moved
        // task's new type; capture `#<type>` retypes call the same helper.
        // Types are listed, never budgeted, so the applier never refuses.
        if let Some((updated, info)) =
            crate::native::pomodoro_themes::relocation_apply::apply_to_day_contents(
                &day_contents,
                &block_id,
                &after_key,
                &after_name,
                &sources,
            )
        {
            day_contents = updated;
            changed = true;
            relocated.push(RelocatedInfo {
                route: mv
                    .dest_path
                    .with_extension("")
                    .to_string_lossy()
                    .replace('\\', "/"),
                block_id: block_id.clone(),
                from: info.from,
                to: info.to,
                type_name: after_name.clone(),
            });
        }
    }
    if changed {
        Ok((Some((day_rel, day_contents)), relocated))
    } else {
        Ok((None, relocated))
    }
}

fn build_write_plan(
    report: &Report,
    scan: &Scan,
    plan: &MovePlan,
) -> Result<WritePlan, String> {
    let source = plan.moves.first().map(|item| item.source_path.as_path());
    let dest = plan.moves.first().map(|item| item.dest_path.as_path());
    let mut postimages: Vec<_> = plan.postimages.iter().collect();
    postimages.sort_by(|(left, _), (right, _)| {
        output_rank(left, source, dest)
            .cmp(&output_rank(right, source, dest))
            .then_with(|| left.cmp(right))
    });
    let mut outputs = Vec::with_capacity(postimages.len());
    for (relative, postimage) in postimages {
        let path = report.vault.join(relative);
        let original = scan
            .inputs
            .iter()
            .find(|input| input.path == path)
            .ok_or_else(|| {
                format!(
                    "{} was not in the planning read-set",
                    relative.display()
                )
            })?;
        let original_text = original
            .utf8_contents()
            .map_err(|error| error.message())?
            .ok_or_else(|| {
                format!("{} disappeared during planning", relative.display())
            })?;
        if &original_text == postimage {
            continue;
        }
        outputs.push(
            planned_write(path, original, postimage.as_bytes().to_vec(), true)
                .map_err(|error| error.message())?,
        );
    }
    Ok(WritePlan {
        vault_canonical: report
            .vault
            .canonicalize()
            .unwrap_or_else(|_| report.vault.clone()),
        inputs: scan.inputs.clone(),
        scan_paths: scan.scan_paths.clone(),
        outputs,
    })
}

fn output_rank(path: &Path, source: Option<&Path>, dest: Option<&Path>) -> u8 {
    if Some(path) == dest {
        0
    } else if Some(path) == source {
        2
    } else {
        1
    }
}

fn commit_applied(
    report: &Report,
    applied: &[PathBuf],
    child_env: &ChildEnv,
) -> Result<Option<CommitSection>, String> {
    if applied.is_empty() {
        return Ok(None);
    }
    let plan = report
        .plan
        .as_ref()
        .ok_or_else(|| "move plan is unavailable".to_string())?;
    let first = plan
        .moves
        .first()
        .ok_or_else(|| "move plan has no selected tasks".to_string())?;
    let count = plan.moves.len();
    let subject = if report.many {
        let dest = report
            .many_dest
            .as_deref()
            .map(dest_stem)
            .unwrap_or_else(|| first.dest_path.display().to_string());
        format!(
            "bob task move -> {dest} ({count} {})",
            if count == 1 { "task" } else { "tasks" }
        )
    } else {
        format!(
            "bob task move {} -> {} ({count} {})",
            first.source_path.display(),
            first.dest_path.display(),
            if count == 1 { "task" } else { "tasks" }
        )
    };
    let committed = task_command_shell::commit_paths(
        &report.vault,
        child_env,
        applied,
        &subject,
    )?;
    Ok(committed.map(|committed| CommitSection {
        sha: committed.sha,
        subject: committed.subject,
        paths: committed
            .paths
            .iter()
            .map(|path| display_relative(path))
            .collect(),
    }))
}

fn dest_stem(raw: &str) -> String {
    let trimmed = raw.trim();
    let inner = trimmed
        .strip_prefix("[[")
        .and_then(|s| s.strip_suffix("]]"))
        .unwrap_or(trimmed);
    let base = inner.rsplit('/').next().unwrap_or(inner);
    let base = base.rsplit(':').next().unwrap_or(base);
    let base = base.split('#').next().unwrap_or(base);
    let base = base.trim().trim_end_matches(".md");
    if base.is_empty() {
        trimmed.to_string()
    } else {
        base.to_string()
    }
}

fn post_sync(report: &mut Report, child_env: &ChildEnv) -> i32 {
    let cycle = task_command_shell::run_post_sync(child_env);
    report.post_sync = Some(SyncSection::from_report(&cycle, false));
    if !cycle.conflicts.is_empty() {
        let mut conflicts = cycle.conflicts.clone();
        conflicts.sort();
        report.fail("post_sync", "post_sync_conflict", format!("post-sync conflict in {}: the remote copy wins in place; inspect _conflicts/ before retrying", conflicts.join(", ")), Some("resolve the _conflicts/ copy and inspect the local move before another attempt"));
        return 1;
    }
    if cycle.ok {
        report.progress("post-sync completed");
        return 0;
    }
    let sha = report
        .commit
        .as_ref()
        .and_then(|commit| commit.sha.as_deref())
        .unwrap_or("unknown");
    report.fail(
        "post_sync",
        "vault_sync_failed",
        format!(
            "post-sync failed ({}); local commit {sha} remains",
            cycle
                .error
                .clone()
                .unwrap_or_else(|| "vault sync reported failure".to_string())
        ),
        Some("run bob vault-sync later to publish the local commit"),
    );
    1
}

fn pre_sync_detail(section: &SyncSection) -> String {
    if section.files_committed == 0 {
        "pre-sync completed; vault already in sync".to_string()
    } else {
        format!(
            "pre-sync committed {} pending note(s)",
            section.files_committed
        )
    }
}

fn display_paths(vault: &Path, paths: &[PathBuf]) -> Vec<String> {
    paths
        .iter()
        .map(|path| relative_display(vault, path))
        .collect()
}

fn relative_display(vault: &Path, path: &Path) -> String {
    path.strip_prefix(vault)
        .map(display_relative)
        .unwrap_or_else(|_| path.display().to_string())
}

fn display_relative(path: &Path) -> String {
    path.components()
        .filter_map(|component| match component {
            std::path::Component::Normal(part) => {
                Some(part.to_string_lossy().into_owned())
            }
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/")
}

fn print_report(report: &Report) {
    match report.format {
        OutputFormat::Json => println!("{}", json_document(report)),
        OutputFormat::Human => print_human(report),
    }
}

fn json_document(report: &Report) -> Value {
    let plan = report.plan.as_ref();
    let moves = plan.map_or_else(Vec::new, |plan| {
        plan.moves
            .iter()
            .map(|item| {
                json!({
                    "source_path": display_relative(&item.source_path),
                    "destination_path": display_relative(&item.dest_path),
                    "source_line": item.source_line,
                    "title": item.title,
                    "old_block_id": item.old_block_id,
                    "new_block_id": item.new_block_id,
                    "old_dependency_id": item.old_dependency_id,
                    "new_dependency_id": item.new_dependency_id,
                })
            })
            .collect()
    });
    let renamed = plan.map_or_else(Vec::new, |plan| {
        plan.renamed_block_ids
            .iter()
            .map(|item| {
                json!({
                    "destination_path": display_relative(&item.path),
                    "old_block_id": item.old_block_id,
                    "new_block_id": item.new_block_id,
                })
            })
            .collect()
    });
    let repaired = plan.map_or_else(Vec::new, |plan| {
        plan.repaired_files
            .iter()
            .map(|item| {
                json!({
                    "path": display_relative(&item.path),
                    "link_count": item.link_count,
                    "dependency_token_count": item.dependency_token_count,
                })
            })
            .collect()
    });
    json!({
        "schema_version": 1,
        "ok": report.error.is_none(),
        "dry_run": report.dry_run,
        "moves": moves,
        "renamed_block_ids": renamed,
        "repaired_files": repaired,
        "needs_reconcile": plan.is_some_and(|plan| plan.needs_reconcile),
        "warnings": report.warnings,
        "relocated": report.relocated,
        "git": {
            "mode": report.mode.as_str(),
            "pre_sync": report.pre_sync.as_ref().map(|sync| json!({"ok": sync.ok, "files_committed": sync.files_committed, "error": sync.error})),
            "commit": report.commit.as_ref().map(|commit| json!({"sha": commit.sha, "subject": commit.subject, "paths": commit.paths})),
            "post_sync": report.post_sync.as_ref().map(|sync| json!({"ok": sync.ok, "pushed": sync.pushed, "conflicts": sync.conflicts, "error": sync.error})),
        },
        "applied_paths": report.applied_paths,
        "deferred_paths": report.deferred_paths,
        "recovery_directory": report.recovery_directory,
        "error": report.error.as_ref().map(|error| json!({"stage": error.stage, "code": error.code, "message": error.message, "hint": error.hint})),
    })
}

fn print_human(report: &Report) {
    let styler = Styler::detect();
    if report.many {
        let count = report
            .plan
            .as_ref()
            .map(|plan| plan.moves.len())
            .unwrap_or(0);
        let dest = report
            .many_dest
            .as_deref()
            .map(|raw| {
                let stem = dest_stem(raw);
                format!("{stem}.md")
            })
            .unwrap_or_default();
        let action = if report.dry_run {
            "Would move"
        } else {
            "Moved"
        };
        println!(
            "{}",
            styler.bold(&format!(
                "↪ {action} {count} {} into {dest} › Tasks",
                if count == 1 { "task" } else { "tasks" }
            ))
        );
    } else {
        let action = if report.dry_run { "Plan" } else { "Move" };
        let mut title = format!("{} {COMMAND_NAME}", styler.bold(action));
        if report.dry_run {
            title.push_str(" · dry run");
        }
        println!("{title}");
    }
    if let Some(plan) = &report.plan {
        println!();
        for item in &plan.moves {
            let identity = match (&item.old_block_id, &item.new_block_id) {
                (Some(old), Some(new)) if old != new => {
                    format!("^{old} → ^{new}")
                }
                (Some(id), _) => format!("^{id}"),
                _ => "(no block id)".to_string(),
            };
            println!(
                "  {} {}:{} → {}  {}",
                styler.cyan(&identity),
                display_relative(&item.source_path),
                item.source_line,
                display_relative(&item.dest_path),
                item.title
            );
            // D10 human row: `Moved to dev · link moved to BUILD`.
            for relocated in &report.relocated {
                let matches_old = item
                    .old_block_id
                    .as_ref()
                    .is_some_and(|id| id == &relocated.block_id);
                let matches_new = item
                    .new_block_id
                    .as_ref()
                    .is_some_and(|id| id == &relocated.block_id);
                if matches_old || matches_new {
                    println!(
                        "    {} link moved to {}",
                        styler.dim("·"),
                        relocated.type_name
                    );
                }
            }
        }
        if !plan.renamed_block_ids.is_empty() {
            println!("\n  Renamed block ids:");
            for item in &plan.renamed_block_ids {
                println!(
                    "    {}  ^{} → ^{}",
                    display_relative(&item.path),
                    item.old_block_id,
                    item.new_block_id
                );
            }
        }
        if !plan.repaired_files.is_empty() {
            println!("\n  Repaired references:");
            for item in &plan.repaired_files {
                println!(
                    "    {}  {} link(s), {} dependency token(s)",
                    display_relative(&item.path),
                    item.link_count,
                    item.dependency_token_count
                );
            }
        }
        if plan.needs_reconcile {
            println!(
                "\n  {} Project blocking changed; run bob task reconcile.",
                styler.yellow("Reconcile:")
            );
        }
        if !report.dry_run && !report.applied_paths.is_empty() {
            println!(
                "\n  {} Applied {} file(s): {}",
                styler.green("✓"),
                report.applied_paths.len(),
                report.applied_paths.join(", ")
            );
        }
    }
    if !report.dry_run && report.mode == GitMode::Offline {
        println!("  Offline mode: sync cycles skipped.");
    }
    if let Some(sync) = &report.pre_sync {
        println!("  {}", pre_sync_detail(sync));
    }
    if let Some(commit) = &report.commit {
        let sha = commit
            .sha
            .as_deref()
            .map(short_sha)
            .unwrap_or_else(|| "no changes".to_string());
        println!(
            "  {} Committed {}  {}",
            styler.green("✓"),
            sha,
            commit.subject
        );
    }
    if let Some(sync) = &report.post_sync {
        if sync.ok && sync.conflicts.is_empty() {
            println!("  {} Post-sync completed.", styler.green("✓"));
        } else {
            println!("  Post-sync did not complete cleanly.");
        }
    }
    if !report.applied_paths.is_empty() && !report.deferred_paths.is_empty() {
        println!("  Applied: {}", report.applied_paths.join(", "));
        println!("  Deferred: {}", report.deferred_paths.join(", "));
    }
    if let Some(recovery) = &report.recovery_directory
        && (!report.deferred_paths.is_empty() || report.error.is_some())
    {
        println!("  Recovery: {recovery}");
    }
    for warning in &report.warnings {
        eprintln!("{}: {warning}", styler.warning_prefix());
    }
    if let Some(error) = &report.error {
        eprintln!("{} {}: {}", styler.red("✗"), error.stage, error.message);
        if let Some(hint) = &error.hint {
            eprintln!("  hint: {hint}");
        }
    }
}

fn short_sha(sha: &str) -> String {
    sha.chars().take(7).collect()
}

#[cfg(test)]
mod tests {
    use std::{
        fs, io,
        path::Path,
        process::{Command, Stdio},
        sync::{
            atomic::{AtomicBool, AtomicUsize, Ordering},
            Arc,
        },
        time::Duration,
    };

    use super::*;

    fn write_fixture(root: &Path) {
        fs::create_dir_all(root).expect("create fixture vault");
        fs::write(
            root.join("source.md"),
            "---\ntype: [[area]]\n---\n\n## Tasks\n\n- [ ] #task Move safely ^move-me\n",
        )
        .expect("write source");
        fs::write(
            root.join("dest.md"),
            "---\ntype: [[area]]\n---\n\n## Tasks\n",
        )
        .expect("write destination");
        fs::write(root.join("backlinks.md"), "See [[source#^move-me]].\n")
            .expect("write backlink");
        fs::write(root.join("context.md"), "original context\n")
            .expect("write context");
    }

    fn request() -> MoveRequest {
        MoveRequest {
            source: "source".to_string(),
            dest: "dest".to_string(),
            selectors: vec![MoveSelector::Block("move-me".to_string())],
            allow_closed: false,
            placement: MovePlacement::Append,
            id_collision: IdCollisionPolicy::Rename,
        }
    }

    fn session(root: &Path, state_home: &Path) -> ApplySession {
        let rescan_root = root.to_path_buf();
        let mut session = ApplySession::production(Box::new(move || {
            link_repair_markdown_files(&rescan_root)
        }));
        session.state_home = state_home.to_path_buf();
        session.quiet_period = Duration::ZERO;
        session
    }

    #[test]
    fn changed_read_set_and_new_markdown_backlink_trigger_fresh_replan() {
        let temp = tempfile::tempdir().expect("temp directory");
        let root = temp.path().join("vault");
        let state_home = temp.path().join("state");
        write_fixture(&root);
        let root_for_callback = root.clone();
        let changed = Arc::new(AtomicBool::new(false));
        let changed_for_callback = Arc::clone(&changed);
        let state_for_factory = state_home.clone();
        let mut factory = move |vault: &Path| {
            let mut session = session(vault, &state_for_factory);
            let root = root_for_callback.clone();
            let changed = Arc::clone(&changed_for_callback);
            session.before_preflight = Some(Box::new(move || {
                if !changed.swap(true, Ordering::SeqCst) {
                    fs::write(
                        root.join("context.md"),
                        "concurrent context kept\n",
                    )
                    .expect("edit read-only input");
                    fs::write(
                        root.join("new-backlink.md"),
                        "[[source#^move-me]]\n",
                    )
                    .expect("add backlink after planning");
                }
            }));
            session
        };
        let mut report = Report::new(OutputFormat::Json, false, true, 5);
        report.vault = root.clone();

        let (applied, partial) = plan_and_apply_with_session_factory(
            &mut report,
            &request(),
            &Duration::from_secs(5),
            &mut factory,
        )
        .expect("apply after one fresh re-plan");

        assert!(!partial);
        assert_eq!(report.error.as_ref().map(|error| error.stage), None);
        assert!(applied.iter().any(|path| path.ends_with("new-backlink.md")));
        assert_eq!(
            fs::read_to_string(root.join("context.md")).unwrap(),
            "concurrent context kept\n"
        );
        assert!(fs::read_to_string(root.join("new-backlink.md"))
            .unwrap()
            .contains("[[dest#^move-me]]"));
        assert!(!fs::read_to_string(root.join("source.md"))
            .unwrap()
            .contains("Move safely"));
    }

    #[test]
    fn partial_write_leaves_source_when_its_last_replacement_is_interrupted() {
        let temp = tempfile::tempdir().expect("temp directory");
        let root = temp.path().join("vault");
        let state_home = temp.path().join("state");
        write_fixture(&root);
        let source = root.join("source.md");
        let source_for_callback = source.clone();
        let interrupted = Arc::new(AtomicBool::new(false));
        let interrupted_for_callback = Arc::clone(&interrupted);
        let state_for_factory = state_home.clone();
        let mut factory = move |vault: &Path| {
            let mut session = session(vault, &state_for_factory);
            let source = source_for_callback.clone();
            let interrupted = Arc::clone(&interrupted_for_callback);
            session.before_replace = Some(Box::new(move |path| {
                if path == source && !interrupted.swap(true, Ordering::SeqCst) {
                    let existing =
                        fs::read_to_string(path).expect("read source");
                    fs::write(path, format!("{existing}\nconcurrent edit\n"))
                        .expect("change source before replacement");
                }
            }));
            session
        };
        let mut report = Report::new(OutputFormat::Json, false, true, 5);
        report.vault = root.clone();

        let (applied, partial) = plan_and_apply_with_session_factory(
            &mut report,
            &request(),
            &Duration::from_secs(5),
            &mut factory,
        )
        .expect("partial application is reported");

        assert!(partial);
        assert_eq!(
            report.error.as_ref().map(|error| error.stage),
            Some("apply")
        );
        assert_eq!(report.applied_paths, vec!["dest.md", "backlinks.md"]);
        assert_eq!(report.deferred_paths, vec!["source.md"]);
        assert_eq!(applied.len(), 2);
        assert!(fs::read_to_string(source)
            .unwrap()
            .contains("Move safely ^move-me\n\nconcurrent edit"));
        assert!(fs::read_to_string(root.join("dest.md"))
            .unwrap()
            .contains("Move safely"));
    }

    #[test]
    fn clap_accepts_mixed_repeated_block_and_match_selectors() {
        let matches = build_cli()
            .try_get_matches_from([
                COMMAND_NAME,
                "src",
                "dest",
                "-b",
                "c",
                "-m",
                "Alpha",
                "-b",
                "a",
                "-b",
                "b",
            ])
            .expect("mixed selectors");
        assert_eq!(
            selectors_from_matches(&matches),
            vec![
                MoveSelector::Block("c".into()),
                MoveSelector::Block("a".into()),
                MoveSelector::Block("b".into()),
                MoveSelector::Match("Alpha".into()),
            ]
        );
    }

    #[test]
    fn clap_defers_selector_validation_to_run() {
        let matches = build_cli()
            .try_get_matches_from([COMMAND_NAME, "src", "dest"])
            .expect("selector validation happens in run");
        assert!(selectors_from_matches(&matches).is_empty());
    }

    #[test]
    fn fail_keeps_the_primary_error_and_warns_about_later_failures() {
        let mut report = Report::new(OutputFormat::Json, false, true, 5);
        report.fail(
            "apply",
            "partial_write",
            "partial write",
            Some("inspect recovery"),
        );
        report.fail(
            "commit",
            "git_commit_failed",
            "hook rejected the commit",
            Some("fix git"),
        );
        assert_eq!(
            report.error.as_ref().map(|error| error.stage),
            Some("apply")
        );
        assert!(
            report
                .warnings
                .iter()
                .any(|warning| warning.contains("also failed during commit")),
            "{:?}",
            report.warnings
        );
        let json = json_document(&report);
        assert_eq!(json["ok"], false);
        assert_eq!(json["error"]["stage"], "apply");
        assert!(json["warnings"].as_array().unwrap().iter().any(|warning| {
            warning
                .as_str()
                .unwrap()
                .contains("also failed during commit")
        }));
    }

    #[test]
    fn retryable_scan_is_retried_until_a_fresh_snapshot_succeeds() {
        let temp = tempfile::tempdir().expect("temp directory");
        let root = temp.path().join("vault");
        let state_home = temp.path().join("state");
        write_fixture(&root);
        let attempts = Arc::new(AtomicUsize::new(0));
        let attempts_for_scan = Arc::clone(&attempts);
        let state_for_factory = state_home.clone();
        let mut session_factory =
            move |vault: &Path| session(vault, &state_for_factory);
        let mut scan_factory = move |vault: &Path| {
            let attempt = attempts_for_scan.fetch_add(1, Ordering::SeqCst);
            if attempt == 0 {
                return Err(ScanError::Retryable(
                    "read note source.md: file changed while it was being read"
                        .into(),
                ));
            }
            scan_vault(vault)
        };
        let mut report = Report::new(OutputFormat::Json, false, true, 5);
        report.vault = root.clone();

        let (applied, partial) = plan_and_apply_with_hooks(
            &mut report,
            &request(),
            &Duration::from_secs(5),
            &mut session_factory,
            &mut scan_factory,
        )
        .expect("retryable scan then apply");

        assert!(!partial);
        assert_eq!(attempts.load(Ordering::SeqCst), 2);
        assert!(applied.iter().any(|path| path.ends_with("dest.md")));
        assert!(fs::read_to_string(root.join("dest.md"))
            .unwrap()
            .contains("Move safely"));
    }

    #[test]
    fn fatal_scan_does_not_retry_even_with_budget() {
        let temp = tempfile::tempdir().expect("temp directory");
        let root = temp.path().join("vault");
        let state_home = temp.path().join("state");
        write_fixture(&root);
        let attempts = Arc::new(AtomicUsize::new(0));
        let attempts_for_scan = Arc::clone(&attempts);
        let state_for_factory = state_home.clone();
        let mut session_factory =
            move |vault: &Path| session(vault, &state_for_factory);
        let mut scan_factory = move |_vault: &Path| {
            attempts_for_scan.fetch_add(1, Ordering::SeqCst);
            Err(ScanError::Fatal(
                "read note source.md: permission denied".into(),
            ))
        };
        let mut report = Report::new(OutputFormat::Json, false, true, 5);
        report.vault = root.clone();

        let outcome = plan_and_apply_with_hooks(
            &mut report,
            &request(),
            &Duration::from_secs(5),
            &mut session_factory,
            &mut scan_factory,
        );

        assert!(outcome.is_none());
        assert_eq!(attempts.load(Ordering::SeqCst), 1);
        assert_eq!(
            report.error.as_ref().map(|error| error.stage),
            Some("plan")
        );
        assert_eq!(
            fs::read_to_string(root.join("source.md")).unwrap(),
            "---\ntype: [[area]]\n---\n\n## Tasks\n\n- [ ] #task Move safely ^move-me\n"
        );
    }

    #[test]
    fn zero_budget_makes_a_retryable_scan_terminal() {
        let temp = tempfile::tempdir().expect("temp directory");
        let root = temp.path().join("vault");
        let state_home = temp.path().join("state");
        write_fixture(&root);
        let attempts = Arc::new(AtomicUsize::new(0));
        let attempts_for_scan = Arc::clone(&attempts);
        let state_for_factory = state_home.clone();
        let mut session_factory =
            move |vault: &Path| session(vault, &state_for_factory);
        let mut scan_factory = move |_vault: &Path| {
            attempts_for_scan.fetch_add(1, Ordering::SeqCst);
            Err(ScanError::Retryable("unstable".into()))
        };
        let mut report = Report::new(OutputFormat::Json, false, true, 0);
        report.vault = root.clone();

        let outcome = plan_and_apply_with_hooks(
            &mut report,
            &request(),
            &Duration::ZERO,
            &mut session_factory,
            &mut scan_factory,
        );

        assert!(outcome.is_none());
        assert_eq!(attempts.load(Ordering::SeqCst), 1);
        assert_eq!(
            report.error.as_ref().map(|error| error.stage),
            Some("plan")
        );
    }

    #[test]
    fn staging_failure_writes_no_notes_and_keeps_recovery() {
        let temp = tempfile::tempdir().expect("temp directory");
        let root = temp.path().join("vault");
        let state_home = temp.path().join("state");
        write_fixture(&root);
        let source_before = fs::read(root.join("source.md")).unwrap();
        let dest_before = fs::read(root.join("dest.md")).unwrap();
        let backlinks_before = fs::read(root.join("backlinks.md")).unwrap();
        let state_for_factory = state_home.clone();
        let mut factory = move |vault: &Path| {
            let mut session = session(vault, &state_for_factory);
            session.fail_staging = Some(Box::new(|_| {
                Err(io::Error::other("injected staging failure"))
            }));
            session
        };
        let mut report = Report::new(OutputFormat::Json, false, true, 5);
        report.vault = root.clone();

        let outcome = plan_and_apply_with_session_factory(
            &mut report,
            &request(),
            &Duration::from_secs(5),
            &mut factory,
        );

        assert!(outcome.is_none());
        assert_eq!(
            report.error.as_ref().map(|error| error.stage),
            Some("apply")
        );
        assert!(report.applied_paths.is_empty());
        assert_eq!(
            report.deferred_paths,
            vec!["dest.md", "backlinks.md", "source.md"]
        );
        assert!(
            report
                .recovery_directory
                .as_ref()
                .is_some_and(|path| Path::new(path).is_dir()),
            "{:?}",
            report.recovery_directory
        );
        assert_eq!(fs::read(root.join("source.md")).unwrap(), source_before);
        assert_eq!(fs::read(root.join("dest.md")).unwrap(), dest_before);
        assert_eq!(
            fs::read(root.join("backlinks.md")).unwrap(),
            backlinks_before
        );
        let json = json_document(&report);
        assert_eq!(json["ok"], false);
        assert_eq!(json["error"]["stage"], "apply");
        assert!(json["error"]["hint"]
            .as_str()
            .unwrap()
            .contains("no move files were written"));
    }

    #[test]
    fn failure_after_destination_leaves_later_files_deferred() {
        let temp = tempfile::tempdir().expect("temp directory");
        let root = temp.path().join("vault");
        let state_home = temp.path().join("state");
        write_fixture(&root);
        let backlinks = root.join("backlinks.md");
        let backlinks_for_callback = backlinks.clone();
        let interrupted = Arc::new(AtomicBool::new(false));
        let interrupted_for_callback = Arc::clone(&interrupted);
        let state_for_factory = state_home.clone();
        let mut factory = move |vault: &Path| {
            let mut session = session(vault, &state_for_factory);
            let backlinks = backlinks_for_callback.clone();
            let interrupted = Arc::clone(&interrupted_for_callback);
            session.before_replace = Some(Box::new(move |path| {
                if path == backlinks
                    && !interrupted.swap(true, Ordering::SeqCst)
                {
                    let existing =
                        fs::read_to_string(path).expect("read backlinks");
                    fs::write(path, format!("{existing}concurrent edit\n"))
                        .expect("change backlinks before replacement");
                }
            }));
            session
        };
        let mut report = Report::new(OutputFormat::Json, false, true, 5);
        report.vault = root.clone();

        let (applied, partial) = plan_and_apply_with_session_factory(
            &mut report,
            &request(),
            &Duration::from_secs(5),
            &mut factory,
        )
        .expect("partial application is reported");

        assert!(partial);
        assert_eq!(
            report.error.as_ref().map(|error| error.stage),
            Some("apply")
        );
        assert_eq!(report.applied_paths, vec!["dest.md"]);
        assert_eq!(report.deferred_paths, vec!["backlinks.md", "source.md"]);
        assert_eq!(applied.len(), 1);
        assert!(
            report
                .recovery_directory
                .as_ref()
                .is_some_and(|path| Path::new(path).is_dir()),
            "{:?}",
            report.recovery_directory
        );
        assert!(fs::read_to_string(root.join("dest.md"))
            .unwrap()
            .contains("Move safely"));
        assert!(fs::read_to_string(root.join("source.md"))
            .unwrap()
            .contains("Move safely ^move-me"));
        assert!(fs::read_to_string(&backlinks)
            .unwrap()
            .contains("[[source#^move-me]]"));
        assert!(fs::read_to_string(&backlinks)
            .unwrap()
            .contains("concurrent edit"));
        let json = json_document(&report);
        assert_eq!(json["ok"], false);
        assert_eq!(json["applied_paths"][0], "dest.md");
        assert!(json["error"]["hint"]
            .as_str()
            .unwrap()
            .contains("selectors are not replayed automatically"));
    }

    #[test]
    fn ordinary_commit_failure_skips_post_sync_and_keeps_the_write() {
        let temp = tempfile::tempdir().expect("temp directory");
        let root = temp.path().join("vault");
        let state_home = temp.path().join("state");
        write_fixture(&root);
        init_repo(&root);
        reject_commits(&root);
        let state_for_factory = state_home.clone();
        let mut factory =
            move |vault: &Path| session(vault, &state_for_factory);
        let mut report = Report::new(OutputFormat::Json, false, true, 5);
        report.vault = root.clone();
        let (applied, partial) = plan_and_apply_with_session_factory(
            &mut report,
            &request(),
            &Duration::from_secs(5),
            &mut factory,
        )
        .expect("apply before commit");
        assert!(!partial);

        let exit = finish_after_writes(
            &mut report,
            &applied,
            partial,
            true,
            &Vec::new(),
        );

        assert_eq!(exit, 1);
        assert_eq!(
            report.error.as_ref().map(|error| error.stage),
            Some("commit")
        );
        assert!(report.post_sync.is_none());
        assert!(report.commit.is_none());
        assert!(fs::read_to_string(root.join("dest.md"))
            .unwrap()
            .contains("Move safely"));
        assert!(!fs::read_to_string(root.join("source.md"))
            .unwrap()
            .contains("Move safely"));
        assert_eq!(git_subject(&root), "initial");
    }

    #[test]
    fn secondary_commit_failure_after_partial_apply_is_a_warning() {
        let temp = tempfile::tempdir().expect("temp directory");
        let root = temp.path().join("vault");
        let state_home = temp.path().join("state");
        write_fixture(&root);
        init_repo(&root);
        let source = root.join("source.md");
        let source_for_callback = source.clone();
        let interrupted = Arc::new(AtomicBool::new(false));
        let interrupted_for_callback = Arc::clone(&interrupted);
        let state_for_factory = state_home.clone();
        let mut factory = move |vault: &Path| {
            let mut session = session(vault, &state_for_factory);
            let source = source_for_callback.clone();
            let interrupted = Arc::clone(&interrupted_for_callback);
            session.before_replace = Some(Box::new(move |path| {
                if path == source && !interrupted.swap(true, Ordering::SeqCst) {
                    let existing =
                        fs::read_to_string(path).expect("read source");
                    fs::write(path, format!("{existing}\nconcurrent edit\n"))
                        .expect("change source before replacement");
                }
            }));
            session
        };
        let mut report = Report::new(OutputFormat::Json, false, true, 5);
        report.vault = root.clone();
        let (applied, partial) = plan_and_apply_with_session_factory(
            &mut report,
            &request(),
            &Duration::from_secs(5),
            &mut factory,
        )
        .expect("partial application is reported");
        assert!(partial);
        reject_commits(&root);

        let exit = finish_after_writes(
            &mut report,
            &applied,
            partial,
            true,
            &Vec::new(),
        );

        assert_eq!(exit, 1);
        assert_eq!(
            report.error.as_ref().map(|error| error.stage),
            Some("apply")
        );
        assert!(
            report
                .warnings
                .iter()
                .any(|warning| warning.contains("also failed during commit")),
            "{:?}",
            report.warnings
        );
        assert!(report.post_sync.is_none());
        assert_eq!(report.applied_paths, vec!["dest.md", "backlinks.md"]);
        assert_eq!(report.deferred_paths, vec!["source.md"]);
        assert!(fs::read_to_string(root.join("dest.md"))
            .unwrap()
            .contains("Move safely"));
        assert!(fs::read_to_string(&source)
            .unwrap()
            .contains("concurrent edit"));
        let json = json_document(&report);
        assert_eq!(json["ok"], false);
        assert_eq!(json["error"]["stage"], "apply");
        assert!(json["error"]["hint"]
            .as_str()
            .unwrap()
            .contains("selectors are not replayed automatically"));
    }

    fn init_repo(vault: &Path) {
        let run = |args: &[&str]| {
            let status = Command::new("git")
                .arg("-C")
                .arg(vault)
                .args(args)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .expect("git");
            assert!(status.success(), "git {args:?} failed");
        };
        run(&["init", "-q"]);
        run(&["config", "user.email", "test@example.com"]);
        run(&["config", "user.name", "Test"]);
        run(&["config", "commit.gpgsign", "false"]);
        run(&["add", "-A"]);
        run(&["commit", "-q", "-m", "initial"]);
    }

    fn reject_commits(vault: &Path) {
        let hook = vault.join(".git/hooks/commit-msg");
        // Fork-safe scratch-then-copy writer: git executes this hook while
        // other test threads fork, so an in-place `fs::write` can fail the
        // hook exec with ETXTBSY (bead `bob-cli-5l`).
        crate::native::test_stub::write_executable_stub(
            &hook,
            "#!/bin/sh\nexit 1\n",
        );
    }

    fn git_subject(vault: &Path) -> String {
        let output = Command::new("git")
            .arg("-C")
            .arg(vault)
            .args(["log", "-1", "--format=%s"])
            .output()
            .expect("git log");
        assert!(output.status.success());
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    }
}
