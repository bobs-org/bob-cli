use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    ffi::{OsStr, OsString},
    fs, io, iter,
    ops::Range,
    path::{Component, Path, PathBuf},
    process,
};

use clap::{
    builder::OsStringValueParser, Arg, ArgAction, ArgMatches,
    Command as ClapCommand,
};
use serde::Serialize;
use serde_json::{json, Value};

const POMODORO_MARKER: &str = "🍅";

use super::{
    collect_done, env as bob_env, is_always_excluded_note_directory_name,
    pomodoro,
    style::{display_width, pad_right, Styler},
};

const COMMAND_NAME: &str = "bob task-status-hooks";
const DEFAULT_GLOBAL_FILTER: &str = "#task";
const TASKS_SETTINGS: &str =
    ".obsidian/plugins/obsidian-tasks-plugin/data.json";

pub(crate) fn run(args: Vec<OsString>) -> i32 {
    let mut command = build_cli();
    let matches = match command.try_get_matches_from_mut(
        iter::once(OsString::from(COMMAND_NAME)).chain(args),
    ) {
        Ok(matches) => matches,
        Err(error) => return print_clap_error(error),
    };

    let format = OutputFormat::from_matches(&matches);
    let request = Request::from_matches(&matches);
    match sync_task_statuses(&request) {
        Ok(result) => {
            print_result(&result, format);
            0
        }
        Err(error) => print_error(error, format),
    }
}

fn print_clap_error(error: clap::Error) -> i32 {
    let exit_code = error.exit_code();
    if let Err(print_error) = error.print() {
        eprintln!(
            "{COMMAND_NAME}: failed to print command-line error: {print_error}"
        );
    }
    exit_code
}

