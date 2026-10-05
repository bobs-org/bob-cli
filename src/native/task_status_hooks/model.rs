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

/// One dependency projection write (`contract` §4.3): `kind` is
/// `dependent_field` (R1/R2 field projection), `target_id` (R1 target
/// `[id::]` stamp), or `line_removed` (R9).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct DependencyProjectionUpdate {
    pub(super) kind: String,
    pub(super) path: String,
    pub(super) line: usize,
    pub(super) detail: String,
}

/// One adopted, healed, or canonicalised Depends-On line: `detail`
/// carries the inserted line text (`adopted_dependency_lines`,
/// `canonicalized_dependency_lines`) or the `old -> new` rewrite
/// (`healed_dependency_links`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct DependencyLineReport {
    pub(super) path: String,
    pub(super) line: usize,
    pub(super) detail: String,
}

/// One dependency reconciliation warning (`contract` §4): `kind`
/// names the rule (`unresolved_dependency_link`,
/// `unadoptable_dependency_id`, `non_task_dependency`,
/// `self_dependency`, `dependency_cycle`, `malformed_dependency_line`,
/// `previous_daily_target`, `unencodable_dependency_target`,
/// `dependency_field_ids_dropped`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct DependencyWarning {
    pub(super) kind: String,
    pub(super) path: String,
    pub(super) line: usize,
    pub(super) detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct StruckCompletedReference {
    pub(crate) target: String,
    pub(crate) block_id: String,
    pub(crate) pomodoro: String,
    pub(crate) removed_embed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct MovedCompletedReference {
    pub(crate) target: String,
    pub(crate) block_id: String,
    pub(crate) source_pomodoro: String,
    pub(crate) destination_pomodoro: String,
}

/// A carried bullet deleted instead of moved because the destination entry
/// already links the task. Reported only when the structural planner runs
/// with its opt-in dedupe flag.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct DeduplicatedCompletedReference {
    pub(crate) target: String,
    pub(crate) block_id: String,
    pub(crate) source_pomodoro: String,
    pub(crate) destination_pomodoro: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct MarkerReference {
    pub(crate) target: String,
    pub(crate) block_id: String,
    pub(crate) pomodoro: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct RemovedCanceledReference {
    pub(crate) target: String,
    pub(crate) block_id: String,
    pub(crate) line_number: usize,
    pub(crate) pomodoro: String,
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
pub(crate) struct RemovedEmptyPomodoro {
    pub(crate) line_number: usize,
    pub(crate) line: String,
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
    pub(super) dependency_projection_updates: Vec<DependencyProjectionUpdate>,
    pub(super) adopted_dependency_lines: Vec<DependencyLineReport>,
    pub(super) healed_dependency_links: Vec<DependencyLineReport>,
    pub(super) canonicalized_dependency_lines: Vec<DependencyLineReport>,
    pub(super) legacy_dependency_children: usize,
    pub(super) dependency_warnings: Vec<DependencyWarning>,
    pub(super) plan_budget: Option<plan_budget::PlanReport>,
}

#[derive(Debug, Clone)]
pub(crate) struct FileScan {
    pub(crate) path: PathBuf,
    pub(crate) relative_path: PathBuf,
    pub(crate) contents: String,
    pub(crate) tasks: Vec<TaskLine>,
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
pub(crate) struct TaskLine {
    pub(crate) line_index: usize,
    pub(crate) status: char,
    pub(crate) status_byte_offset: usize,
    pub(crate) block_id: Option<String>,
    pub(crate) task_id: Option<String>,
    pub(crate) depends_on: Vec<String>,
    pub(crate) scheduled: Option<NaiveDate>,
    pub(crate) status_type: TaskStatusType,
    pub(crate) status_recognized: bool,
    pub(crate) description: String,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct RawReference {
    pub(crate) target: String,
    pub(crate) block_id: String,
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
    pub(crate) done_statuses: BTreeSet<char>,
    pub(crate) status_types: BTreeMap<char, TaskStatusType>,
    pub(crate) status_definitions: Vec<TaskStatusDefinition>,
    pub(crate) status_settings_error: Option<String>,
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

    pub(crate) fn is_terminal(self) -> bool {
        matches!(self, Self::Done | Self::Cancelled | Self::NonTask)
    }
}

#[derive(Debug, Clone)]
pub(crate) struct PomodoroEntry {
    pub(crate) line_index: usize,
    pub(crate) end_line: usize,
    pub(crate) open: bool,
    pub(crate) completed: bool,
    pub(crate) timed: bool,
    pub(crate) has_child: bool,
    pub(crate) child_indentation: Option<String>,
    pub(crate) context: String,
}

#[derive(Debug, Clone)]
pub(crate) struct LinkOccurrence {
    pub(crate) reference: RawReference,
    pub(crate) edit_start: usize,
    pub(crate) edit_end: usize,
    pub(crate) current_token: String,
    pub(crate) preserved_marked_token: String,
    pub(crate) preserved_unmarked_token: String,
    pub(crate) retired_marked_token: String,
    pub(crate) retired_unmarked_token: String,
    pub(crate) embedded: bool,
    pub(crate) struck: bool,
    pub(crate) marker_count: usize,
}

#[derive(Debug, Clone)]
pub(crate) struct LinkBullet {
    pub(crate) entry_index: usize,
    pub(crate) line_index: usize,
    pub(crate) end_line: usize,
    pub(crate) indentation: String,
    pub(crate) links: Vec<LinkOccurrence>,
}

#[derive(Debug, Clone)]
pub(crate) struct PomodoroModel {
    pub(crate) entries: Vec<PomodoroEntry>,
    pub(crate) bullets: Vec<LinkBullet>,
    pub(crate) open_pomodoros: usize,
    pub(crate) raw_references: BTreeSet<RawReference>,
    pub(crate) recent_references: BTreeSet<RawReference>,
    pub(crate) all_references: BTreeSet<RawReference>,
}

#[derive(Debug, Clone)]
pub(crate) struct ResolvedReference {
    pub(crate) path: PathBuf,
    pub(crate) statuses: Vec<char>,
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
pub(crate) struct StructuralPlan {
    pub(crate) token_edits: BTreeMap<usize, Vec<TokenEdit>>,
    pub(crate) moves: Vec<BulletMove>,
    pub(crate) deleted_lines: BTreeSet<usize>,
    pub(crate) target_entry: Option<usize>,
    pub(crate) struck: Vec<StruckCompletedReference>,
    pub(crate) moved: Vec<MovedCompletedReference>,
    pub(crate) deduplicated: Vec<DeduplicatedCompletedReference>,
    pub(crate) marker_added: Vec<MarkerReference>,
    pub(crate) marker_removed: Vec<MarkerReference>,
    pub(crate) removed_canceled: Vec<RemovedCanceledReference>,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct EmptyPomodoroPlan {
    pub(crate) deleted_lines: BTreeSet<usize>,
    pub(crate) removed: Vec<RemovedEmptyPomodoro>,
}

#[derive(Debug, Clone)]
pub(crate) struct TokenEdit {
    pub(crate) start: usize,
    pub(crate) end: usize,
    pub(crate) replacement: String,
}

#[derive(Debug, Clone)]
pub(crate) struct BulletMove {
    pub(crate) start_line: usize,
    pub(crate) end_line: usize,
    pub(crate) source_indentation: String,
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
