use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
};

use chrono::NaiveDate;
use serde::Serialize;

use super::super::{
    capture_drawers::DrawerCatalogRecord,
    capture_language::DrawerSelector,
    config::relations::CancelLabel,
    note_tasks::NoteTaskSettings,
    task_complete::SuccessorSelection,
    task_locator::{LocatedTask, TaskLocator},
    task_move::transplant::{RenameRecord, RepairedFile},
    vault_links::NoteIndex,
};

/// Whether this merge came from an explicit task gesture or a relation label.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MergeGesture {
    Explicit,
    Label,
}

/// A concrete destination shape. Selectors retain whether the caller supplied
/// a slug or an exact literal/title through `DrawerSelector::exact`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum MergeContainer {
    Task(TaskLocator),
    Drawer(TaskLocator, DrawerSelector),
    Section {
        note: String,
        selector: DrawerSelector,
    },
}

/// The complete input to the pure merge engine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MergeRequest {
    pub(crate) sources: Vec<TaskLocator>,
    pub(crate) target: MergeContainer,
    pub(crate) label: String,
    pub(crate) gesture: MergeGesture,
    pub(crate) selection: SuccessorSelection,
}

/// Daily-note state captured by the command adapter. A required read failure
/// stays distinct from an absent daily note; the planner never performs I/O.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DailyNoteReadState {
    NotRead,
    Missing,
    Read(String),
    Failed(String),
}

/// Relation rendering policy needed by merge provenance and mirrors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct MergeRelationPolicy {
    pub(crate) linked_from_lines: bool,
    pub(crate) linked_from_mentions: bool,
}

impl Default for MergeRelationPolicy {
    fn default() -> Self {
        Self {
            linked_from_lines: true,
            linked_from_mentions: true,
        }
    }
}

/// All changing environment/configuration facts are supplied by the caller.
/// This value contains no filesystem handles and reads no clock or catalog.
#[derive(Debug, Clone)]
pub(crate) struct MergeContext<'a> {
    pub(crate) today: NaiveDate,
    pub(crate) daily_path: PathBuf,
    pub(crate) daily_read: DailyNoteReadState,
    pub(crate) task_settings: &'a NoteTaskSettings,
    pub(crate) note_index: &'a NoteIndex,
    pub(crate) drawer_catalog: &'a BTreeMap<String, DrawerCatalogRecord>,
    pub(crate) relation_policy: MergeRelationPolicy,
    /// Configured successor labels, already normalized by the caller.
    pub(crate) successor_labels: &'a [CancelLabel],
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MergeError {
    pub(crate) code: &'static str,
    pub(crate) message: String,
}

impl MergeError {
    pub(crate) fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SourceDisposition {
    Open,
    Closed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PreparedSource {
    pub(crate) locator: TaskLocator,
    pub(crate) path: PathBuf,
    /// Original one-based line; later stages re-resolve the stable identity
    /// against staged bytes before writing.
    pub(crate) line: usize,
    pub(crate) block_id: String,
    pub(crate) title: String,
    pub(crate) previous_status: char,
    pub(crate) disposition: SourceDisposition,
    pub(crate) moved_lines: usize,
    pub(crate) moved_notes: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PreparedContainerKind {
    Task,
    Drawer { title: String, created: bool },
    Section { title: String, created: bool },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PreparedContainer {
    pub(crate) path: PathBuf,
    pub(crate) kind: PreparedContainerKind,
    pub(crate) block_id: Option<String>,
    pub(crate) title: Option<String>,
    pub(crate) insertion_offset: usize,
    pub(crate) empty_heading: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PreparedPayload {
    pub(crate) source_index: usize,
    pub(crate) source_path: PathBuf,
    pub(crate) contents: String,
}

/// Internal preparation stage. It carries payload postimages and the complete
/// consulted note set, but is not a successful public merge plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PreparedMerge {
    pub(crate) container: Option<PreparedContainer>,
    pub(crate) sources: Vec<PreparedSource>,
    pub(crate) payloads: Vec<PreparedPayload>,
    pub(crate) postimages: BTreeMap<PathBuf, String>,
    pub(crate) renamed_block_ids: Vec<RenameRecord>,
    pub(crate) repaired_files: Vec<RepairedFile>,
    pub(crate) read_paths: BTreeSet<PathBuf>,
    pub(crate) warnings: Vec<String>,
}

/// Fully composed pure merge plan. Only changed postimages are present;
/// `write_order` keeps destination-before-source staging deterministic.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MergePlan {
    pub(crate) postimages: BTreeMap<PathBuf, String>,
    pub(crate) write_order: Vec<PathBuf>,
    pub(crate) records: Vec<MergeRecord>,
    pub(crate) renamed_block_ids: Vec<RenameRecord>,
    pub(crate) repaired_files: Vec<RepairedFile>,
    pub(crate) warnings: Vec<String>,
    /// Every note consulted from the supplied snapshot. The adapter adds
    /// its vault scan-path set to this read set before guarded writes.
    pub(crate) read_paths: BTreeSet<PathBuf>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum MergeOutcome {
    Merged,
    Unchanged,
    AlreadyClosed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct MergeTaskDescriptor {
    pub(crate) path: PathBuf,
    pub(crate) block_id: String,
    pub(crate) line: usize,
    pub(crate) title: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct MergeTargetDescriptor {
    pub(crate) path: PathBuf,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) block_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) drawer: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) section: Option<String>,
    pub(crate) kind: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct MergeOffer {
    pub(crate) key: String,
    pub(crate) n: usize,
    pub(crate) group: String,
    pub(crate) selected: bool,
    pub(crate) entry_name: String,
    pub(crate) entry_role: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) merged_from: Option<String>,
}

/// One source result in source argument order, shared by command and
/// capture adapters.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct MergeRecord {
    pub(crate) source: MergeTaskDescriptor,
    pub(crate) target: MergeTargetDescriptor,
    pub(crate) label: String,
    pub(crate) outcome: MergeOutcome,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) previous_status: Option<char>,
    pub(crate) moved_lines: usize,
    pub(crate) moved_notes: usize,
    pub(crate) dependents_repointed: Vec<String>,
    pub(crate) prerequisites_left: Vec<String>,
    pub(crate) successors_orphaned: Vec<String>,
    pub(crate) links_pruned: usize,
    pub(crate) offers: Vec<MergeOffer>,
    pub(crate) message: String,
}

impl From<&LocatedTask> for SourceDisposition {
    fn from(task: &LocatedTask) -> Self {
        if task.task.status_type.is_open() {
            Self::Open
        } else {
            Self::Closed
        }
    }
}

/// Re-resolve a stable locator to the canonical `note:block-id` spelling.
pub(crate) fn locator_text(locator: &TaskLocator) -> String {
    format!("{}:{}", locator.note, locator.block_id)
}
