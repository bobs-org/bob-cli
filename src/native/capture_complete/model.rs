use serde::Serialize;

use crate::native::{
    capture_block_ids,
    capture_language::CompletionContext,
    capture_link_tasks,
    capture_links::{
        WikilinkBlockCandidate, WikilinkHeadingCandidate, WikilinkNoteCandidate,
    },
    capture_pomodoros::{self, PomodoroState},
    capture_targets::CaptureTargetKind,
};

/// Bump only for a breaking change to the JSON object below; new optional
/// fields keep version 1.
pub(super) const SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum OutputFormat {
    Human,
    Json,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub(super) struct Replacement {
    pub(super) start: usize,
    pub(super) end: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct RouteCandidate {
    pub(super) replacement: String,
    pub(super) route: String,
    pub(super) label: String,
    pub(super) kind: CaptureTargetKind,
    pub(super) status: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct SectionCandidate {
    pub(super) replacement: String,
    pub(super) title: String,
    pub(super) level: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct TaskCandidate {
    pub(super) replacement: String,
    #[serde(rename = "ref")]
    pub(super) task_ref: String,
    pub(super) block_id: Option<String>,
    pub(super) route: String,
    pub(super) requires_block_id: bool,
    pub(super) status_symbol: char,
    pub(super) status_name: String,
    pub(super) status_type: &'static str,
    pub(super) text: String,
    pub(super) section: Option<String>,
    pub(super) depth: usize,
    pub(super) child_count: usize,
    pub(super) line: usize,
    pub(super) pomodoro: Option<ActiveTaskPomodoroCandidate>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct TaskSectionCandidate {
    pub(super) replacement: String,
    pub(super) title: String,
    pub(super) slug: String,
    pub(super) route: String,
    pub(super) block_id: Option<String>,
    pub(super) text: String,
    pub(super) line: usize,
    pub(super) child_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct ActiveTaskPomodoroCandidate {
    pub(super) line: usize,
    pub(super) name: Option<String>,
    pub(super) time_range: Option<String>,
    pub(super) is_current: bool,
}

/// One `task_link` (`:` picker) candidate: any linkable open task in a
/// routable inbox, area, or non-terminal project note. The JSON keys match
/// the picker contract: `replacement` is the `@route:id` an accept inserts
/// (empty for ID-less tasks, which clients must never insert),
/// `requires_block_id` marks those rows, and `pulls_forward` is omitted
/// when false.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct TaskLinkCandidate {
    pub(super) replacement: String,
    #[serde(rename = "ref")]
    pub(super) task_ref: String,
    pub(super) route: String,
    pub(super) note_kind: CaptureTargetKind,
    pub(super) block_id: Option<String>,
    pub(super) requires_block_id: bool,
    pub(super) block_id_suggestions: Vec<String>,
    pub(super) status_symbol: char,
    pub(super) status_name: String,
    pub(super) status_type: &'static str,
    pub(super) text: String,
    pub(super) section: Option<String>,
    pub(super) depth: usize,
    pub(super) line: usize,
    pub(super) group: capture_link_tasks::LinkTaskGroup,
    pub(super) scheduled: Option<String>,
    #[serde(skip_serializing_if = "is_false")]
    pub(super) pulls_forward: bool,
    pub(super) pomodoro: Option<ActiveTaskPomodoroCandidate>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct ActiveTaskCandidate {
    pub(super) replacement: String,
    #[serde(rename = "ref")]
    pub(super) task_ref: String,
    pub(super) route: String,
    pub(super) block_id: String,
    pub(super) status_symbol: char,
    pub(super) status_name: String,
    pub(super) status_type: &'static str,
    pub(super) text: String,
    pub(super) section: Option<String>,
    pub(super) pomodoro: Option<ActiveTaskPomodoroCandidate>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct PomodoroNameCandidate {
    pub(super) replacement: String,
    #[serde(rename = "ref", skip_serializing_if = "Option::is_none")]
    pub(super) pomodoro_ref: Option<capture_pomodoros::PomodoroRef>,
    pub(super) name: Option<String>,
    pub(super) requires_name: bool,
    #[serde(skip_serializing_if = "is_false")]
    pub(super) creates_pomodoro: bool,
    /// Marks the row `bob capture` with a bare `=` would start. Only set
    /// in the `pomodoro_start_name` context; always false (and therefore
    /// omitted) in the `pomodoro_name` context so that output stays
    /// byte-identical.
    #[serde(skip_serializing_if = "is_false")]
    pub(super) next_up: bool,
    /// Resulting theme count and cap when this create row is accepted.
    /// Only on `creates_pomodoro` rows (new and again rows in
    /// `pomodoro_start_name`); omitted when the daily note or the plan
    /// config is unavailable.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) plan_themes_after: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) plan_themes_cap: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) line: Option<usize>,
    pub(super) state: PomodoroState,
    pub(super) status_symbol: char,
    pub(super) time_range: Option<String>,
    pub(super) placeholder: bool,
    pub(super) is_current: bool,
    pub(super) child_count: usize,
    pub(super) match_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(untagged)]
pub(super) enum Candidates {
    Route(Vec<RouteCandidate>),
    Section(Vec<SectionCandidate>),
    Task(Vec<TaskCandidate>),
    TaskSection(Vec<TaskSectionCandidate>),
    PomodoroName(Vec<PomodoroNameCandidate>),
    ActiveTask(Vec<ActiveTaskCandidate>),
    TaskLink(Vec<TaskLinkCandidate>),
    WikilinkNote(Vec<WikilinkNoteCandidate>),
    WikilinkHeading(Vec<WikilinkHeadingCandidate>),
    WikilinkBlock(Vec<WikilinkBlockCandidate>),
}

impl Candidates {
    pub(super) fn len(&self) -> usize {
        match self {
            Self::Route(items) => items.len(),
            Self::Section(items) => items.len(),
            Self::Task(items) => items.len(),
            Self::TaskSection(items) => items.len(),
            Self::PomodoroName(items) => items.len(),
            Self::ActiveTask(items) => items.len(),
            Self::TaskLink(items) => items.len(),
            Self::WikilinkNote(items) => items.len(),
            Self::WikilinkHeading(items) => items.len(),
            Self::WikilinkBlock(items) => items.len(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct CaptureCompleteResult {
    pub(super) ok: bool,
    pub(super) schema_version: u32,
    pub(super) cursor: usize,
    pub(super) replacement: Replacement,
    pub(super) context: Option<CompletionContext>,
    pub(super) candidates: Candidates,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) block_id: Option<capture_block_ids::BlockIdField>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(super) warnings: Vec<String>,
}

impl CaptureCompleteResult {
    pub(super) fn empty(cursor: usize) -> Self {
        Self {
            ok: true,
            schema_version: SCHEMA_VERSION,
            cursor,
            replacement: Replacement {
                start: cursor,
                end: cursor,
            },
            context: None,
            candidates: Candidates::Route(Vec::new()),
            block_id: None,
            warnings: Vec::new(),
        }
    }
}

pub(super) fn is_false(value: &bool) -> bool {
    !*value
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct CompleteError {
    pub(super) kind: CompleteErrorKind,
    pub(super) message: String,
}

impl CompleteError {
    pub(super) fn usage(message: impl Into<String>) -> Self {
        Self {
            kind: CompleteErrorKind::Usage,
            message: message.into(),
        }
    }

    pub(super) fn io(message: impl Into<String>) -> Self {
        Self {
            kind: CompleteErrorKind::Io,
            message: message.into(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum CompleteErrorKind {
    Usage,
    Io,
}

impl CompleteErrorKind {
    pub(super) fn exit_code(self) -> i32 {
        match self {
            Self::Usage => 2,
            Self::Io => 1,
        }
    }
}
