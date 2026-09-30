//! Report, scan, and plan types for task-status sync.
use super::*;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct ChangeItem {
    pub(super) path: String,
    pub(super) line_number: usize,
    pub(super) block_id: String,
    pub(super) description: String,
    pub(super) dependency: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct DependencyStatusChange {
    pub(super) path: String,
    pub(super) line_number: usize,
    pub(super) block_id: String,
    pub(super) description: String,
    pub(super) from: char,
    pub(super) to: char,
    pub(super) open_dependency_ids: Vec<String>,
    pub(super) unresolved_dependency_ids: Vec<String>,
    pub(super) future_scheduled_date: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct UnresolvedReference {
    pub(super) target: String,
    pub(super) block_id: String,
    pub(super) reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct StruckCompletedReference {
    pub(super) target: String,
    pub(super) block_id: String,
    pub(super) pomodoro: String,
    pub(super) removed_embed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct MovedCompletedReference {
    pub(super) target: String,
    pub(super) block_id: String,
    pub(super) source_pomodoro: String,
    pub(super) destination_pomodoro: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct MarkerReference {
    pub(super) target: String,
    pub(super) block_id: String,
    pub(super) pomodoro: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct RemovedCanceledReference {
    pub(super) target: String,
    pub(super) block_id: String,
    pub(super) line_number: usize,
    pub(super) pomodoro: String,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub(super) struct DuplicateTaskIdentity {
    pub(super) path: String,
    pub(super) block_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct RemovedDuplicateLine {
    pub(super) line_number: usize,
    pub(super) pomodoro: String,
    pub(super) line: String,
    pub(super) duplicate_tasks: Vec<DuplicateTaskIdentity>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct RemovedEmptyPomodoro {
    pub(super) line_number: usize,
    pub(super) line: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct GroupedTaskSectionReport {
    pub(super) path: String,
    pub(super) original_heading_line: usize,
    pub(super) heading_ancestry: Vec<String>,
    pub(super) open: usize,
    pub(super) next_and_in_progress: usize,
    pub(super) blocked: usize,
    pub(super) done_and_canceled: usize,
    pub(super) moved_block_count: usize,
    pub(super) moved_blocks: Vec<GroupedMovedBlockReport>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct GroupedMovedBlockReport {
    pub(super) original_line: usize,
    pub(super) destination: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct GroupingWarningReport {
    pub(super) path: String,
    pub(super) original_heading_line: usize,
    pub(super) heading_ancestry: Vec<String>,
    pub(super) code: String,
    pub(super) message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct SyncResult {
    pub(super) ok: bool,
    pub(super) dry_run: bool,
    pub(super) daily_file: String,
    pub(super) previous_daily_file: Option<String>,
    pub(super) open_pomodoros: usize,
    pub(super) references: usize,
    pub(super) previous_daily_references: usize,
    pub(super) recent_activity_references: usize,
    pub(super) dependency_references: usize,
    pub(super) scanned_files: usize,
    pub(super) marked_next: Vec<ChangeItem>,
    pub(super) marked_in_progress: Vec<ChangeItem>,
    pub(super) cleared: Vec<ChangeItem>,
    /// Kept for JSON compatibility; always empty since sticky lanes retired
    /// the In Progress rollback.
    pub(super) cleared_in_progress: Vec<ChangeItem>,
    pub(super) marked_blocked: Vec<DependencyStatusChange>,
    pub(super) unblocked: Vec<DependencyStatusChange>,
    pub(super) struck_completed_references: Vec<StruckCompletedReference>,
    pub(super) embedded_completed_references: Vec<StruckCompletedReference>,
    pub(super) moved_completed_references: Vec<MovedCompletedReference>,
    pub(super) marker_added_references: Vec<MarkerReference>,
    pub(super) marker_removed_references: Vec<MarkerReference>,
    pub(super) removed_canceled_references: Vec<RemovedCanceledReference>,
    pub(super) removed_duplicate_lines: Vec<RemovedDuplicateLine>,
    pub(super) removed_empty_pomodoros: Vec<RemovedEmptyPomodoro>,
    pub(super) grouped_task_sections: Vec<GroupedTaskSectionReport>,
    pub(super) grouping_warnings: Vec<GroupingWarningReport>,
    pub(super) applied_files: Vec<String>,
    pub(super) deferred_files: Vec<String>,
    pub(super) recovery_directory: Option<String>,
    pub(super) kept_next: usize,
    pub(super) kept_in_progress: usize,
    pub(super) unresolved_references: Vec<UnresolvedReference>,
    pub(super) plan_budget: Option<plan_budget::PlanReport>,
}

#[derive(Debug, Clone)]
pub(super) struct FileScan {
    pub(super) path: PathBuf,
    pub(super) relative_path: PathBuf,
    pub(super) contents: String,
    pub(super) tasks: Vec<TaskLine>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum NoteKind {
    Area,
    Project,
    Other,
}

impl NoteKind {
    pub(super) fn is_area_or_project(self) -> bool {
        matches!(self, Self::Area | Self::Project)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct TaskLine {
    pub(super) line_index: usize,
    pub(super) status: char,
    pub(super) status_byte_offset: usize,
    pub(super) block_id: Option<String>,
    pub(super) task_id: Option<String>,
    pub(super) depends_on: Vec<String>,
    pub(super) scheduled: Option<NaiveDate>,
    pub(super) status_type: TaskStatusType,
    pub(super) status_recognized: bool,
    pub(super) description: String,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct RawReference {
    pub(super) target: String,
    pub(super) block_id: String,
}

#[derive(Debug, Clone)]
pub(super) struct PlannedChange {
    pub(super) file_index: usize,
    pub(super) status_byte_offset: usize,
    pub(super) replacement: char,
}

#[derive(Debug, Clone)]
pub(super) struct ComposedOutput {
    pub(super) path: PathBuf,
    pub(super) contents: String,
    pub(super) structural_regrouping: bool,
}

#[derive(Debug, Clone, Default)]
pub(super) struct ComposeResult {
    pub(super) outputs: Vec<ComposedOutput>,
    pub(super) grouped_task_sections: Vec<GroupedTaskSectionReport>,
    pub(super) grouping_warnings: Vec<GroupingWarningReport>,
}

#[derive(Debug, Clone, Default)]
pub(super) struct ApplyReport {
    pub(super) applied_files: Vec<String>,
    pub(super) deferred_files: Vec<String>,
    pub(super) recovery_directory: Option<String>,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct ComposeContext<'a> {
    pub(super) daily_path: &'a Path,
    pub(super) previous_daily_path: Option<&'a Path>,
    pub(super) daily_contents: &'a str,
    pub(super) normalized_daily_contents: &'a str,
    pub(super) settings: &'a TasksSettings,
}

#[derive(Debug, Clone)]
pub(crate) struct TasksSettings {
    pub(crate) global_filter: String,
    pub(super) done_statuses: BTreeSet<char>,
    pub(crate) status_types: BTreeMap<char, TaskStatusType>,
    pub(crate) status_definitions: Vec<TaskStatusDefinition>,
    pub(super) status_settings_error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TaskStatusDefinition {
    pub(crate) symbol: String,
    pub(crate) name: String,
    pub(super) next_status_symbol: String,
    pub(super) available_as_command: bool,
    pub(super) status_type: TaskStatusType,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TaskStatusType {
    Todo,
    Done,
    InProgress,
    OnHold,
    Cancelled,
    NonTask,
    Empty,
}

impl TaskStatusType {
    pub(super) fn from_settings(value: &str) -> Self {
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

    pub(super) fn as_tasks_type_name(self) -> &'static str {
        match self {
            Self::Todo => "TODO",
            Self::Done => "DONE",
            Self::InProgress => "IN_PROGRESS",
            Self::OnHold => "ON_HOLD",
            Self::Cancelled => "CANCELLED",
            Self::NonTask => "NON_TASK",
            Self::Empty => "EMPTY",
        }
    }

    pub(crate) fn is_open(self) -> bool {
        matches!(self, Self::Todo | Self::InProgress | Self::OnHold)
    }

    pub(super) fn is_terminal(self) -> bool {
        matches!(self, Self::Done | Self::Cancelled | Self::NonTask)
    }
}

#[derive(Debug, Clone)]
pub(super) struct PomodoroEntry {
    pub(super) line_index: usize,
    pub(super) end_line: usize,
    pub(super) open: bool,
    pub(super) completed: bool,
    pub(super) timed: bool,
    pub(super) has_child: bool,
    pub(super) child_indentation: Option<String>,
    pub(super) context: String,
}

#[derive(Debug, Clone)]
pub(super) struct LinkOccurrence {
    pub(super) reference: RawReference,
    pub(super) edit_start: usize,
    pub(super) edit_end: usize,
    pub(super) current_token: String,
    pub(super) preserved_marked_token: String,
    pub(super) preserved_unmarked_token: String,
    pub(super) retired_marked_token: String,
    pub(super) retired_unmarked_token: String,
    pub(super) embedded: bool,
    pub(super) struck: bool,
    pub(super) marker_count: usize,
}

#[derive(Debug, Clone)]
pub(super) struct LinkBullet {
    pub(super) entry_index: usize,
    pub(super) line_index: usize,
    pub(super) end_line: usize,
    pub(super) indentation: String,
    pub(super) links: Vec<LinkOccurrence>,
}

#[derive(Debug, Clone)]
pub(super) struct PomodoroModel {
    pub(super) entries: Vec<PomodoroEntry>,
    pub(super) bullets: Vec<LinkBullet>,
    pub(super) open_pomodoros: usize,
    pub(super) raw_references: BTreeSet<RawReference>,
    pub(super) recent_references: BTreeSet<RawReference>,
    pub(super) all_references: BTreeSet<RawReference>,
}

#[derive(Debug, Clone)]
pub(super) struct ResolvedReference {
    pub(super) path: PathBuf,
    pub(super) statuses: Vec<char>,
}

#[derive(Debug, Clone, Default)]
pub(super) struct ArchiveReferenceCatalog {
    pub(super) note_paths: BTreeSet<PathBuf>,
    pub(super) load_failures: BTreeMap<PathBuf, String>,
    pub(super) task_blocks: BTreeMap<(PathBuf, String), Vec<char>>,
}

pub(super) struct TaskReferenceResolver<'a> {
    pub(super) note_index: &'a NoteIndex,
    pub(super) task_blocks: &'a BTreeMap<(PathBuf, String), Vec<char>>,
    pub(super) archive_catalog: &'a ArchiveReferenceCatalog,
}

#[derive(Debug, Clone, Copy)]
pub(super) enum ReferenceContext {
    CurrentDaily,
    PreviousDaily,
}

pub(super) enum ResolvePathError {
    Unresolved,
    ArchiveUnavailable(String),
}

#[derive(Debug, Clone)]
pub(super) struct StructuralPlan {
    pub(super) token_edits: BTreeMap<usize, Vec<TokenEdit>>,
    pub(super) moves: Vec<BulletMove>,
    pub(super) deleted_lines: BTreeSet<usize>,
    pub(super) target_entry: Option<usize>,
    pub(super) struck: Vec<StruckCompletedReference>,
    pub(super) moved: Vec<MovedCompletedReference>,
    pub(super) marker_added: Vec<MarkerReference>,
    pub(super) marker_removed: Vec<MarkerReference>,
    pub(super) removed_canceled: Vec<RemovedCanceledReference>,
}

#[derive(Debug, Clone, Default)]
pub(super) struct EmptyPomodoroPlan {
    pub(super) deleted_lines: BTreeSet<usize>,
    pub(super) removed: Vec<RemovedEmptyPomodoro>,
}

#[derive(Debug, Clone)]
pub(super) struct TokenEdit {
    pub(super) start: usize,
    pub(super) end: usize,
    pub(super) replacement: String,
}

#[derive(Debug, Clone)]
pub(super) struct BulletMove {
    pub(super) start_line: usize,
    pub(super) end_line: usize,
    pub(super) source_indentation: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Transition {
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
pub(super) enum RankedStatus {
    Ready,
    Next,
    InProgress,
}

impl RankedStatus {
    pub(super) fn from_checkbox(status: char) -> Option<Self> {
        match status {
            ' ' => Some(Self::Ready),
            '*' => Some(Self::Next),
            '/' => Some(Self::InProgress),
            _ => None,
        }
    }

    pub(super) fn checkbox(self) -> char {
        match self {
            Self::Ready => ' ',
            Self::Next => '*',
            Self::InProgress => '/',
        }
    }
}
