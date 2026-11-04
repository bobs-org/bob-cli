use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
};

use chrono::NaiveDate;

use super::super::{
    capture_drawers::DrawerCatalogRecord,
    capture_language::DrawerSelector,
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