fn build_cli() -> ClapCommand {
    ClapCommand::new(COMMAND_NAME)
        .about("Sync active and dependency-blocked task statuses")
        .long_about(
            "Make today's Pomodoro ledger the source of truth for active task statuses.\n\n\
Tasks block-linked from child bullets of open Pomodoro entries have a minimum \
desired status of Next [*]; tasks already In Progress [/] keep that stronger \
status. Dependency tasks are discovered recursively from sole transcluded \
block-link child bullets and inherit the strongest effective parent status, \
promoting Ready [ ] tasks to Next or In Progress and Next tasks to In Progress. \
Status propagation never lowers a task. Existing [*] tasks not reachable from \
an open entry are independently reset to [ ]. Tasks whose Dataview \
[dependsOn:: ...] metadata names an open vault-wide [id:: ...] task are marked \
Blocked [?], overriding Ready, Next, and In Progress. When those dependencies \
close, Blocked tasks recover to the final Pomodoro-derived status or Ready. \
Completed linked-task references \
are retired as struck, non-embedded links. References found under open \
Pomodoros keep the existing policy of moving their containing bullets beneath \
the current timed Pomodoro, or the last completed Pomodoro when \
there is no current one. A link to an unambiguously cancelled Tasks task \
removes its complete Markdown list-item subtree from an open Pomodoro, \
including for custom single-character statuses whose Tasks type is CANCELLED; \
the cancelled task status itself is left unchanged. \
Done, cancelled, non-task, and unknown task statuses are never transitioned.\n\n\
When the same resolved task is linked beneath multiple open Pomodoros, the \
first open Pomodoro in file order keeps ownership and every conflicting \
physical line beneath later open Pomodoros is removed in full. Aliases, \
embeds, same-note links, and alternate note spellings compare by resolved \
vault-relative path plus block ID. Repeats within one owning Pomodoro are \
preserved, as are unresolved links and links beneath completed or cancelled \
Pomodoros. If a block ID matches multiple task lines, canceled-reference \
list-item removal requires every match to have a recognized CANCELLED status.\n\n\
Only Markdown checkbox lines allowed by the Obsidian Tasks globalFilter are \
considered. The scan skips hidden directories, templates, generated notes, \
and done archives. Blocked writes require exactly one compatible Tasks status \
named Blocked with symbol [?], type ON_HOLD, and next status Ready. Missing \
daily notes and daily notes without a Pomodoros \
section, as well as notes with multiple open timed Pomodoros, fail before any \
file is changed.",
        )
        .after_help(format!(
            "Examples:\n  {COMMAND_NAME}\n  {COMMAND_NAME} --dry-run\n  {COMMAND_NAME} --format json\n  {COMMAND_NAME} --bob-dir /tmp/bob-vault\n\nEnvironment:\n  BOB_DAY_FILE  exact daily note used as the Pomodoro source\n  BOB_DIR       Bob vault root when --bob-dir is omitted\n  BOB_NOW       current date/time override for daily-note selection"
        ))
        .disable_help_flag(true)
        .arg(
            Arg::new("bob-dir")
                .long("bob-dir")
                .short('b')
                .value_name("DIR")
                .value_parser(OsStringValueParser::new())
                .help("Bob vault root; defaults to BOB_DIR or ~/bob"),
        )
        .arg(
            Arg::new("dry-run")
                .long("dry-run")
                .short('d')
                .action(ArgAction::SetTrue)
                .help("Compute and report the sync without writing notes"),
        )
        .arg(
            Arg::new("format")
                .long("format")
                .short('f')
                .value_name("FORMAT")
                .value_parser(["human", "json"])
                .default_value("human")
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
struct Request {
    bob_dir: PathBuf,
    dry_run: bool,
}

impl Request {
    fn from_matches(matches: &ArgMatches) -> Self {
        let bob_dir = matches
            .get_one::<OsString>("bob-dir")
            .map(PathBuf::from)
            .map(|path| bob_env::expand_tilde(&path))
            .unwrap_or_else(bob_env::bob_dir);
        Self {
            bob_dir,
            dry_run: matches.get_flag("dry-run"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct ChangeItem {
    path: String,
    line_number: usize,
    block_id: String,
    description: String,
    dependency: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct DependencyStatusChange {
    path: String,
    line_number: usize,
    block_id: String,
    description: String,
    from: char,
    to: char,
    open_dependency_ids: Vec<String>,
    unresolved_dependency_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct UnresolvedReference {
    target: String,
    block_id: String,
    reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct StruckCompletedReference {
    target: String,
    block_id: String,
    pomodoro: String,
    removed_embed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct MovedCompletedReference {
    target: String,
    block_id: String,
    source_pomodoro: String,
    destination_pomodoro: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct MarkerReference {
    target: String,
    block_id: String,
    pomodoro: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct RemovedCanceledReference {
    target: String,
    block_id: String,
    line_number: usize,
    pomodoro: String,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
struct DuplicateTaskIdentity {
    path: String,
    block_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct RemovedDuplicateLine {
    line_number: usize,
    pomodoro: String,
    line: String,
    duplicate_tasks: Vec<DuplicateTaskIdentity>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct SyncResult {
    ok: bool,
    dry_run: bool,
    daily_file: String,
    open_pomodoros: usize,
    references: usize,
    dependency_references: usize,
    scanned_files: usize,
    marked_next: Vec<ChangeItem>,
    marked_in_progress: Vec<ChangeItem>,
    cleared: Vec<ChangeItem>,
    marked_blocked: Vec<DependencyStatusChange>,
    unblocked: Vec<DependencyStatusChange>,
    struck_completed_references: Vec<StruckCompletedReference>,
    embedded_completed_references: Vec<StruckCompletedReference>,
    moved_completed_references: Vec<MovedCompletedReference>,
    marker_added_references: Vec<MarkerReference>,
    marker_removed_references: Vec<MarkerReference>,
    removed_canceled_references: Vec<RemovedCanceledReference>,
    removed_duplicate_lines: Vec<RemovedDuplicateLine>,
    kept_next: usize,
    kept_in_progress: usize,
    unresolved_references: Vec<UnresolvedReference>,
}

#[derive(Debug, Clone)]
struct FileScan {
    path: PathBuf,
    relative_path: PathBuf,
    contents: String,
    tasks: Vec<TaskLine>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TaskLine {
    line_index: usize,
    status: char,
    status_byte_offset: usize,
    block_id: Option<String>,
    task_id: Option<String>,
    depends_on: Vec<String>,
    status_type: TaskStatusType,
    status_recognized: bool,
    description: String,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct RawReference {
    target: String,
    block_id: String,
}

#[derive(Debug, Clone)]
struct PlannedChange {
    file_index: usize,
    status_byte_offset: usize,
    replacement: char,
}

#[derive(Debug, Clone)]
struct TasksSettings {
    global_filter: String,
    done_statuses: BTreeSet<char>,
    status_types: BTreeMap<char, TaskStatusType>,
    status_definitions: Vec<TaskStatusDefinition>,
    status_settings_error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TaskStatusDefinition {
    symbol: String,
    name: String,
    next_status_symbol: String,
    available_as_command: bool,
    status_type: TaskStatusType,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TaskStatusType {
    Todo,
    Done,
    InProgress,
    OnHold,
    Cancelled,
    NonTask,
    Empty,
}

impl TaskStatusType {
    fn from_settings(value: &str) -> Self {
        match value {
            "DONE" => Self::Done,
            "IN_PROGRESS" => Self::InProgress,
            "ON_HOLD" => Self::OnHold,
            "CANCELLED" => Self::Cancelled,
            "NON_TASK" => Self::NonTask,
            "EMPTY" => Self::Empty,
            _ => Self::Todo,
        }
    }

    fn is_open(self) -> bool {
        matches!(self, Self::Todo | Self::InProgress | Self::OnHold)
    }

    fn is_terminal(self) -> bool {
        matches!(self, Self::Done | Self::Cancelled | Self::NonTask)
    }
}

#[derive(Debug, Clone)]
struct PomodoroEntry {
    line_index: usize,
    end_line: usize,
    open: bool,
    completed: bool,
    timed: bool,
    child_indentation: Option<String>,
    context: String,
}

#[derive(Debug, Clone)]
struct LinkOccurrence {
    reference: RawReference,
    edit_start: usize,
    edit_end: usize,
    current_token: String,
    preserved_marked_token: String,
    preserved_unmarked_token: String,
    retired_marked_token: String,
    retired_unmarked_token: String,
    embedded: bool,
    struck: bool,
    marker_count: usize,
}

#[derive(Debug, Clone)]
struct LinkBullet {
    entry_index: usize,
    line_index: usize,
    end_line: usize,
    indentation: String,
    links: Vec<LinkOccurrence>,
}

#[derive(Debug, Clone)]
struct PomodoroModel {
    entries: Vec<PomodoroEntry>,
    bullets: Vec<LinkBullet>,
    open_pomodoros: usize,
    raw_references: BTreeSet<RawReference>,
    all_references: BTreeSet<RawReference>,
}

#[derive(Debug, Clone)]
struct ResolvedReference {
    path: PathBuf,
    statuses: Vec<char>,
}

#[derive(Debug, Clone)]
struct StructuralPlan {
    token_edits: BTreeMap<usize, Vec<TokenEdit>>,
    moves: Vec<BulletMove>,
    deleted_lines: BTreeSet<usize>,
    target_entry: Option<usize>,
    struck: Vec<StruckCompletedReference>,
    moved: Vec<MovedCompletedReference>,
    marker_added: Vec<MarkerReference>,
    marker_removed: Vec<MarkerReference>,
    removed_canceled: Vec<RemovedCanceledReference>,
}

#[derive(Debug, Clone)]
struct TokenEdit {
    start: usize,
    end: usize,
    replacement: String,
}

#[derive(Debug, Clone)]
struct BulletMove {
    start_line: usize,
    end_line: usize,
    source_indentation: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Transition {
    MarkNext,
    MarkInProgress,
    Clear,
    MarkBlocked,
    Unblock(RankedStatus),
    KeptNext,
    KeptInProgress,
    Unchanged,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum RankedStatus {
    Ready,
    Next,
    InProgress,
}

impl RankedStatus {
    fn from_checkbox(status: char) -> Option<Self> {
        match status {
            ' ' => Some(Self::Ready),
            '*' => Some(Self::Next),
            '/' => Some(Self::InProgress),
            _ => None,
        }
    }

    fn checkbox(self) -> char {
        match self {
            Self::Ready => ' ',
            Self::Next => '*',
            Self::InProgress => '/',
        }
    }
}

fn sync_task_statuses(request: &Request) -> Result<SyncResult, SyncError> {
    let daily_path = pomodoro::day_file_for(&request.bob_dir);
    let daily_contents = fs::read_to_string(&daily_path).map_err(|error| {
        if error.kind() == io::ErrorKind::NotFound {
            SyncError::new(format!(
                "daily note does not exist: {}",
                daily_path.display()
            ))
        } else {
            SyncError::io("read daily note", &daily_path, error)
        }
    })?;
    let daily_lines = logical_lines(&daily_contents);
    let section =
        pomodoro::pomodoros_section_range(&daily_lines).ok_or_else(|| {
            SyncError::new(format!(
                "daily note has no Pomodoros section: {}",
                daily_path.display()
            ))
        })?;
    let pomodoro_model = scan_pomodoros(&daily_lines, section.clone());
    let timed_open = pomodoro_model
        .entries
        .iter()
        .filter(|entry| entry.open && entry.timed)
        .count();
    if timed_open > 1 {
        return Err(SyncError::new(
            "Bob daily note has multiple open timed Pomodoros",
        ));
    }

    let settings = read_tasks_settings(&request.bob_dir);
    let markdown_files = markdown_files(&request.bob_dir).map_err(|error| {
        SyncError::io("scan vault", &request.bob_dir, error)
    })?;
    let canonical_daily_path = daily_path.canonicalize().ok();
    let mut files = Vec::with_capacity(markdown_files.len());
    for path in markdown_files {
        let contents = if path == daily_path
            || canonical_daily_path.as_ref().is_some_and(|daily| {
                path.canonicalize().ok().as_ref() == Some(daily)
            }) {
            daily_contents.clone()
        } else {
            fs::read_to_string(&path)
                .map_err(|error| SyncError::io("read note", &path, error))?
        };
        let relative_path = path
            .strip_prefix(&request.bob_dir)
            .map(Path::to_path_buf)
            .map_err(|_| {
                SyncError::new(format!(
                    "note is not under the vault root: {}",
                    path.display()
                ))
            })?;
        let tasks = parse_tasks(&contents, &settings);
        files.push(FileScan {
            path,
            relative_path,
            contents,
            tasks,
        });
    }

    let note_index = NoteIndex::from_paths(
        files.iter().map(|file| file.relative_path.clone()),
    );
    let task_blocks = task_blocks(&files);
    let daily_relative = daily_path.strip_prefix(&request.bob_dir).ok();
    let mut unresolved = Vec::new();
    let dependency_edges =
        dependency_edges(&files, &note_index, &task_blocks, &mut unresolved);
    let mut resolved_references = BTreeMap::new();
    for reference in &pomodoro_model.all_references {
        let resolved =
            note_index.resolve(daily_relative, reference.target.trim());
        let Some(path) = resolved else {
            unresolved.push(UnresolvedReference {
                target: reference.target.clone(),
                block_id: reference.block_id.clone(),
                reason: "note target did not resolve uniquely".to_string(),
            });
            continue;
        };
        let key = (path.clone(), reference.block_id.clone());
        let statuses = task_blocks.get(&key).cloned().unwrap_or_default();
        if statuses.is_empty() {
            unresolved.push(UnresolvedReference {
                target: reference.target.clone(),
                block_id: reference.block_id.clone(),
                reason: format!(
                    "{} has no matching task block",
                    display_path(&path)
                ),
            });
            continue;
        }
        if statuses.len() > 1 {
            let has_done = statuses
                .iter()
                .any(|status| is_done_status(*status, &settings.done_statuses));
            let has_not_done = statuses.iter().any(|status| {
                !is_done_status(*status, &settings.done_statuses)
            });
            let has_canceled = statuses.iter().any(|status| {
                is_canceled_status(*status, &settings.status_types)
            });
            let has_not_canceled = statuses.iter().any(|status| {
                !is_canceled_status(*status, &settings.status_types)
            });
            let reason = if has_done && has_not_done && has_canceled {
                format!(
                    "{} has {} tasks with conflicting statuses; completed-link normalization and canceled-reference list-item removal were skipped",
                    display_path(&path),
                    statuses.len()
                )
            } else if has_done && has_not_done {
                format!(
                    "{} has {} tasks with conflicting statuses; completed-link normalization was skipped",
                    display_path(&path),
                    statuses.len()
                )
            } else if has_canceled && has_not_canceled {
                format!(
                    "{} has {} tasks with conflicting statuses; canceled-reference list-item removal was skipped",
                    display_path(&path),
                    statuses.len()
                )
            } else {
                format!(
                    "{} has {} tasks with this block id; all were matched",
                    display_path(&path),
                    statuses.len()
                )
            };
            unresolved.push(UnresolvedReference {
                target: reference.target.clone(),
                block_id: reference.block_id.clone(),
                reason,
            });
        }
        resolved_references
            .insert(reference.clone(), ResolvedReference { path, statuses });
    }

    let removed_duplicate_lines = plan_duplicate_line_removals(
        &daily_lines,
        &pomodoro_model,
        &resolved_references,
    );
    let deleted_lines = removed_duplicate_lines
        .iter()
        .map(|item| item.line_number - 1)
        .collect::<BTreeSet<_>>();
    let structural_plan = plan_structural_changes(
        &pomodoro_model,
        &resolved_references,
        &settings.done_statuses,
        &settings.status_types,
        &deleted_lines,
    );
    let structurally_updated_daily = apply_structural_plan(
        &daily_contents,
        &pomodoro_model,
        &structural_plan,
    );
    let updated_lines = logical_lines(&structurally_updated_daily);
    let updated_section = pomodoro::pomodoros_section_range(&updated_lines)
        .expect("the structural rewrite preserves the Pomodoros section");
    let final_references =
        scan_pomodoros(&updated_lines, updated_section).raw_references;
    let direct_desired = final_references
        .iter()
        .filter_map(|reference| {
            resolved_references.get(reference).map(|resolved| {
                (resolved.path.clone(), reference.block_id.clone())
            })
        })
        .collect::<BTreeSet<_>>();
    let desired =
        desired_statuses(&direct_desired, &dependency_edges, &task_blocks);
    let dependency_desired = desired
        .keys()
        .filter(|identity| !direct_desired.contains(*identity))
        .cloned()
        .collect::<BTreeSet<_>>();
    let task_dependency_states = task_dependency_states(&files);

    let mut marked_next = Vec::new();
    let mut marked_in_progress = Vec::new();
    let mut cleared = Vec::new();
    let mut marked_blocked = Vec::new();
    let mut unblocked = Vec::new();
    let mut changes = Vec::new();
    let mut kept_next = 0;
    let mut kept_in_progress = 0;
    for (file_index, file) in files.iter().enumerate() {
        for (task_index, task) in file.tasks.iter().enumerate() {
            let desired_status = task.block_id.as_ref().and_then(|block_id| {
                desired
                    .get(&(file.relative_path.clone(), block_id.clone()))
                    .copied()
            });
            let dependency_state = task_dependency_states
                .get(&(file_index, task_index))
                .cloned()
                .unwrap_or_default();
            match task_transition(
                task,
                desired_status,
                !dependency_state.open_dependency_ids.is_empty(),
            ) {
                Transition::MarkNext => {
                    let dependency =
                        task.block_id.as_ref().is_some_and(|block_id| {
                            dependency_desired.contains(&(
                                file.relative_path.clone(),
                                block_id.clone(),
                            ))
                        });
                    marked_next.push(change_item(file, task, dependency));
                    changes.push(PlannedChange {
                        file_index,
                        status_byte_offset: task.status_byte_offset,
                        replacement: '*',
                    });
                }
                Transition::MarkInProgress => {
                    let dependency =
                        task.block_id.as_ref().is_some_and(|block_id| {
                            dependency_desired.contains(&(
                                file.relative_path.clone(),
                                block_id.clone(),
                            ))
                        });
                    marked_in_progress
                        .push(change_item(file, task, dependency));
                    changes.push(PlannedChange {
                        file_index,
                        status_byte_offset: task.status_byte_offset,
                        replacement: '/',
                    });
                }
                Transition::Clear => {
                    cleared.push(change_item(file, task, false));
                    changes.push(PlannedChange {
                        file_index,
                        status_byte_offset: task.status_byte_offset,
                        replacement: ' ',
                    });
                }
                Transition::MarkBlocked => {
                    marked_blocked.push(dependency_status_change(
                        file,
                        task,
                        '?',
                        &dependency_state,
                    ));
                    changes.push(PlannedChange {
                        file_index,
                        status_byte_offset: task.status_byte_offset,
                        replacement: '?',
                    });
                }
                Transition::Unblock(status) => {
                    let replacement = status.checkbox();
                    unblocked.push(dependency_status_change(
                        file,
                        task,
                        replacement,
                        &dependency_state,
                    ));
                    changes.push(PlannedChange {
                        file_index,
                        status_byte_offset: task.status_byte_offset,
                        replacement,
                    });
                }
                Transition::KeptNext => kept_next += 1,
                Transition::KeptInProgress => kept_in_progress += 1,
                Transition::Unchanged => {}
            }
        }
    }

    if !marked_blocked.is_empty() || !unblocked.is_empty() {
        validate_blocked_status(&settings)?;
    }

    let outputs = compose_outputs(
        &files,
        &changes,
        &daily_path,
        &daily_contents,
        &pomodoro_model,
        &structural_plan,
    );
    if !request.dry_run {
        apply_outputs(&outputs)?;
    }

    Ok(SyncResult {
        ok: true,
        dry_run: request.dry_run,
        daily_file: daily_relative
            .map(display_path)
            .unwrap_or_else(|| daily_path.to_string_lossy().into_owned()),
        open_pomodoros: pomodoro_model.open_pomodoros,
        references: pomodoro_model.raw_references.len(),
        dependency_references: dependency_desired.len(),
        scanned_files: files.len(),
        marked_next,
        marked_in_progress,
        cleared,
        marked_blocked,
        unblocked,
        struck_completed_references: structural_plan.struck,
        embedded_completed_references: Vec::new(),
        moved_completed_references: structural_plan.moved,
        marker_added_references: structural_plan.marker_added,
        marker_removed_references: structural_plan.marker_removed,
        removed_canceled_references: structural_plan.removed_canceled,
        removed_duplicate_lines,
        kept_next,
        kept_in_progress,
        unresolved_references: unresolved,
    })
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct TaskDependencyState {
    open_dependency_ids: Vec<String>,
    unresolved_dependency_ids: Vec<String>,
}

fn task_dependency_states(
    files: &[FileScan],
) -> BTreeMap<(usize, usize), TaskDependencyState> {
    let mut identities = BTreeMap::<String, bool>::new();
    for file in files {
        for task in &file.tasks {
            let Some(task_id) = &task.task_id else {
                continue;
            };
            let is_open = identities.entry(task_id.clone()).or_default();
            *is_open |= task.status_recognized && task.status_type.is_open();
        }
    }

    let mut states = BTreeMap::new();
    for (file_index, file) in files.iter().enumerate() {
        for (task_index, task) in file.tasks.iter().enumerate() {
            let mut state = TaskDependencyState::default();
            for dependency in &task.depends_on {
                match identities.get(dependency) {
                    Some(true) => {
                        state.open_dependency_ids.push(dependency.clone())
                    }
                    None => {
                        state.unresolved_dependency_ids.push(dependency.clone())
                    }
                    Some(false) => {}
                }
            }
            state.open_dependency_ids.sort();
            state.open_dependency_ids.dedup();
            state.unresolved_dependency_ids.sort();
            state.unresolved_dependency_ids.dedup();
            states.insert((file_index, task_index), state);
        }
    }
    states
}

fn task_transition(
    task: &TaskLine,
    desired: Option<RankedStatus>,
    has_open_dependency: bool,
) -> Transition {
    if task.status_type.is_terminal()
        || !task.status_type.is_open()
        || !task.status_recognized
    {
        return Transition::Unchanged;
    }
    if has_open_dependency {
        return if task.status == '?' {
            Transition::Unchanged
        } else {
            Transition::MarkBlocked
        };
    }
    if task.status == '?' {
        return Transition::Unblock(desired.unwrap_or(RankedStatus::Ready));
    }
    transition(task.status, desired)
}

fn transition(status: char, desired: Option<RankedStatus>) -> Transition {
    let Some(desired) = desired else {
        return if status == '*' {
            Transition::Clear
        } else {
            Transition::Unchanged
        };
    };
    let Some(current) = RankedStatus::from_checkbox(status) else {
        return Transition::Unchanged;
    };
    if current < desired {
        return match desired {
            RankedStatus::Next => Transition::MarkNext,
            RankedStatus::InProgress => Transition::MarkInProgress,
            RankedStatus::Ready => Transition::Unchanged,
        };
    }
    match current {
        RankedStatus::Next => Transition::KeptNext,
        RankedStatus::InProgress => Transition::KeptInProgress,
        RankedStatus::Ready => Transition::Unchanged,
    }
}

fn logical_lines(contents: &str) -> Vec<&str> {
    contents.split_inclusive('\n').map(logical_line).collect()
}

fn logical_line(segment: &str) -> &str {
    let without_lf = segment.strip_suffix('\n').unwrap_or(segment);
    without_lf.strip_suffix('\r').unwrap_or(without_lf)
}

fn scan_pomodoros(lines: &[&str], section: Range<usize>) -> PomodoroModel {
    let fenced_lines = fenced_lines(lines, section.clone());
    let mut entries = Vec::new();
    for line_index in section.clone() {
        let line = lines[line_index];
        if fenced_lines.contains(&line_index)
            || line.starts_with(' ')
            || line.starts_with('\t')
            || !line.starts_with('-')
        {
            continue;
        }
        let open_task = pomodoro::open_ledger_task(line);
        let completed_task = pomodoro::completed_ledger_task(line);
        if open_task.is_none() && completed_task.is_none() {
            continue;
        }
        let end_line = entry_block_end(lines, line_index, section.end);
        let child_indentation = (line_index + 1..end_line)
            .filter(|index| !fenced_lines.contains(index))
            .find_map(|index| bullet_indentation(lines[index]));
        entries.push(PomodoroEntry {
            line_index,
            end_line,
            open: open_task.is_some(),
            completed: completed_task.is_some(),
            timed: open_task
                .is_some_and(|task| pomodoro::task_time_range(task).is_some()),
            child_indentation,
            context: line.trim_end().to_string(),
        });
    }

    let mut bullets = Vec::new();
    let mut raw_references = BTreeSet::new();
    let mut all_references = BTreeSet::new();
    for (entry_index, entry) in entries.iter().enumerate() {
        for line_index in entry.line_index + 1..entry.end_line {
            if fenced_lines.contains(&line_index) {
                continue;
            }
            let Some(indentation) = bullet_indentation(lines[line_index])
            else {
                continue;
            };
            let links = block_link_occurrences(lines[line_index]);
            if links.is_empty() {
                continue;
            }
            all_references
                .extend(links.iter().map(|link| link.reference.clone()));
            if entry.open {
                raw_references.extend(
                    links
                        .iter()
                        .filter(|link| !link.struck)
                        .map(|link| link.reference.clone()),
                );
            }
            bullets.push(LinkBullet {
                entry_index,
                line_index,
                end_line: bullet_block_end(
                    lines,
                    line_index,
                    entry.end_line,
                    indentation.len(),
                ),
                indentation,
                links,
            });
        }
    }

    PomodoroModel {
        open_pomodoros: entries.iter().filter(|entry| entry.open).count(),
        entries,
        bullets,
        raw_references,
        all_references,
    }
}

fn is_sub_bullet(line: &str) -> bool {
    if line
        .chars()
        .next()
        .is_some_and(|character| matches!(character, '-' | '*' | '+'))
    {
        return line.chars().nth(1).is_some_and(char::is_whitespace);
    }
    let digits = line
        .bytes()
        .take_while(|byte| byte.is_ascii_digit())
        .count();
    digits > 0
        && line.as_bytes().get(digits).is_some_and(|byte| {
            matches!(byte, b'.' | b')')
                && line
                    .as_bytes()
                    .get(digits + 1)
                    .is_some_and(u8::is_ascii_whitespace)
        })
}

fn bullet_indentation(line: &str) -> Option<String> {
    let indentation_len = line
        .as_bytes()
        .iter()
        .take_while(|byte| matches!(byte, b' ' | b'\t'))
        .count();
    (indentation_len > 0 && is_sub_bullet(&line[indentation_len..]))
        .then(|| line[..indentation_len].to_string())
}

fn block_link_occurrences(line: &str) -> Vec<LinkOccurrence> {
    let mut links = Vec::new();
    let struck_spans = strikethrough_spans(line);
    let mut rest = line;
    let mut base = 0;
    while let Some(open) = rest.find("[[") {
        let absolute_open = base + open;
        let after_open = &rest[open + 2..];
        let Some(close) = after_open.find("]]") else {
            break;
        };
        let inside = &after_open[..close];
        let link_end = absolute_open + 2 + close + 2;
        let link_target = inside.split('|').next().unwrap_or("");
        if let Some(fragment) = link_target.find("#^") {
            let target = link_target[..fragment].trim();
            let block_id = link_target[fragment + 2..].trim();
            if !block_id.is_empty()
                && block_id.bytes().all(collect_done::is_block_id_byte)
            {
                let embedded = line[..absolute_open].ends_with('!');
                let token_start = absolute_open - usize::from(embedded);
                let struck_span = struck_spans.iter().find(|span| {
                    token_start >= span.start + 2 && link_end <= span.end - 2
                });
                let struck = struck_span.is_some();
                let exact_struck_span = struck_span.filter(|span| {
                    token_start == span.start + 2 && link_end == span.end - 2
                });
                let display_start =
                    exact_struck_span.map_or(token_start, |span| span.start);
                let edit_end =
                    exact_struck_span.map_or(link_end, |span| span.end);
                let (edit_start, marker_count) =
                    pomodoro_marker_prefix(line, display_start);
                let wikilink = &line[absolute_open..link_end];
                let before = if !struck && line[..token_start].ends_with("~~") {
                    " "
                } else {
                    ""
                };
                let after = if !struck && line[link_end..].starts_with("~~") {
                    " "
                } else {
                    ""
                };
                let preserved = &line[display_start..edit_end];
                let (retired_marked_token, retired_unmarked_token) = if struck {
                    if exact_struck_span.is_some() {
                        let token = format!("~~{wikilink}~~");
                        (format!("{POMODORO_MARKER} {token}"), token)
                    } else {
                        (
                            format!("{POMODORO_MARKER} {wikilink}"),
                            wikilink.to_string(),
                        )
                    }
                } else {
                    (
                        format!(
                            "{before}{POMODORO_MARKER} ~~{wikilink}~~{after}"
                        ),
                        format!("{before}~~{wikilink}~~{after}"),
                    )
                };
                links.push(LinkOccurrence {
                    reference: RawReference {
                        target: target.to_string(),
                        block_id: block_id.to_string(),
                    },
                    edit_start,
                    edit_end,
                    current_token: line[edit_start..edit_end].to_string(),
                    preserved_marked_token: format!(
                        "{POMODORO_MARKER} {preserved}"
                    ),
                    preserved_unmarked_token: preserved.to_string(),
                    retired_marked_token,
                    retired_unmarked_token,
                    embedded,
                    struck,
                    marker_count,
                });
            }
        }
        base = absolute_open + 2 + close + 2;
        rest = &after_open[close + 2..];
    }
    links
}

fn pomodoro_marker_prefix(line: &str, token_start: usize) -> (usize, usize) {
    let mut cursor = token_start;
    let mut count = 0;
    loop {
        let whitespace_end = cursor;
        let mut marker_end = cursor;
        while marker_end > 0
            && matches!(line.as_bytes()[marker_end - 1], b' ' | b'\t')
        {
            marker_end -= 1;
        }
        if marker_end == whitespace_end
            || !line[..marker_end].ends_with(POMODORO_MARKER)
        {
            break;
        }
        cursor = marker_end - POMODORO_MARKER.len();
        count += 1;
    }
    if count == 0 {
        return (token_start, 0);
    }
    (cursor, count)
}

fn desired_link_token(
    link: &LinkOccurrence,
    retire: bool,
    marked: bool,
) -> &str {
    match (retire, marked) {
        (true, true) => &link.retired_marked_token,
        (true, false) => &link.retired_unmarked_token,
        (false, true) => &link.preserved_marked_token,
        (false, false) => &link.preserved_unmarked_token,
    }
}

fn completed_pomodoro_marker_expected(link: &LinkOccurrence) -> bool {
    if link.embedded {
        return false;
    }
    if link.struck {
        return link.marker_count > 0;
    }
    true
}

fn marker_expected_for_occurrence(
    entry: &PomodoroEntry,
    link: &LinkOccurrence,
) -> bool {
    entry.completed && completed_pomodoro_marker_expected(link)
}

fn strikethrough_spans(line: &str) -> Vec<std::ops::Range<usize>> {
    let mut delimiters = Vec::new();
    let mut cursor = 0;
    while let Some(offset) = line[cursor..].find("~~") {
        let position = cursor + offset;
        delimiters.push(position);
        cursor = position + 2;
    }
    delimiters
        .chunks_exact(2)
        .map(|pair| pair[0]..pair[1] + 2)
        .collect()
}

fn read_tasks_settings(vault: &Path) -> TasksSettings {
    let mut settings = TasksSettings {
        global_filter: DEFAULT_GLOBAL_FILTER.to_string(),
        done_statuses: BTreeSet::from(['x', 'X']),
        status_types: BTreeMap::from([
            (' ', TaskStatusType::Todo),
            ('x', TaskStatusType::Done),
            ('X', TaskStatusType::Done),
            ('/', TaskStatusType::InProgress),
            ('*', TaskStatusType::OnHold),
            ('-', TaskStatusType::Cancelled),
        ]),
        status_definitions: Vec::new(),
        status_settings_error: None,
    };
    let path = vault.join(TASKS_SETTINGS);
    let Ok(contents) = fs::read_to_string(&path) else {
        settings.status_settings_error =
            Some(format!("Tasks settings are missing at {}", path.display()));
        return settings;
    };
    let Ok(value) = serde_json::from_str::<Value>(&contents) else {
        settings.status_settings_error = Some(format!(
            "Tasks settings are not valid JSON at {}",
            path.display()
        ));
        return settings;
    };
    settings.global_filter = value
        .get("globalFilter")
        .and_then(Value::as_str)
        .unwrap_or(DEFAULT_GLOBAL_FILTER)
        .to_string();
    let mut configured_symbols = BTreeSet::new();
    for collection in ["coreStatuses", "customStatuses"] {
        let statuses = value
            .get("statusSettings")
            .and_then(|settings| settings.get(collection))
            .and_then(Value::as_array)
            .into_iter()
            .flatten();
        for status in statuses {
            let symbol = status
                .get("symbol")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let status_type = TaskStatusType::from_settings(
                status.get("type").and_then(Value::as_str).unwrap_or("TODO"),
            );
            let definition = TaskStatusDefinition {
                symbol: symbol.to_string(),
                name: status
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                next_status_symbol: status
                    .get("nextStatusSymbol")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                available_as_command: status
                    .get("availableAsCommand")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
                status_type,
            };
            settings.status_definitions.push(definition);

            let mut chars = symbol.chars();
            if let Some(symbol) = chars.next()
                && chars.next().is_none()
            {
                if configured_symbols.insert(symbol) {
                    settings.status_types.insert(symbol, status_type);
                }
                if status_type == TaskStatusType::Done {
                    settings.done_statuses.insert(symbol);
                }
            }
        }
    }
    settings
}

fn validate_blocked_status(settings: &TasksSettings) -> Result<(), SyncError> {
    if let Some(error) = &settings.status_settings_error {
        return Err(SyncError::new(format!(
            "cannot reconcile Blocked [?] tasks: {error}; configure one custom status named Blocked with symbol '?', type ON_HOLD, next status ' ', and availableAsCommand true"
        )));
    }
    let candidates = settings
        .status_definitions
        .iter()
        .filter(|definition| {
            definition.symbol == "?"
                || definition.name.eq_ignore_ascii_case("Blocked")
        })
        .collect::<Vec<_>>();
    if candidates.is_empty() {
        return Err(SyncError::new(
            "cannot reconcile Blocked [?] tasks: Tasks settings have no Blocked status; configure one custom status named Blocked with symbol '?', type ON_HOLD, next status ' ', and availableAsCommand true",
        ));
    }
    if candidates.len() > 1 {
        return Err(SyncError::new(format!(
            "cannot reconcile Blocked [?] tasks: Tasks settings contain {} definitions using symbol '?' or name Blocked; keep exactly one compatible definition",
            candidates.len()
        )));
    }
    let definition = candidates[0];
    if definition.symbol != "?"
        || definition.name != "Blocked"
        || definition.status_type != TaskStatusType::OnHold
        || definition.next_status_symbol != " "
        || !definition.available_as_command
    {
        return Err(SyncError::new(format!(
            "cannot reconcile Blocked [?] tasks: the Tasks status is incompatible (symbol={:?}, name={:?}, type={:?}, next={:?}, availableAsCommand={}); expected symbol '?', name Blocked, type ON_HOLD, next status ' ', and availableAsCommand true",
            definition.symbol,
            definition.name,
            definition.status_type,
            definition.next_status_symbol,
            definition.available_as_command
        )));
    }
    Ok(())
}

fn fenced_lines(lines: &[&str], section: Range<usize>) -> BTreeSet<usize> {
    super::markdown::fenced_lines(lines, section)
}

fn entry_block_end(
    lines: &[&str],
    entry_line: usize,
    section_end: usize,
) -> usize {
    let mut index = entry_line + 1;
    while index < section_end {
        if leading_indentation_len(lines[index]) > 0
            || (lines[index].trim().is_empty()
                && next_nonblank_is_indented(lines, index + 1, section_end))
        {
            index += 1;
        } else {
            break;
        }
    }
    index
}

fn bullet_block_end(
    lines: &[&str],
    bullet_line: usize,
    entry_end: usize,
    indentation: usize,
) -> usize {
    let mut index = bullet_line + 1;
    while index < entry_end {
        let line = lines[index];
        if leading_indentation_len(line) > indentation
            || (line.trim().is_empty()
                && next_nonblank_more_indented(
                    lines,
                    index + 1,
                    entry_end,
                    indentation,
                ))
        {
            index += 1;
        } else {
            break;
        }
    }
    index
}

fn leading_indentation_len(line: &str) -> usize {
    line.as_bytes()
        .iter()
        .take_while(|byte| matches!(byte, b' ' | b'\t'))
        .count()
}

fn next_nonblank_is_indented(lines: &[&str], start: usize, end: usize) -> bool {
    lines[start..end]
        .iter()
        .find(|line| !line.trim().is_empty())
        .is_some_and(|line| leading_indentation_len(line) > 0)
}

fn next_nonblank_more_indented(
    lines: &[&str],
    start: usize,
    end: usize,
    indentation: usize,
) -> bool {
    lines[start..end]
        .iter()
        .find(|line| !line.trim().is_empty())
        .is_some_and(|line| leading_indentation_len(line) > indentation)
}

fn is_done_status(status: char, done_statuses: &BTreeSet<char>) -> bool {
    matches!(status, 'x' | 'X') || done_statuses.contains(&status)
}

fn is_canceled_status(
    status: char,
    status_types: &BTreeMap<char, TaskStatusType>,
) -> bool {
    status_types.get(&status) == Some(&TaskStatusType::Cancelled)
}

fn plan_duplicate_line_removals(
    lines: &[&str],
    model: &PomodoroModel,
    resolved: &BTreeMap<RawReference, ResolvedReference>,
) -> Vec<RemovedDuplicateLine> {
    let mut owners: BTreeMap<(PathBuf, String), usize> = BTreeMap::new();
    let mut removed = Vec::new();

    for bullet in &model.bullets {
        let entry = &model.entries[bullet.entry_index];
        if !entry.open {
            continue;
        }
        let tasks = bullet
            .links
            .iter()
            .filter_map(|link| {
                resolved.get(&link.reference).map(|reference| {
                    (reference.path.clone(), link.reference.block_id.clone())
                })
            })
            .collect::<BTreeSet<_>>();
        if tasks.is_empty() {
            continue;
        }

        let duplicate_tasks = tasks
            .iter()
            .filter(|task| {
                owners
                    .get(*task)
                    .is_some_and(|owner| *owner != bullet.entry_index)
            })
            .map(|(path, block_id)| DuplicateTaskIdentity {
                path: display_path(path),
                block_id: block_id.clone(),
            })
            .collect::<Vec<_>>();
        if !duplicate_tasks.is_empty() {
            removed.push(RemovedDuplicateLine {
                line_number: bullet.line_index + 1,
                pomodoro: entry.context.clone(),
                line: lines[bullet.line_index].to_string(),
                duplicate_tasks,
            });
            continue;
        }

        for task in tasks {
            owners.entry(task).or_insert(bullet.entry_index);
        }
    }

    removed
}

fn plan_structural_changes(
    model: &PomodoroModel,
    resolved: &BTreeMap<RawReference, ResolvedReference>,
    done_statuses: &BTreeSet<char>,
    status_types: &BTreeMap<char, TaskStatusType>,
    duplicate_deleted_lines: &BTreeSet<usize>,
) -> StructuralPlan {
    let current = model
        .entries
        .iter()
        .position(|entry| entry.open && entry.timed);
    let fallback = model.entries.iter().rposition(|entry| entry.completed);
    let target_entry = current.or(fallback);
    let mut token_edits: BTreeMap<usize, Vec<TokenEdit>> = BTreeMap::new();
    let mut move_candidates = Vec::new();
    let mut struck = Vec::new();
    let mut moved = Vec::new();
    let mut marker_added = Vec::new();
    let mut marker_removed = Vec::new();
    let mut removed_canceled = Vec::new();
    let mut deleted_lines = duplicate_deleted_lines.clone();

    // Cancellation owns the complete list-item subtree. Plan it before any
    // token edits or moves so descendants and sibling links that will be
    // deleted cannot produce additional structural work or reports.
    for bullet in &model.bullets {
        if deleted_lines.contains(&bullet.line_index) {
            continue;
        }
        let source = &model.entries[bullet.entry_index];
        let canceled_links = bullet
            .links
            .iter()
            .filter(|link| {
                source.open
                    && resolved.get(&link.reference).is_some_and(|reference| {
                        !reference.statuses.is_empty()
                            && reference.statuses.iter().all(|status| {
                                is_canceled_status(*status, status_types)
                            })
                    })
            })
            .collect::<Vec<_>>();
        if canceled_links.is_empty() {
            continue;
        }
        for link in canceled_links {
            removed_canceled.push(RemovedCanceledReference {
                target: link.reference.target.clone(),
                block_id: link.reference.block_id.clone(),
                line_number: bullet.line_index + 1,
                pomodoro: source.context.clone(),
            });
        }
        deleted_lines.extend(bullet.line_index..bullet.end_line);
    }

    for bullet in &model.bullets {
        if deleted_lines.contains(&bullet.line_index) {
            continue;
        }
        let source = &model.entries[bullet.entry_index];
        let completed_links = bullet
            .links
            .iter()
            .filter(|link| {
                resolved.get(&link.reference).is_some_and(|reference| {
                    !reference.statuses.is_empty()
                        && reference.statuses.iter().all(|status| {
                            is_done_status(*status, done_statuses)
                        })
                })
            })
            .collect::<Vec<_>>();
        let move_target = if !completed_links.is_empty() && source.open {
            target_entry.filter(|target| {
                if *target == bullet.entry_index {
                    return false;
                }
                let has_live_reference = bullet.links.iter().any(|link| {
                    resolved.get(&link.reference).is_some_and(|reference| {
                        reference.statuses.iter().any(|status| {
                            !is_done_status(*status, done_statuses)
                        })
                    })
                });
                model.entries[*target].open || !has_live_reference
            })
        } else {
            None
        };
        let final_entry = move_target.unwrap_or(bullet.entry_index);
        for link in &bullet.links {
            let retire = completed_links
                .iter()
                .any(|completed| std::ptr::eq(*completed, link));
            let marker_expected = marker_expected_for_occurrence(
                &model.entries[final_entry],
                link,
            );
            let replacement = desired_link_token(link, retire, marker_expected);
            if link.current_token != replacement {
                token_edits.entry(bullet.line_index).or_default().push(
                    TokenEdit {
                        start: link.edit_start,
                        end: link.edit_end,
                        replacement: replacement.to_string(),
                    },
                );
            }
            if retire && (!link.struck || link.embedded) {
                struck.push(StruckCompletedReference {
                    target: link.reference.target.clone(),
                    block_id: link.reference.block_id.clone(),
                    pomodoro: source.context.clone(),
                    removed_embed: link.embedded,
                });
            }

            let desired_marker_count = usize::from(marker_expected);
            if link.marker_count != desired_marker_count {
                let item = MarkerReference {
                    target: link.reference.target.clone(),
                    block_id: link.reference.block_id.clone(),
                    pomodoro: model.entries[final_entry].context.clone(),
                };
                if link.marker_count < desired_marker_count {
                    marker_added.push(item);
                } else {
                    marker_removed.push(item);
                }
            }
        }

        if let Some(target) = move_target {
            move_candidates.push(BulletMove {
                start_line: bullet.line_index,
                end_line: bullet.end_line,
                source_indentation: bullet.indentation.clone(),
            });
            for link in completed_links {
                moved.push(MovedCompletedReference {
                    target: link.reference.target.clone(),
                    block_id: link.reference.block_id.clone(),
                    source_pomodoro: source.context.clone(),
                    destination_pomodoro: model.entries[target].context.clone(),
                });
            }
        }
    }

    move_candidates.sort_by_key(|item| (item.start_line, item.end_line));
    let mut moves: Vec<BulletMove> = Vec::new();
    for candidate in move_candidates {
        if moves
            .last()
            .is_some_and(|previous| candidate.start_line < previous.end_line)
        {
            continue;
        }
        moves.push(candidate);
    }

    StructuralPlan {
        token_edits,
        moves,
        deleted_lines,
        target_entry,
        struck,
        moved,
        marker_added,
        marker_removed,
        removed_canceled,
    }
}

fn apply_structural_plan(
    contents: &str,
    model: &PomodoroModel,
    plan: &StructuralPlan,
) -> String {
    if plan.token_edits.is_empty()
        && plan.moves.is_empty()
        && plan.deleted_lines.is_empty()
    {
        return contents.to_string();
    }
    let mut lines = contents
        .split_inclusive('\n')
        .map(str::to_string)
        .collect::<Vec<_>>();
    for (line_index, edits) in &plan.token_edits {
        let mut edits = edits.iter().collect::<Vec<_>>();
        edits.sort_by_key(|edit| edit.start);
        for edit in edits.into_iter().rev() {
            lines[*line_index]
                .replace_range(edit.start..edit.end, &edit.replacement);
        }
    }

    let target_indentation = plan.target_entry.map(|target| {
        model.entries[target]
            .child_indentation
            .clone()
            .or_else(|| {
                model
                    .entries
                    .iter()
                    .find_map(|entry| entry.child_indentation.clone())
            })
            .unwrap_or_else(|| "  ".to_string())
    });
    let mut removed = plan.deleted_lines.clone();
    let mut moved_lines = Vec::new();
    for item in &plan.moves {
        let target_indentation = target_indentation
            .as_deref()
            .expect("moves always have a target Pomodoro");
        for (index, line) in lines
            .iter()
            .enumerate()
            .take(item.end_line)
            .skip(item.start_line)
        {
            removed.insert(index);
            if !plan.deleted_lines.contains(&index) {
                moved_lines.push(reindent_segment(
                    line,
                    &item.source_indentation,
                    target_indentation,
                ));
            }
        }
    }
    let insertion_line = plan
        .target_entry
        .map(|target| model.entries[target].end_line);
    let ending = if contents.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    };
    let mut output =
        String::with_capacity(contents.len() + plan.struck.len() * 4);
    for index in 0..=lines.len() {
        if insertion_line == Some(index) && !moved_lines.is_empty() {
            if !output.is_empty() && !output.ends_with('\n') {
                output.push_str(ending);
            }
            for line in &moved_lines {
                output.push_str(line);
            }
            if index < lines.len() && !output.ends_with('\n') {
                output.push_str(ending);
            }
        }
        if index < lines.len() && !removed.contains(&index) {
            output.push_str(&lines[index]);
        }
    }
    output
}

fn reindent_segment(
    segment: &str,
    source_indentation: &str,
    target_indentation: &str,
) -> String {
    let line = logical_line(segment);
    if line.is_empty() || !line.starts_with(source_indentation) {
        return segment.to_string();
    }
    let ending = &segment[line.len()..];
    format!(
        "{target_indentation}{}{ending}",
        &line[source_indentation.len()..]
    )
}

fn markdown_files(vault: &Path) -> io::Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    collect_markdown_files(vault, &mut files)?;
    Ok(files)
}

fn collect_markdown_files(
    directory: &Path,
    files: &mut Vec<PathBuf>,
) -> io::Result<()> {
    let mut entries =
        fs::read_dir(directory)?.collect::<Result<Vec<_>, io::Error>>()?;
    entries.sort_by_key(|entry| entry.path());
    for entry in entries {
        let path = entry.path();
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            let name = entry.file_name();
            if should_skip_directory(&name) {
                continue;
            }
            collect_markdown_files(&path, files)?;
        } else if file_type.is_file()
            && path
                .extension()
                .and_then(OsStr::to_str)
                .is_some_and(|extension| extension.eq_ignore_ascii_case("md"))
        {
            files.push(path);
        }
    }
    Ok(())
}

fn should_skip_directory(name: &OsStr) -> bool {
    name == OsStr::new("done")
        || name.to_str().is_some_and(|name| name.starts_with('.'))
        || is_always_excluded_note_directory_name(name)
}

fn parse_tasks(contents: &str, settings: &TasksSettings) -> Vec<TaskLine> {
    let mut tasks = Vec::new();
    let mut byte_start = 0;
    for (line_index, segment) in contents.split_inclusive('\n').enumerate() {
        let line = logical_line(segment);
        if let Some(mut task) = parse_task_line(line, settings) {
            task.line_index = line_index;
            task.status_byte_offset += byte_start;
            tasks.push(task);
        }
        byte_start += segment.len();
    }
    tasks
}

fn parse_task_line(line: &str, settings: &TasksSettings) -> Option<TaskLine> {
    let bytes = line.as_bytes();
    let mut index = bytes
        .iter()
        .take_while(|byte| matches!(byte, b' ' | b'\t'))
        .count();
    index = after_list_marker(line, index)?;
    while bytes.get(index).is_some_and(u8::is_ascii_whitespace) {
        index += 1;
    }
    if bytes.get(index) != Some(&b'[') {
        return None;
    }
    let close_offset = line[index + 1..].find(']')?;
    let close = index + 1 + close_offset;
    let status_text = &line[index + 1..close];
    let mut status_chars = status_text.chars();
    let status = status_chars.next()?;
    if status_chars.next().is_some() {
        return None;
    }
    let after_checkbox = &line[close + 1..];
    if !after_checkbox.is_empty()
        && !after_checkbox.starts_with(char::is_whitespace)
    {
        return None;
    }
    let body = after_checkbox.trim_start();
    if !body.contains(&settings.global_filter) {
        return None;
    }
    let block_id = trailing_block_id(body);
    let metadata = task_metadata(body, block_id.as_deref());
    let description =
        task_description(body, &settings.global_filter, block_id.as_deref());
    let configured_status = settings.status_types.get(&status).copied();
    Some(TaskLine {
        line_index: 0,
        status,
        status_byte_offset: index + 1,
        block_id,
        task_id: metadata.task_id,
        depends_on: metadata.depends_on,
        status_type: configured_status.unwrap_or(TaskStatusType::Todo),
        status_recognized: configured_status.is_some(),
        description,
    })
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct TaskMetadata {
    task_id: Option<String>,
    depends_on: Vec<String>,
}

fn task_metadata(body: &str, block_id: Option<&str>) -> TaskMetadata {
    let mut metadata = TaskMetadata::default();
    let mut state = block_id
        .and_then(|block_id| {
            body.trim_end()
                .strip_suffix(&format!("^{block_id}"))
                .map(str::trim_end)
        })
        .unwrap_or(body);

    for _ in 0..20 {
        let Some((start, key, value)) = trailing_dataview_field(state) else {
            if let Some(start) = trailing_task_tag_start(state) {
                state = state[..start].trim_end();
                continue;
            }
            break;
        };
        let recognized = match key {
            "id" if valid_task_identity(value) => {
                metadata.task_id = Some(value.to_string());
                true
            }
            "dependsOn" => parse_task_dependencies(value)
                .map(|dependencies| metadata.depends_on = dependencies)
                .is_some(),
            "priority" => matches!(
                value,
                "highest" | "high" | "medium" | "low" | "lowest"
            ),
            "start" | "created" | "scheduled" | "due" | "completion"
            | "cancelled" => valid_task_date(value),
            "repeat" => value.bytes().all(|byte| {
                byte.is_ascii_alphanumeric()
                    || matches!(byte, b',' | b' ' | b'!')
            }),
            "onCompletion" => {
                !value.is_empty()
                    && value.bytes().all(|byte| byte.is_ascii_alphabetic())
            }
            _ => false,
        };
        if !recognized {
            break;
        }
        state = state[..start].trim_end();
    }
    metadata
}

fn trailing_task_tag_start(state: &str) -> Option<usize> {
    let trimmed = state.trim_end();
    let start = trimmed
        .char_indices()
        .rev()
        .find(|(_, character)| character.is_whitespace())
        .map_or(0, |(index, character)| index + character.len_utf8());
    let tag = &trimmed[start..];
    let value = tag.strip_prefix('#')?;
    (!value.is_empty()
        && !value.chars().any(|character| {
            character.is_whitespace()
                || "!@#$%^&*(),.?\":{}|<>".contains(character)
        }))
    .then_some(start)
}

fn trailing_dataview_field(state: &str) -> Option<(usize, &str, &str)> {
    let mut end = state.trim_end().len();
    if state[..end].ends_with(',') {
        end -= 1;
        end = state[..end].trim_end().len();
    }
    let close = state[..end].chars().next_back()?;
    let open = match close {
        ']' => '[',
        ')' => '(',
        _ => return None,
    };
    let without_close = &state[..end - close.len_utf8()];
    let start = without_close.rfind(open)?;
    let inner = without_close[start + open.len_utf8()..].trim();
    let (key, value) = inner.split_once("::")?;
    (key == key.trim()).then_some((start, key, value.trim()))
}

fn valid_task_identity(value: &str) -> bool {
    !value.is_empty()
        && value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-')
        })
}

fn parse_task_dependencies(value: &str) -> Option<Vec<String>> {
    if value.is_empty() || value.bytes().any(|byte| byte == b'\t') {
        return None;
    }
    value
        .split(',')
        .map(|part| part.trim_matches(' '))
        .map(|part| valid_task_identity(part).then(|| part.to_string()))
        .collect()
}

fn valid_task_date(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() == 10
        && bytes[4] == b'-'
        && bytes[7] == b'-'
        && bytes.iter().enumerate().all(|(index, byte)| {
            matches!(index, 4 | 7) || byte.is_ascii_digit()
        })
}

fn after_list_marker(line: &str, index: usize) -> Option<usize> {
    let bytes = line.as_bytes();
    if matches!(bytes.get(index), Some(b'-' | b'*' | b'+')) {
        return bytes
            .get(index + 1)
            .is_some_and(u8::is_ascii_whitespace)
            .then_some(index + 1);
    }
    let digits = bytes[index..]
        .iter()
        .take_while(|byte| byte.is_ascii_digit())
        .count();
    if digits == 0 || !matches!(bytes.get(index + digits), Some(b'.' | b')')) {
        return None;
    }
    bytes
        .get(index + digits + 1)
        .is_some_and(u8::is_ascii_whitespace)
        .then_some(index + digits + 1)
}

fn trailing_block_id(body: &str) -> Option<String> {
    let token = body.split_whitespace().next_back()?;
    let block_id = token.strip_prefix('^')?;
    (!block_id.is_empty()
        && block_id.bytes().all(collect_done::is_block_id_byte))
    .then(|| block_id.to_string())
}

fn task_description(
    body: &str,
    global_filter: &str,
    block_id: Option<&str>,
) -> String {
    let without_block = block_id
        .and_then(|block_id| {
            body.trim_end()
                .strip_suffix(&format!("^{block_id}"))
                .map(str::trim_end)
        })
        .unwrap_or(body);
    without_block
        .replacen(global_filter, "", 1)
        .trim()
        .to_string()
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct NoteIndex {
    relative_paths: BTreeSet<PathBuf>,
    basename_paths: BTreeMap<String, Option<PathBuf>>,
}

impl NoteIndex {
    fn from_paths<I>(paths: I) -> Self
    where
        I: IntoIterator<Item = PathBuf>,
    {
        let mut relative_paths = BTreeSet::new();
        let mut basename_paths = BTreeMap::new();
        for path in paths {
            if let Some(name) = markdown_basename(&path) {
                match basename_paths.entry(name.to_lowercase()) {
                    std::collections::btree_map::Entry::Vacant(entry) => {
                        entry.insert(Some(path.clone()));
                    }
                    std::collections::btree_map::Entry::Occupied(mut entry) => {
                        if entry.get().as_ref() != Some(&path) {
                            entry.insert(None);
                        }
                    }
                }
            }
            relative_paths.insert(path);
        }
        Self {
            relative_paths,
            basename_paths,
        }
    }

    fn resolve(
        &self,
        current_path: Option<&Path>,
        target: &str,
    ) -> Option<PathBuf> {
        if target.is_empty() {
            return current_path.map(Path::to_path_buf);
        }
        let candidate = target_to_markdown_path(target)?;
        if self.relative_paths.contains(&candidate) {
            return Some(candidate);
        }
        if target.contains('/') || target.contains('\\') {
            return None;
        }
        let basename = target
            .strip_suffix(".md")
            .or_else(|| target.strip_suffix(".MD"))
            .unwrap_or(target)
            .to_lowercase();
        self.basename_paths
            .get(&basename)
            .and_then(|path| path.clone())
    }
}

fn markdown_basename(path: &Path) -> Option<&str> {
    let name = path.file_name()?.to_str()?;
    name.strip_suffix(".md")
        .or_else(|| name.strip_suffix(".MD"))
        .or(Some(name))
}

fn target_to_markdown_path(target: &str) -> Option<PathBuf> {
    let mut path = PathBuf::new();
    for component in Path::new(target).components() {
        match component {
            Component::Normal(part) => path.push(part),
            _ => return None,
        }
    }
    if !path
        .extension()
        .and_then(OsStr::to_str)
        .is_some_and(|extension| extension.eq_ignore_ascii_case("md"))
    {
        let file_name = path.file_name()?.to_os_string();
        path.set_file_name(format!("{}.md", file_name.to_string_lossy()));
    }
    Some(path)
}

fn task_blocks(files: &[FileScan]) -> BTreeMap<(PathBuf, String), Vec<char>> {
    let mut blocks = BTreeMap::new();
    for file in files {
        for task in &file.tasks {
            if let Some(block_id) = &task.block_id {
                blocks
                    .entry((file.relative_path.clone(), block_id.clone()))
                    .or_insert_with(Vec::new)
                    .push(task.status);
            }
        }
    }
    blocks
}

fn dependency_edges(
    files: &[FileScan],
    note_index: &NoteIndex,
    task_blocks: &BTreeMap<(PathBuf, String), Vec<char>>,
    unresolved: &mut Vec<UnresolvedReference>,
) -> BTreeMap<(PathBuf, String), BTreeSet<(PathBuf, String)>> {
    let mut edges: BTreeMap<(PathBuf, String), BTreeSet<(PathBuf, String)>> =
        BTreeMap::new();
    for file in files {
        if !file.tasks.iter().any(|task| task.block_id.is_some()) {
            continue;
        }
        let lines = logical_lines(&file.contents);
        let fenced = fenced_lines(&lines, 0..lines.len());
        for task in &file.tasks {
            let Some(source_block_id) = &task.block_id else {
                continue;
            };
            let source = (file.relative_path.clone(), source_block_id.clone());
            let source_indent =
                leading_indentation_width(lines[task.line_index]);
            for line_index in task.line_index + 1..lines.len() {
                let line = lines[line_index];
                if line.trim().is_empty() {
                    continue;
                }
                if fenced.contains(&line_index) {
                    continue;
                }
                let indentation = leading_indentation_width(line);
                if indentation <= source_indent {
                    break;
                }
                if nearest_parent_list_item(&lines, line_index)
                    != Some(task.line_index)
                {
                    continue;
                }
                let Some(reference) = sole_transcluded_block_reference(line)
                else {
                    continue;
                };
                let Some(target_path) = note_index.resolve(
                    Some(&file.relative_path),
                    reference.target.trim(),
                ) else {
                    unresolved.push(UnresolvedReference {
                        target: reference.target,
                        block_id: reference.block_id,
                        reason: format!(
                            "dependency from {}:{} did not resolve uniquely",
                            display_path(&file.relative_path),
                            task.line_index + 1
                        ),
                    });
                    continue;
                };
                let target = (target_path.clone(), reference.block_id.clone());
                if !task_blocks.contains_key(&target) {
                    unresolved.push(UnresolvedReference {
                        target: reference.target,
                        block_id: reference.block_id,
                        reason: format!(
                            "dependency from {}:{} resolved to {}, which has no matching task block",
                            display_path(&file.relative_path),
                            task.line_index + 1,
                            display_path(&target_path)
                        ),
                    });
                    continue;
                }
                edges.entry(source.clone()).or_default().insert(target);
            }
        }
    }
    edges
}

fn desired_statuses(
    direct: &BTreeSet<(PathBuf, String)>,
    edges: &BTreeMap<(PathBuf, String), BTreeSet<(PathBuf, String)>>,
    task_blocks: &BTreeMap<(PathBuf, String), Vec<char>>,
) -> BTreeMap<(PathBuf, String), RankedStatus> {
    let mut desired = BTreeMap::new();
    let mut queue = direct.iter().cloned().collect::<VecDeque<_>>();
    for identity in direct {
        desired.insert(
            identity.clone(),
            strongest_current_status(
                task_blocks.get(identity).map(Vec::as_slice),
            )
            .unwrap_or(RankedStatus::Next)
            .max(RankedStatus::Next),
        );
    }
    while let Some(source) = queue.pop_front() {
        let source_status = desired[&source];
        let Some(targets) = edges.get(&source) else {
            continue;
        };
        for target in targets {
            let target_status = strongest_current_status(
                task_blocks.get(target).map(Vec::as_slice),
            )
            .unwrap_or(source_status)
            .max(source_status);
            let should_update = desired
                .get(target)
                .is_none_or(|current| *current < target_status);
            if should_update {
                desired.insert(target.clone(), target_status);
                queue.push_back(target.clone());
            }
        }
    }
    desired
}

fn strongest_current_status(statuses: Option<&[char]>) -> Option<RankedStatus> {
    statuses
        .into_iter()
        .flatten()
        .filter_map(|status| RankedStatus::from_checkbox(*status))
        .max()
}

fn leading_indentation_width(line: &str) -> usize {
    line.bytes()
        .take_while(|byte| matches!(byte, b' ' | b'\t'))
        .fold(0, |width, byte| {
            if byte == b'\t' {
                width + (4 - width % 4)
            } else {
                width + 1
            }
        })
}

fn nearest_parent_list_item(
    lines: &[&str],
    child_line: usize,
) -> Option<usize> {
    let child_indent = leading_indentation_width(lines.get(child_line)?);
    for line_index in (0..child_line).rev() {
        let line = lines[line_index];
        if line.trim().is_empty()
            || leading_indentation_width(line) >= child_indent
        {
            continue;
        }
        let byte_indent = line
            .bytes()
            .take_while(|byte| matches!(byte, b' ' | b'\t'))
            .count();
        if after_list_marker(line, byte_indent).is_some() {
            return Some(line_index);
        }
    }
    None
}

fn sole_transcluded_block_reference(line: &str) -> Option<RawReference> {
    let byte_indent = line
        .bytes()
        .take_while(|byte| matches!(byte, b' ' | b'\t'))
        .count();
    let marker_end = after_list_marker(line, byte_indent)?;
    let body = line[marker_end..].trim();
    let inside = body.strip_prefix("![[")?.strip_suffix("]]")?;
    if inside.contains('|') {
        return None;
    }
    let fragment = inside.find("#^")?;
    let target = inside[..fragment].trim();
    let block_id = inside[fragment + 2..].trim();
    (!block_id.is_empty()
        && block_id.bytes().all(collect_done::is_block_id_byte))
    .then(|| RawReference {
        target: target.to_string(),
        block_id: block_id.to_string(),
    })
}

fn change_item(
    file: &FileScan,
    task: &TaskLine,
    dependency: bool,
) -> ChangeItem {
    ChangeItem {
        path: display_path(&file.relative_path),
        line_number: task.line_index + 1,
        block_id: task.block_id.clone().unwrap_or_default(),
        description: task.description.clone(),
        dependency,
    }
}

fn dependency_status_change(
    file: &FileScan,
    task: &TaskLine,
    to: char,
    state: &TaskDependencyState,
) -> DependencyStatusChange {
    DependencyStatusChange {
        path: display_path(&file.relative_path),
        line_number: task.line_index + 1,
        block_id: task.block_id.clone().unwrap_or_default(),
        description: task.description.clone(),
        from: task.status,
        to,
        open_dependency_ids: state.open_dependency_ids.clone(),
        unresolved_dependency_ids: state.unresolved_dependency_ids.clone(),
    }
}

fn display_path(path: &Path) -> String {
    path.components()
        .filter_map(|component| match component {
            Component::Normal(part) => Some(part.to_string_lossy()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/")
}

fn compose_outputs(
    files: &[FileScan],
    changes: &[PlannedChange],
    daily_path: &Path,
    daily_contents: &str,
    pomodoro_model: &PomodoroModel,
    structural_plan: &StructuralPlan,
) -> Vec<(PathBuf, String)> {
    let mut by_file: BTreeMap<usize, Vec<&PlannedChange>> = BTreeMap::new();
    for change in changes {
        by_file.entry(change.file_index).or_default().push(change);
    }
    let mut updated = BTreeMap::new();
    for (file_index, mut file_changes) in by_file {
        let file = &files[file_index];
        file_changes.sort_by_key(|change| change.status_byte_offset);
        let mut contents = file.contents.clone();
        for change in file_changes.into_iter().rev() {
            let offset = change.status_byte_offset;
            let status_len = contents[offset..]
                .chars()
                .next()
                .map(char::len_utf8)
                .unwrap_or(1);
            contents.replace_range(
                offset..offset + status_len,
                &change.replacement.to_string(),
            );
        }
        updated.insert(file_index, contents);
    }

    let canonical_daily_path = daily_path.canonicalize().ok();
    let daily_file_index = files.iter().position(|file| {
        file.path == daily_path
            || canonical_daily_path.as_ref().is_some_and(|daily| {
                file.path.canonicalize().ok().as_ref() == Some(daily)
            })
    });
    let daily_base = daily_file_index
        .and_then(|index| updated.get(&index))
        .map(String::as_str)
        .unwrap_or(daily_contents);
    let updated_daily =
        apply_structural_plan(daily_base, pomodoro_model, structural_plan);
    let external_daily = if let Some(index) = daily_file_index {
        updated.insert(index, updated_daily);
        None
    } else {
        Some(updated_daily)
    };

    let mut outputs = updated
        .into_iter()
        .filter_map(|(index, contents)| {
            (contents != files[index].contents)
                .then(|| (files[index].path.clone(), contents))
        })
        .collect::<Vec<_>>();
    if let Some(updated_daily) = external_daily
        && updated_daily != daily_contents
    {
        outputs.push((daily_path.to_path_buf(), updated_daily));
    }
    outputs
}

fn apply_outputs(outputs: &[(PathBuf, String)]) -> Result<(), SyncError> {
    for (path, contents) in outputs {
        atomic_write(path, contents)
            .map_err(|error| SyncError::io("write note", path, error))?;
    }
    Ok(())
}

fn atomic_write(path: &Path, contents: &str) -> io::Result<()> {
    let file_name = path.file_name().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("path has no file name: {}", path.display()),
        )
    })?;
    let mut temp_name = OsString::from(".");
    temp_name.push(file_name);
    temp_name.push(format!(".{}.tmp", process::id()));
    let temp_path = path.with_file_name(temp_name);
    let _ = fs::remove_file(&temp_path);
    fs::write(&temp_path, contents)?;
    fs::rename(&temp_path, path).inspect_err(|_| {
        let _ = fs::remove_file(&temp_path);
    })
}

fn print_result(result: &SyncResult, format: OutputFormat) {
    match format {
        OutputFormat::Json => println!(
            "{}",
            serde_json::to_string(result).unwrap_or_else(|error| {
                panic!("serialize {COMMAND_NAME} result: {error}")
            })
        ),
        OutputFormat::Human => print_human_result(result),
    }
    print_warnings(result);
}

fn print_human_result(result: &SyncResult) {
    let styler = Styler::detect();
    let change_count = result.marked_next.len()
        + result.marked_in_progress.len()
        + result.cleared.len()
        + result.marked_blocked.len()
        + result.unblocked.len()
        + result.struck_completed_references.len()
        + result.moved_completed_references.len()
        + result.marker_added_references.len()
        + result.marker_removed_references.len()
        + result.removed_canceled_references.len()
        + result.removed_duplicate_lines.len();
    let prefix = if result.dry_run {
        styler.success_prefix(true)
    } else {
        styler.green("\u{2713}")
    };
    if change_count == 0 {
        println!(
            "{prefix} {COMMAND_NAME}  {} \u{2014} already in sync, no changes",
            styler.cyan(&result.daily_file)
        );
        return;
    }

    println!(
        "{prefix} {COMMAND_NAME}  {}",
        styler.cyan(&result.daily_file)
    );
    println!(
        "  {} open pomodoros \u{b7} {} direct references \u{b7} {} dependency references \u{b7} {} files scanned",
        result.open_pomodoros,
        result.references,
        result.dependency_references,
        result.scanned_files
    );
    print_change_section(
        &styler,
        if result.dry_run {
            "would mark next"
        } else {
            "marked next"
        },
        "[ ] \u{2192} [*]",
        &result.marked_next,
        true,
    );
    print_change_section(
        &styler,
        if result.dry_run {
            "would mark in progress"
        } else {
            "marked in progress"
        },
        "[ ] or [*] -> [/]",
        &result.marked_in_progress,
        true,
    );
    print_change_section(
        &styler,
        if result.dry_run {
            "would clear"
        } else {
            "cleared"
        },
        "[*] \u{2192} [ ]",
        &result.cleared,
        false,
    );
    print_dependency_status_section(
        &styler,
        true,
        if result.dry_run {
            "would mark blocked"
        } else {
            "marked blocked"
        },
        &result.marked_blocked,
    );
    print_dependency_status_section(
        &styler,
        false,
        if result.dry_run {
            "would unblock"
        } else {
            "unblocked"
        },
        &result.unblocked,
    );
    print_completed_reference_sections(result);
    print_marker_reference_sections(result);
    print_canceled_reference_section(result);
    print_duplicate_line_section(result);
    if result.kept_next > 0 || result.kept_in_progress > 0 {
        println!();
        println!(
            "  kept {} already next \u{b7} {} in progress",
            result.kept_next, result.kept_in_progress
        );
    }
    println!(
        "Summary: {} marked next, {} marked in progress, {} cleared, {} blocked, {} unblocked, {} struck, {} moved, {} marked, {} unmarked, {} canceled-reference triggers, {} duplicate-line removals",
        result.marked_next.len(),
        result.marked_in_progress.len(),
        result.cleared.len(),
        result.marked_blocked.len(),
        result.unblocked.len(),
        result.struck_completed_references.len(),
        result.moved_completed_references.len(),
        result.marker_added_references.len(),
        result.marker_removed_references.len(),
        result.removed_canceled_references.len(),
        result.removed_duplicate_lines.len()
    );
}

fn print_dependency_status_section(
    styler: &Styler,
    blocking: bool,
    heading: &str,
    changes: &[DependencyStatusChange],
) {
    if changes.is_empty() {
        return;
    }
    println!();
    println!("  {heading}");
    let description_width = changes
        .iter()
        .map(|change| display_width(&change.description))
        .max()
        .unwrap_or(0);
    for change in changes {
        let transition = format!("[{}] \u{2192} [{}]", change.from, change.to);
        let transition = if blocking {
            styler.yellow(&transition)
        } else {
            styler.green(&transition)
        };
        let description = pad_right(&change.description, description_width);
        let dependencies = if blocking {
            format!(" (open: {})", change.open_dependency_ids.join(", "))
        } else if change.unresolved_dependency_ids.is_empty() {
            String::new()
        } else {
            format!(
                " (unresolved: {})",
                change.unresolved_dependency_ids.join(", ")
            )
        };
        println!(
            "    {transition}  {description}  {}{}{}",
            styler.cyan(&change.path),
            if change.block_id.is_empty() {
                String::new()
            } else {
                format!(" ^{}", change.block_id)
            },
            dependencies
        );
    }
}

fn print_duplicate_line_section(result: &SyncResult) {
    if result.removed_duplicate_lines.is_empty() {
        return;
    }
    println!();
    println!(
        "  {} duplicate task-link lines",
        if result.dry_run {
            "would remove"
        } else {
            "removed"
        }
    );
    for item in &result.removed_duplicate_lines {
        let identities = item
            .duplicate_tasks
            .iter()
            .map(|task| format!("{}#^{}", task.path, task.block_id))
            .collect::<Vec<_>>()
            .join(", ");
        println!(
            "    line {}  {}  {}  {}",
            item.line_number,
            item.line.trim(),
            item.pomodoro,
            identities
        );
    }
}

fn print_canceled_reference_section(result: &SyncResult) {
    if result.removed_canceled_references.is_empty() {
        return;
    }
    println!();
    println!(
        "  {} list items containing canceled task references",
        if result.dry_run {
            "would remove"
        } else {
            "removed"
        }
    );
    for item in &result.removed_canceled_references {
        println!(
            "    [[{}#^{}]]  line {}  {}",
            item.target, item.block_id, item.line_number, item.pomodoro
        );
    }
}

fn print_marker_reference_sections(result: &SyncResult) {
    for (items, dry_heading, heading, marker) in [
        (
            &result.marker_added_references,
            "would mark",
            "marked",
            "🍅",
        ),
        (
            &result.marker_removed_references,
            "would unmark",
            "unmarked",
            "",
        ),
    ] {
        if items.is_empty() {
            continue;
        }
        println!();
        println!(
            "  {} Pomodoro references",
            if result.dry_run { dry_heading } else { heading }
        );
        for item in items {
            println!(
                "    {}[[{}#^{}]]  {}",
                if marker.is_empty() { "" } else { "🍅 " },
                item.target,
                item.block_id,
                item.pomodoro
            );
        }
    }
}

fn print_completed_reference_sections(result: &SyncResult) {
    if !result.struck_completed_references.is_empty() {
        println!();
        println!(
            "  {} completed references",
            if result.dry_run {
                "would retire"
            } else {
                "retired"
            }
        );
        for item in &result.struck_completed_references {
            println!(
                "    ~~[[{}#^{}]]~~  {}{}",
                item.target,
                item.block_id,
                item.pomodoro,
                if item.removed_embed {
                    " (removed embed)"
                } else {
                    ""
                }
            );
        }
    }
    if !result.moved_completed_references.is_empty() {
        println!();
        println!(
            "  {} completed references",
            if result.dry_run {
                "would move"
            } else {
                "moved"
            }
        );
        for item in &result.moved_completed_references {
            println!(
                "    [[{}#^{}]]  {} -> {}",
                item.target,
                item.block_id,
                item.source_pomodoro,
                item.destination_pomodoro
            );
        }
    }
}

fn print_change_section(
    styler: &Styler,
    heading: &str,
    transition: &str,
    changes: &[ChangeItem],
    promotion: bool,
) {
    if changes.is_empty() {
        return;
    }
    println!();
    println!("  {heading}");
    let description_width = changes
        .iter()
        .map(|change| display_width(&change.description))
        .max()
        .unwrap_or(0);
    for change in changes {
        let transition = if promotion {
            styler.green(transition)
        } else {
            styler.yellow(transition)
        };
        let description = pad_right(&change.description, description_width);
        println!(
            "    {transition}  {description}  {} ^{}{}",
            styler.cyan(&change.path),
            change.block_id,
            if change.dependency {
                " (dependency)"
            } else {
                ""
            }
        );
    }
}

fn print_warnings(result: &SyncResult) {
    let styler = Styler::detect();
    for warning in &result.unresolved_references {
        eprintln!(
            "{}: [[{}#^{}]] \u{2014} {}",
            styler.warning_prefix(),
            warning.target,
            warning.block_id,
            warning.reason
        );
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SyncError {
    message: String,
}

impl SyncError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }

    fn io(action: &str, path: &Path, error: io::Error) -> Self {
        Self::new(format!("failed to {action} {}: {error}", path.display()))
    }
}

fn print_error(error: SyncError, format: OutputFormat) -> i32 {
    match format {
        OutputFormat::Human => eprintln!("{COMMAND_NAME}: {}", error.message),
        OutputFormat::Json => {
            println!("{}", json!({ "ok": false, "error": error.message }))
        }
    }
    1
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reference(target: &str, block_id: &str) -> RawReference {
        RawReference {
            target: target.to_string(),
            block_id: block_id.to_string(),
        }
    }

    fn identity(block_id: &str) -> (PathBuf, String) {
        (PathBuf::from("tasks.md"), block_id.to_string())
    }

    fn test_settings() -> TasksSettings {
        TasksSettings {
            global_filter: "#task".to_string(),
            done_statuses: BTreeSet::from(['x', 'X']),
            status_types: BTreeMap::from([
                (' ', TaskStatusType::Todo),
                ('x', TaskStatusType::Done),
                ('X', TaskStatusType::Done),
                ('/', TaskStatusType::InProgress),
                ('*', TaskStatusType::OnHold),
                ('?', TaskStatusType::OnHold),
                ('-', TaskStatusType::Cancelled),
            ]),
            status_definitions: vec![TaskStatusDefinition {
                symbol: "?".to_string(),
                name: "Blocked".to_string(),
                next_status_symbol: " ".to_string(),
                available_as_command: true,
                status_type: TaskStatusType::OnHold,
            }],
            status_settings_error: None,
        }
    }

    fn resolved(
        values: &[(&str, &str, Vec<char>)],
    ) -> BTreeMap<RawReference, ResolvedReference> {
        values
            .iter()
            .map(|(target, block_id, statuses)| {
                (
                    reference(target, block_id),
                    ResolvedReference {
                        path: PathBuf::from(format!("{target}.md")),
                        statuses: statuses.clone(),
                    },
                )
            })
            .collect()
    }

    fn resolved_paths(
        values: &[(&str, &str, &str, Vec<char>)],
    ) -> BTreeMap<RawReference, ResolvedReference> {
        values
            .iter()
            .map(|(target, block_id, path, statuses)| {
                (
                    reference(target, block_id),
                    ResolvedReference {
                        path: PathBuf::from(path),
                        statuses: statuses.clone(),
                    },
                )
            })
            .collect()
    }

    #[test]
    fn extracts_only_block_links_under_open_pomodoros() {
        let lines = [
            "- [ ] Open (0900-0930)",
            "  - [[dev#^one]] and [[Projects/Alpha.md#^two|alias]]",
            "  - ignore [[note]], [[note#Heading]], and [[note|alias #^fake]]",
            "- [x] Closed (0930-1000)",
            "  - [[dev#^closed]]",
        ];
        let model = scan_pomodoros(&lines, 0..lines.len());
        assert_eq!(model.open_pomodoros, 1);
        assert_eq!(
            model.raw_references,
            BTreeSet::from([
                RawReference {
                    target: "Projects/Alpha.md".to_string(),
                    block_id: "two".to_string(),
                },
                RawReference {
                    target: "dev".to_string(),
                    block_id: "one".to_string(),
                },
            ])
        );
    }

    #[test]
    fn duplicate_lines_use_canonical_task_identity_and_first_open_owner() {
        let contents = concat!(
            "- [ ] First\n",
            "  - [[Projects/Alpha#^ship]]\n",
            "  - [[Alpha#^ship|same owner repeat]]\n",
            "  - [[#^daily]]\n",
            "- [ ] Second\n",
            "  - ![[Alpha.md#^ship|embedded duplicate]]\n",
            "  - [[2026/Today#^daily|same-note duplicate]]\n",
            "- [ ] Third\n",
            "  - ~~[[Projects/Alpha.md#^ship]]~~\n",
        );
        let lines = logical_lines(contents);
        let model = scan_pomodoros(&lines, 0..lines.len());
        let resolved = resolved_paths(&[
            ("Projects/Alpha", "ship", "Projects/Alpha.md", vec![' ']),
            ("Alpha", "ship", "Projects/Alpha.md", vec![' ']),
            ("Alpha.md", "ship", "Projects/Alpha.md", vec![' ']),
            ("Projects/Alpha.md", "ship", "Projects/Alpha.md", vec![' ']),
            ("", "daily", "2026/Today.md", vec![' ']),
            ("2026/Today", "daily", "2026/Today.md", vec![' ']),
        ]);

        let removals = plan_duplicate_line_removals(&lines, &model, &resolved);
        assert_eq!(
            removals
                .iter()
                .map(|item| item.line_number)
                .collect::<Vec<_>>(),
            vec![6, 7, 9]
        );
        assert!(removals.iter().all(|item| item.duplicate_tasks.len() == 1));
        assert_eq!(
            removals[0].duplicate_tasks[0],
            DuplicateTaskIdentity {
                path: "Projects/Alpha.md".to_string(),
                block_id: "ship".to_string(),
            }
        );
    }

    #[test]
    fn deleted_conflict_line_cannot_claim_an_unrelated_task() {
        let contents = concat!(
            "- [ ] First\n",
            "  - [[tasks#^alpha]]\n",
            "- [ ] Second\n",
            "  - [[tasks#^alpha]] and [[tasks#^beta]]\n",
            "- [ ] Third\n",
            "  - [[tasks#^beta]]\n",
            "- [ ] Fourth\n",
            "  - [[tasks#^beta]] and [[tasks#^alpha]]\n",
        );
        let lines = logical_lines(contents);
        let model = scan_pomodoros(&lines, 0..lines.len());
        let resolved = resolved(&[
            ("tasks", "alpha", vec![' ']),
            ("tasks", "beta", vec![' ']),
        ]);

        let removals = plan_duplicate_line_removals(&lines, &model, &resolved);
        assert_eq!(
            removals
                .iter()
                .map(|item| item.line_number)
                .collect::<Vec<_>>(),
            vec![4, 8]
        );
        assert_eq!(removals[1].duplicate_tasks.len(), 2);
    }

    #[test]
    fn duplicate_cleanup_ignores_distinct_unresolved_and_ineligible_links() {
        let contents = concat!(
            "- [ ] First with [[Alpha#^same]] on its top-level line\n",
            "  - [[Alpha#^same]]\n",
            "  - ~~[[Alpha#^same]]~~\n",
            "- [ ] Second\n",
            "  - [[Beta#^same]] and [[missing#^same]]\n",
            "  ```md\n",
            "  - [[Alpha#^same]]\n",
            "  ```\n",
            "- [x] Closed\n",
            "  - [[Alpha#^same]]\n",
            "- [-] Cancelled\n",
            "  - [[Alpha#^same]]\n",
        );
        let lines = logical_lines(contents);
        let model = scan_pomodoros(&lines, 0..lines.len());
        let resolved = resolved_paths(&[
            ("Alpha", "same", "Alpha.md", vec![' ']),
            ("Beta", "same", "Beta.md", vec![' ']),
        ]);

        assert!(
            plan_duplicate_line_removals(&lines, &model, &resolved).is_empty()
        );
    }

    #[test]
    fn full_line_deletion_preserves_children_crlf_and_final_line_ending() {
        let contents = concat!(
            "- [ ] First\r\n",
            "  - [[tasks#^alpha]] and [[tasks#^beta]]\r\n",
            "- [ ] Second\r\n",
            "  - authored [[tasks#^alpha]] plus [[tasks#^beta]]\r\n",
            "    - retained child\r\n",
            "- [ ] Third\r\n",
            "  - [[tasks#^gamma]]",
        );
        let lines = logical_lines(contents);
        let model = scan_pomodoros(&lines, 0..lines.len());
        let resolved = resolved(&[
            ("tasks", "alpha", vec![' ']),
            ("tasks", "beta", vec![' ']),
            ("tasks", "gamma", vec![' ']),
        ]);
        let removals = plan_duplicate_line_removals(&lines, &model, &resolved);
        assert_eq!(removals.len(), 1);
        assert_eq!(removals[0].duplicate_tasks.len(), 2);
        let deleted_lines = BTreeSet::from([removals[0].line_number - 1]);
        let plan = plan_structural_changes(
            &model,
            &resolved,
            &BTreeSet::from(['x', 'X']),
            &test_settings().status_types,
            &deleted_lines,
        );

        assert_eq!(
            apply_structural_plan(contents, &model, &plan),
            concat!(
                "- [ ] First\r\n",
                "  - [[tasks#^alpha]] and [[tasks#^beta]]\r\n",
                "- [ ] Second\r\n",
                "    - retained child\r\n",
                "- [ ] Third\r\n",
                "  - [[tasks#^gamma]]",
            )
        );
    }

    #[test]
    fn deleted_completed_duplicate_is_not_retired_moved_or_reinserted() {
        let contents = concat!(
            "- [ ] Current (0900-0930)\n",
            "  - [[tasks#^done]]\n",
            "- [ ] Later\n",
            "  - ![[tasks#^done|duplicate]]\n",
            "    - retained child\n",
        );
        let lines = logical_lines(contents);
        let model = scan_pomodoros(&lines, 0..lines.len());
        let resolved = resolved(&[("tasks", "done", vec!['x'])]);
        let removals = plan_duplicate_line_removals(&lines, &model, &resolved);
        let deleted_lines = removals
            .iter()
            .map(|item| item.line_number - 1)
            .collect::<BTreeSet<_>>();
        let plan = plan_structural_changes(
            &model,
            &resolved,
            &BTreeSet::from(['x', 'X']),
            &test_settings().status_types,
            &deleted_lines,
        );

        assert_eq!(plan.struck.len(), 1);
        assert!(plan.moved.is_empty());
        assert!(!plan.token_edits.contains_key(&3));
        assert_eq!(
            apply_structural_plan(contents, &model, &plan),
            concat!(
                "- [ ] Current (0900-0930)\n",
                "  - ~~[[tasks#^done]]~~\n",
                "- [ ] Later\n",
                "    - retained child\n",
            )
        );
    }

    #[test]
    fn resolves_exact_and_unique_case_insensitive_basenames() {
        let index = NoteIndex::from_paths([
            PathBuf::from("Areas/Home.md"),
            PathBuf::from("Projects/Alpha.md"),
        ]);
        assert_eq!(
            index.resolve(None, "Projects/Alpha"),
            Some(PathBuf::from("Projects/Alpha.md"))
        );
        assert_eq!(
            index.resolve(None, "alpha.md"),
            Some(PathBuf::from("Projects/Alpha.md"))
        );
        assert_eq!(
            index.resolve(None, "HOME"),
            Some(PathBuf::from("Areas/Home.md"))
        );
    }

    #[test]
    fn ambiguous_basename_does_not_resolve() {
        let index = NoteIndex::from_paths([
            PathBuf::from("Areas/Home.md"),
            PathBuf::from("Projects/home.md"),
        ]);
        assert_eq!(index.resolve(None, "home"), None);
    }

    #[test]
    fn dotted_note_names_keep_the_full_basename() {
        let index = NoteIndex::from_paths([
            PathBuf::from("Notes.md"),
            PathBuf::from("Notes.v2.md"),
        ]);
        assert_eq!(
            index.resolve(None, "Notes.v2"),
            Some(PathBuf::from("Notes.v2.md"))
        );
        assert_eq!(
            target_to_markdown_path("Notes.v2"),
            Some(PathBuf::from("Notes.v2.md"))
        );
    }

    #[test]
    fn struck_references_are_retired_and_spans_are_paired() {
        let lines = [
            "- [ ] Current",
            "  - ~~[[dev#^old]]~~",
            "  - ~~before~~[[dev#^live]]~~after~~",
        ];
        let model = scan_pomodoros(&lines, 0..lines.len());
        assert!(!model.raw_references.contains(&reference("dev", "old")));
        assert!(model.raw_references.contains(&reference("dev", "live")));
        let live = &model.bullets[1].links[0];
        assert!(!live.struck);
        assert!(live.retired_unmarked_token.starts_with(" ~~"));
        assert_eq!(live.retired_marked_token, " 🍅 ~~[[dev#^live]]~~ ");
    }

    #[test]
    fn fenced_column_zero_content_does_not_end_dependency_scan() {
        let settings = test_settings();
        let a_contents =
            "- [ ] #task A ^a\n  ```\nnot a list item\n  ```\n  - ![[B#^b]]\n";
        let b_contents = "- [ ] #task B ^b\n";
        let files = vec![
            FileScan {
                path: PathBuf::from("A.md"),
                relative_path: PathBuf::from("A.md"),
                contents: a_contents.to_string(),
                tasks: parse_tasks(a_contents, &settings),
            },
            FileScan {
                path: PathBuf::from("B.md"),
                relative_path: PathBuf::from("B.md"),
                contents: b_contents.to_string(),
                tasks: parse_tasks(b_contents, &settings),
            },
        ];
        let index = NoteIndex::from_paths(
            files.iter().map(|file| file.relative_path.clone()),
        );
        let blocks = task_blocks(&files);
        let edges = dependency_edges(&files, &index, &blocks, &mut Vec::new());
        assert!(edges[&(PathBuf::from("A.md"), "a".to_string())]
            .contains(&(PathBuf::from("B.md"), "b".to_string())));
    }

    #[test]
    fn completed_fallback_does_not_take_mixed_live_bullets() {
        let lines = [
            "- [x] Completed (0800-0830)",
            "- [ ] Untimed",
            "  - [[dev#^done]] and [[dev#^live]]",
        ];
        let model = scan_pomodoros(&lines, 0..lines.len());
        let plan = plan_structural_changes(
            &model,
            &resolved(&[
                ("dev", "done", vec!['x']),
                ("dev", "live", vec![' ']),
            ]),
            &BTreeSet::from(['x', 'X']),
            &test_settings().status_types,
            &BTreeSet::new(),
        );
        assert!(plan.moves.is_empty());
        assert_eq!(plan.struck.len(), 1);
    }

    #[test]
    fn parses_task_markers_and_preserves_status_offsets() {
        let contents = concat!(
            "  1. [ ] #task Todo ^todo\r\n",
            "* [*] #task Next ^next\r\n",
            "+ [/] #task Working ^work\r\n",
            "- [*] not a task ^ignored\r\n",
        );
        let tasks = parse_tasks(contents, &test_settings());
        assert_eq!(tasks.len(), 3);
        assert_eq!(tasks[0].status, ' ');
        assert_eq!(tasks[0].block_id.as_deref(), Some("todo"));
        assert_eq!(tasks[0].description, "Todo");
        assert_eq!(&contents[tasks[1].status_byte_offset..][..1], "*");
        assert_eq!(tasks[2].status, '/');
    }

    #[test]
    fn parses_bracket_and_parenthesized_task_dependency_metadata() {
        let contents = concat!(
            "- [ ] #task Bracket [id:: alpha] [dependsOn:: root, other] ^a\n",
            "- [?] #task Parenthesized (id:: beta) (dependsOn:: root) #tag ^b\n",
            "- [ ] #task Invalid [id:: bad value] [dependsOn:: root, bad value] ^c\n",
        );
        let tasks = parse_tasks(contents, &test_settings());
        assert_eq!(tasks[0].task_id.as_deref(), Some("alpha"));
        assert_eq!(tasks[0].depends_on, ["root", "other"]);
        assert_eq!(tasks[1].task_id.as_deref(), Some("beta"));
        assert_eq!(tasks[1].depends_on, ["root"]);
        assert_eq!(tasks[1].status_type, TaskStatusType::OnHold);
        assert!(tasks[1].status_recognized);
        assert!(tasks[2].task_id.is_none());
        assert!(tasks[2].depends_on.is_empty());
    }

    #[test]
    fn task_dependency_index_matches_tasks_duplicate_and_missing_id_semantics()
    {
        let mut settings = test_settings();
        settings.status_types.insert('~', TaskStatusType::NonTask);
        let contents = concat!(
            "- [x] #task Duplicate done [id:: duplicate] ^done\n",
            "- [ ] #task Duplicate open [id:: duplicate] ^open\n",
            "- [x] #task Closed [id:: closed] ^closed\n",
            "- [~] #task Non-task [id:: non-task] ^non-task\n",
            "- [!] #task Unknown [id:: unknown] ^unknown\n",
            "- [ ] #task Self [id:: self] [dependsOn:: self] ^self\n",
            "- [ ] #task Parent [dependsOn:: duplicate, closed, non-task, unknown, missing] ^parent\n",
        );
        let files = vec![FileScan {
            path: PathBuf::from("tasks.md"),
            relative_path: PathBuf::from("tasks.md"),
            contents: contents.to_string(),
            tasks: parse_tasks(contents, &settings),
        }];
        let states = task_dependency_states(&files);
        assert_eq!(states[&(0, 5)].open_dependency_ids, ["self"]);
        assert_eq!(states[&(0, 6)].open_dependency_ids, ["duplicate"]);
        assert_eq!(states[&(0, 6)].unresolved_dependency_ids, ["missing"]);
    }

    #[test]
    fn replacement_changes_only_status_and_preserves_crlf() {
        let contents = "  - [ ] #task Keep everything ^id\r\n";
        let task = parse_tasks(contents, &test_settings()).remove(0);
        let mut changed = contents.to_string();
        changed.replace_range(
            task.status_byte_offset..task.status_byte_offset + 1,
            "*",
        );
        assert_eq!(changed, "  - [*] #task Keep everything ^id\r\n");
    }

    #[test]
    fn transition_matrix_promotes_monotonically_and_clears_only_unreferenced_next(
    ) {
        assert_eq!(
            transition(' ', Some(RankedStatus::Next)),
            Transition::MarkNext
        );
        assert_eq!(
            transition(' ', Some(RankedStatus::InProgress)),
            Transition::MarkInProgress
        );
        assert_eq!(
            transition('*', Some(RankedStatus::InProgress)),
            Transition::MarkInProgress
        );
        assert_eq!(transition(' ', None), Transition::Unchanged);
        assert_eq!(transition('*', None), Transition::Clear);
        assert_eq!(
            transition('*', Some(RankedStatus::Next)),
            Transition::KeptNext
        );
        assert_eq!(
            transition('/', Some(RankedStatus::Next)),
            Transition::KeptInProgress
        );
        assert_eq!(transition('/', None), Transition::Unchanged);
        for status in ['x', 'X', '-', '!'] {
            assert_eq!(
                transition(status, Some(RankedStatus::InProgress)),
                Transition::Unchanged
            );
            assert_eq!(transition(status, None), Transition::Unchanged);
        }
    }

    #[test]
    fn blocked_transition_precedence_and_recovery_are_explicit() {
        let settings = test_settings();
        let ready = parse_tasks("- [ ] #task Ready\n", &settings).remove(0);
        let blocked = parse_tasks("- [?] #task Blocked\n", &settings).remove(0);
        let done = parse_tasks("- [x] #task Done\n", &settings).remove(0);
        assert_eq!(
            task_transition(&ready, Some(RankedStatus::InProgress), true),
            Transition::MarkBlocked
        );
        assert_eq!(
            task_transition(&blocked, Some(RankedStatus::InProgress), true),
            Transition::Unchanged
        );
        assert_eq!(
            task_transition(&blocked, Some(RankedStatus::InProgress), false),
            Transition::Unblock(RankedStatus::InProgress)
        );
        assert_eq!(
            task_transition(&blocked, None, false),
            Transition::Unblock(RankedStatus::Ready)
        );
        assert_eq!(
            task_transition(&done, Some(RankedStatus::InProgress), true),
            Transition::Unchanged
        );
    }

    #[test]
    fn desired_statuses_merge_parents_and_propagate_stronger_intermediates_through_cycles(
    ) {
        let ready_root = identity("ready-root");
        let working_root = identity("working-root");
        let shared = identity("shared");
        let stronger_mid = identity("stronger-mid");
        let leaf = identity("leaf");
        let direct = BTreeSet::from([ready_root.clone(), working_root.clone()]);
        let edges = BTreeMap::from([
            (ready_root.clone(), BTreeSet::from([shared.clone()])),
            (working_root.clone(), BTreeSet::from([shared.clone()])),
            (shared.clone(), BTreeSet::from([stronger_mid.clone()])),
            (stronger_mid.clone(), BTreeSet::from([leaf.clone()])),
            (leaf.clone(), BTreeSet::from([shared.clone()])),
        ]);
        let task_blocks = BTreeMap::from([
            (ready_root.clone(), vec![' ']),
            (working_root.clone(), vec!['*']),
            (shared.clone(), vec![' ']),
            (stronger_mid.clone(), vec!['/']),
            (leaf.clone(), vec!['*']),
        ]);

        let desired = desired_statuses(&direct, &edges, &task_blocks);

        assert_eq!(desired[&ready_root], RankedStatus::Next);
        assert_eq!(desired[&working_root], RankedStatus::Next);
        for identity in [&shared, &stronger_mid, &leaf] {
            assert_eq!(desired[identity], RankedStatus::InProgress);
        }
    }

    #[test]
    fn parses_embedded_alias_and_mixed_block_links() {
        let links = block_link_occurrences(
            "  - ![[dev#^done|Done alias]] and [[dev#^todo]]",
        );
        assert_eq!(links.len(), 2);
        assert!(links[0].embedded);
        assert_eq!(links[0].reference, reference("dev", "done"));
        assert!(!links[1].embedded);
        assert_eq!(links[1].reference, reference("dev", "todo"));
    }

    #[test]
    fn parses_and_normalizes_pomodoro_marker_prefixes_per_link() {
        let links = block_link_occurrences(
            "  - 🍅   ![[dev#^embedded|Alias]] and 🍅 🍅 ~~[[dev#^done]]~~",
        );
        assert_eq!(links.len(), 2);
        assert_eq!(links[0].marker_count, 1);
        assert_eq!(
            links[0].preserved_marked_token,
            "🍅 ![[dev#^embedded|Alias]]"
        );
        assert_eq!(links[1].marker_count, 2);
        assert_eq!(links[1].retired_marked_token, "🍅 ~~[[dev#^done]]~~");
    }

    #[test]
    fn dependency_reference_requires_a_sole_transcluded_block_link() {
        assert_eq!(
            sole_transcluded_block_reference("  - ![[Projects/A#^dep]]"),
            Some(reference("Projects/A", "dep"))
        );
        for line in [
            "  - [[#^plain]]",
            "  - ![[#^dep|alias]]",
            "  - text ![[#^dep]]",
            "  - ![[#^dep]] trailing",
            "  - ![[#heading]]",
        ] {
            assert_eq!(sole_transcluded_block_reference(line), None, "{line}");
        }
    }

    #[test]
    fn moves_completed_mixed_bullet_subtree_to_current_and_strikes_only_done() {
        let contents = concat!(
            "- [ ] Current (0900-0930)\n",
            "  - Existing child\n",
            "- [ ] Future\n",
            "    - [[dev#^done|Done]] and [[dev#^todo]]\n",
            "      - Nested detail\n",
            "  ```\n",
            "  - [[dev#^fenced]]\n",
            "  ```\n",
        );
        let lines = logical_lines(contents);
        let model = scan_pomodoros(&lines, 0..lines.len());
        assert!(!model.raw_references.contains(&reference("dev", "fenced")));
        let plan = plan_structural_changes(
            &model,
            &resolved(&[
                ("dev", "done", vec!['x']),
                ("dev", "todo", vec![' ']),
            ]),
            &BTreeSet::from(['x', 'X']),
            &test_settings().status_types,
            &BTreeSet::new(),
        );
        let updated = apply_structural_plan(contents, &model, &plan);
        assert_eq!(
            updated,
            concat!(
                "- [ ] Current (0900-0930)\n",
                "  - Existing child\n",
                "  - ~~[[dev#^done|Done]]~~ and [[dev#^todo]]\n",
                "    - Nested detail\n",
                "- [ ] Future\n",
                "  ```\n",
                "  - [[dev#^fenced]]\n",
                "  ```\n",
            )
        );
        assert_eq!(plan.struck.len(), 1);
        assert_eq!(plan.moved.len(), 1);
    }

    #[test]
    fn repairs_completed_pomodoro_links_in_place_and_is_idempotent() {
        let contents = concat!(
            "- [x] Historical (0800-0830)\r\n",
            "  - 🍅 ![[dev#^done|Embedded]] and [[dev#^done|Plain]]\r\n",
            "  - 🍅 ~~![[dev#^done|Stale]]~~ and ~~[[dev#^done|Canonical]]~~\r\n",
            "- [ ] Current (0900-0930)\r\n",
        );
        let lines = logical_lines(contents);
        let model = scan_pomodoros(&lines, 0..lines.len());
        assert!(model.raw_references.is_empty());
        assert_eq!(model.all_references.len(), 1);
        let plan = plan_structural_changes(
            &model,
            &resolved(&[("dev", "done", vec!['x'])]),
            &BTreeSet::from(['x', 'X']),
            &test_settings().status_types,
            &BTreeSet::new(),
        );
        assert!(plan.moves.is_empty());
        assert_eq!(plan.struck.len(), 3);
        let updated = apply_structural_plan(contents, &model, &plan);
        assert_eq!(
            updated,
            concat!(
                "- [x] Historical (0800-0830)\r\n",
                "  - ~~[[dev#^done|Embedded]]~~ and 🍅 ~~[[dev#^done|Plain]]~~\r\n",
                "  - ~~[[dev#^done|Stale]]~~ and ~~[[dev#^done|Canonical]]~~\r\n",
                "- [ ] Current (0900-0930)\r\n",
            )
        );
        assert_eq!(plan.marker_added.len(), 1);
        assert_eq!(plan.marker_removed.len(), 2);
        let updated_lines = logical_lines(&updated);
        let updated_model =
            scan_pomodoros(&updated_lines, 0..updated_lines.len());
        let second = plan_structural_changes(
            &updated_model,
            &resolved(&[("dev", "done", vec!['x'])]),
            &BTreeSet::from(['x', 'X']),
            &test_settings().status_types,
            &BTreeSet::new(),
        );
        assert!(
            second.token_edits.is_empty(),
            "unexpected second-pass edits: {:?}",
            second.token_edits
        );
        assert!(second.marker_added.is_empty());
        assert!(second.marker_removed.is_empty());
        assert_eq!(
            apply_structural_plan(&updated, &updated_model, &second),
            updated
        );
    }

    #[test]
    fn repairs_markers_by_owner_and_marks_completed_fallback_moves() {
        let contents = concat!(
            "- [x] Done\n",
            "  - [[dev#^live]] and 🍅 🍅 ~~[[dev#^done]]~~\n",
            "- [ ] Future\n",
            "  - 🍅 [[dev#^open]]\n",
            "  - [[dev#^done]]\n",
            "- [-] Cancelled\n",
            "  - 🍅 🍅 [[dev#^cancelled]]\n",
        );
        let lines = logical_lines(contents);
        let model = scan_pomodoros(&lines, 0..lines.len());
        let plan = plan_structural_changes(
            &model,
            &resolved(&[
                ("dev", "live", vec![' ']),
                ("dev", "done", vec!['x']),
                ("dev", "open", vec![' ']),
            ]),
            &BTreeSet::from(['x', 'X']),
            &test_settings().status_types,
            &BTreeSet::new(),
        );
        assert_eq!(plan.marker_added.len(), 2);
        assert_eq!(plan.marker_removed.len(), 2);
        let updated = apply_structural_plan(contents, &model, &plan);
        assert_eq!(
            updated,
            concat!(
                "- [x] Done\n",
                "  - 🍅 [[dev#^live]] and 🍅 ~~[[dev#^done]]~~\n",
                "  - 🍅 ~~[[dev#^done]]~~\n",
                "- [ ] Future\n",
                "  - [[dev#^open]]\n",
                "- [-] Cancelled\n",
                "  - 🍅 🍅 [[dev#^cancelled]]\n",
            )
        );
    }

    #[test]
    fn conflicting_duplicate_statuses_are_not_normalized() {
        let contents = "- [ ] Future\n  - [[dev#^duplicate]]\n";
        let lines = logical_lines(contents);
        let model = scan_pomodoros(&lines, 0..lines.len());
        let plan = plan_structural_changes(
            &model,
            &resolved(&[("dev", "duplicate", vec!['x', ' '])]),
            &BTreeSet::from(['x', 'X']),
            &test_settings().status_types,
            &BTreeSet::new(),
        );
        assert!(plan.token_edits.is_empty());
        assert!(plan.moves.is_empty());
        assert_eq!(apply_structural_plan(contents, &model, &plan), contents);
    }

    #[test]
    fn canceled_reference_removal_deletes_complete_mixed_content_items() {
        let contents = concat!(
            "- [ ] Open (0900-0930) with [[tasks#^plain]]\r\n",
            "  - start [[tasks#^plain]] middle ![[tasks#^custom|Alias]] end\r\n",
            "  - 🍅 [[tasks#^marked]] and ~~[[tasks#^struck]]~~ and [[tasks#^done]] and [[tasks#^live]]\r\n",
            "  - [[tasks#^all-canceled]] and [[tasks#^mixed]] and [[missing#^unknown]]\r\n",
            "  ```md\r\n",
            "  - [[tasks#^plain]]\r\n",
            "  ```\r\n",
            "- [x] Completed\r\n",
            "  - 🍅 [[tasks#^plain]]\r\n",
            "- [-] Canceled\r\n",
            "  - [[tasks#^custom]]",
        );
        let lines = logical_lines(contents);
        let model = scan_pomodoros(&lines, 0..lines.len());
        let resolved = resolved(&[
            ("tasks", "plain", vec!['-']),
            ("tasks", "custom", vec!['C']),
            ("tasks", "marked", vec!['-']),
            ("tasks", "struck", vec!['-']),
            ("tasks", "done", vec!['x']),
            ("tasks", "live", vec![' ']),
            ("tasks", "all-canceled", vec!['-', 'C']),
            ("tasks", "mixed", vec!['-', ' ']),
        ]);
        let mut settings = test_settings();
        settings.status_types.insert('C', TaskStatusType::Cancelled);
        let plan = plan_structural_changes(
            &model,
            &resolved,
            &settings.done_statuses,
            &settings.status_types,
            &BTreeSet::new(),
        );

        assert_eq!(
            plan.removed_canceled,
            vec![
                RemovedCanceledReference {
                    target: "tasks".to_string(),
                    block_id: "plain".to_string(),
                    line_number: 2,
                    pomodoro: "- [ ] Open (0900-0930) with [[tasks#^plain]]"
                        .to_string(),
                },
                RemovedCanceledReference {
                    target: "tasks".to_string(),
                    block_id: "custom".to_string(),
                    line_number: 2,
                    pomodoro: "- [ ] Open (0900-0930) with [[tasks#^plain]]"
                        .to_string(),
                },
                RemovedCanceledReference {
                    target: "tasks".to_string(),
                    block_id: "marked".to_string(),
                    line_number: 3,
                    pomodoro: "- [ ] Open (0900-0930) with [[tasks#^plain]]"
                        .to_string(),
                },
                RemovedCanceledReference {
                    target: "tasks".to_string(),
                    block_id: "struck".to_string(),
                    line_number: 3,
                    pomodoro: "- [ ] Open (0900-0930) with [[tasks#^plain]]"
                        .to_string(),
                },
                RemovedCanceledReference {
                    target: "tasks".to_string(),
                    block_id: "all-canceled".to_string(),
                    line_number: 4,
                    pomodoro: "- [ ] Open (0900-0930) with [[tasks#^plain]]"
                        .to_string(),
                },
            ]
        );
        assert!(plan.struck.is_empty());
        assert!(plan.moved.is_empty());
        assert!(plan.token_edits.is_empty());
        assert!(plan.marker_added.is_empty());
        assert!(plan.marker_removed.is_empty());
        let updated = apply_structural_plan(contents, &model, &plan);
        assert_eq!(
            updated,
            concat!(
                "- [ ] Open (0900-0930) with [[tasks#^plain]]\r\n",
                "  ```md\r\n",
                "  - [[tasks#^plain]]\r\n",
                "  ```\r\n",
                "- [x] Completed\r\n",
                "  - 🍅 [[tasks#^plain]]\r\n",
                "- [-] Canceled\r\n",
                "  - [[tasks#^custom]]",
            )
        );
        let updated_lines = logical_lines(&updated);
        let updated_model =
            scan_pomodoros(&updated_lines, 0..updated_lines.len());
        assert!(!updated_model
            .raw_references
            .contains(&reference("tasks", "plain")));
        assert!(!updated_model
            .raw_references
            .contains(&reference("tasks", "live")));
        assert!(!updated_model
            .raw_references
            .contains(&reference("tasks", "mixed")));
        let second = plan_structural_changes(
            &updated_model,
            &resolved,
            &settings.done_statuses,
            &settings.status_types,
            &BTreeSet::new(),
        );
        assert!(second.removed_canceled.is_empty());
        assert!(second.token_edits.is_empty());
        assert_eq!(
            apply_structural_plan(&updated, &updated_model, &second),
            updated
        );
    }

    #[test]
    fn canceled_subtrees_compose_with_nested_and_moving_bullets() {
        let contents = concat!(
            "- [x] Completed\r\n",
            "- [ ] Future\r\n",
            "  - surviving [[tasks#^live]]\r\n",
            "    - canceled child [[tasks#^canceled]]\r\n",
            "      - nested detail [[tasks#^done]]\r\n",
            "    - surviving child [[tasks#^other]]\r\n",
            "  - canceled parent [[tasks#^canceled]]\r\n",
            "    - redundant [[tasks#^custom]]\r\n",
            "  - moving parent [[tasks#^done]]\r\n",
            "    - omitted [[tasks#^custom]]\r\n",
            "      - omitted detail\r\n",
        );
        let lines = logical_lines(contents);
        let model = scan_pomodoros(&lines, 0..lines.len());
        let resolved = resolved(&[
            ("tasks", "live", vec![' ']),
            ("tasks", "other", vec![' ']),
            ("tasks", "canceled", vec!['-']),
            ("tasks", "custom", vec!['C']),
            ("tasks", "done", vec!['x']),
        ]);
        let mut settings = test_settings();
        settings.status_types.insert('C', TaskStatusType::Cancelled);

        let plan = plan_structural_changes(
            &model,
            &resolved,
            &settings.done_statuses,
            &settings.status_types,
            &BTreeSet::new(),
        );

        assert_eq!(
            plan.removed_canceled
                .iter()
                .map(|item| (item.block_id.as_str(), item.line_number))
                .collect::<Vec<_>>(),
            vec![("canceled", 4), ("canceled", 7), ("custom", 10)]
        );
        assert_eq!(plan.struck.len(), 1);
        assert_eq!(plan.moved.len(), 1);
        assert_eq!(
            apply_structural_plan(contents, &model, &plan),
            concat!(
                "- [x] Completed\r\n",
                "  - moving parent 🍅 ~~[[tasks#^done]]~~\r\n",
                "- [ ] Future\r\n",
                "  - surviving [[tasks#^live]]\r\n",
                "    - surviving child [[tasks#^other]]\r\n",
            )
        );
    }

    #[test]
    fn canceled_subtree_deletion_preserves_crlf_and_no_final_newline() {
        let contents = concat!(
            "- [ ] Open\r\n",
            "  - keep [[tasks#^live]]\r\n",
            "  - remove [[tasks#^canceled]]\r\n",
            "    - nested detail\r\n",
            "  - final [[tasks#^other]]",
        );
        let lines = logical_lines(contents);
        let model = scan_pomodoros(&lines, 0..lines.len());
        let resolved = resolved(&[
            ("tasks", "live", vec![' ']),
            ("tasks", "canceled", vec!['-']),
            ("tasks", "other", vec![' ']),
        ]);
        let settings = test_settings();
        let plan = plan_structural_changes(
            &model,
            &resolved,
            &settings.done_statuses,
            &settings.status_types,
            &BTreeSet::new(),
        );

        assert_eq!(
            apply_structural_plan(contents, &model, &plan),
            concat!(
                "- [ ] Open\r\n",
                "  - keep [[tasks#^live]]\r\n",
                "  - final [[tasks#^other]]",
            )
        );
    }

    #[test]
    fn duplicate_deleted_lines_do_not_report_canceled_reference_edits() {
        let contents = concat!(
            "- [ ] First\n",
            "  - [[tasks#^canceled]]\n",
            "- [ ] Second\n",
            "  - duplicate [[tasks#^canceled]]\n",
        );
        let lines = logical_lines(contents);
        let model = scan_pomodoros(&lines, 0..lines.len());
        let resolved = resolved(&[("tasks", "canceled", vec!['-'])]);
        let removals = plan_duplicate_line_removals(&lines, &model, &resolved);
        let deleted_lines = removals
            .iter()
            .map(|item| item.line_number - 1)
            .collect::<BTreeSet<_>>();
        let settings = test_settings();
        let plan = plan_structural_changes(
            &model,
            &resolved,
            &settings.done_statuses,
            &settings.status_types,
            &deleted_lines,
        );

        assert_eq!(removals.len(), 1);
        assert_eq!(plan.removed_canceled.len(), 1);
        assert_eq!(plan.removed_canceled[0].line_number, 2);
        assert!(plan.token_edits.is_empty());
        assert_eq!(
            apply_structural_plan(contents, &model, &plan),
            "- [ ] First\n- [ ] Second\n"
        );
    }

    #[test]
    fn completion_classification_accepts_conventional_and_custom_done_only() {
        let done = BTreeSet::from(['x', 'X', 'D']);
        for status in ['x', 'X', 'D'] {
            assert!(is_done_status(status, &done));
        }
        for status in [' ', '*', '/', '-', '?'] {
            assert!(!is_done_status(status, &done));
        }
    }

    #[test]
    fn cancellation_classification_uses_recognized_tasks_status_types() {
        let mut status_types = test_settings().status_types;
        status_types.insert('C', TaskStatusType::Cancelled);
        status_types.insert('Q', TaskStatusType::Todo);

        for status in ['-', 'C'] {
            assert!(is_canceled_status(status, &status_types));
        }
        for status in [' ', 'x', '*', '/', '?', 'Q', '!'] {
            assert!(!is_canceled_status(status, &status_types));
        }
    }
}
