//! Editor model, span, and diagnostic types.

use super::model::*;
use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum SpanKind {
    Route,
    Section,
    TaskBlockIdRoute,
    TaskBlockId,
    PomodoroRoute,
    PomodoroBlockId,
    PomodoroName,
    PomodoroStart,
    PomodoroStartDrop,
    PomodoroClose,
    PomodoroCloseInProgress,
    PomodoroCloseComplete,
    PomodoroCloseDrop,
    ActiveTaskRoute,
    ActiveTaskBlockId,
    PomodoroAdjust,
    PomodoroShift,
    SubBulletRoute,
    SubBulletBlockId,
    SubBulletSection,
    TaskToggleRoute,
    TaskToggleBlockId,
    TaskTogglePomodoroName,
    TaskToggleExplicitToggle,
    ProjectNoteMarker,
    ProjectTaskLinkMarker,
    ProjectTaskBlockId,
    GlobalRoute,
    GlobalSubBulletRoute,
    GlobalSubBulletBlockId,
    PomodoroNote,
    NowTag,
    Schedule,
    Priority,
    Clipboard,
    InteractivePlaceholder,
    WikilinkDelimiter,
    WikilinkTarget,
    WikilinkHeading,
    WikilinkBlockId,
    WikilinkAlias,
}

impl SpanKind {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Route => "route",
            Self::Section => "section",
            Self::TaskBlockIdRoute => "task_block_id_route",
            Self::TaskBlockId => "task_block_id",
            Self::PomodoroRoute => "pomodoro_route",
            Self::PomodoroBlockId => "pomodoro_block_id",
            Self::PomodoroName => "pomodoro_name",
            Self::PomodoroStart => "pomodoro_start",
            Self::PomodoroStartDrop => "pomodoro_start_drop",
            Self::PomodoroClose => "pomodoro_close",
            Self::PomodoroCloseInProgress => "pomodoro_close_in_progress",
            Self::PomodoroCloseComplete => "pomodoro_close_complete",
            Self::PomodoroCloseDrop => "pomodoro_close_drop",
            Self::ActiveTaskRoute => "active_task_route",
            Self::ActiveTaskBlockId => "active_task_block_id",
            Self::PomodoroAdjust => "pomodoro_adjust",
            Self::PomodoroShift => "pomodoro_shift",
            Self::SubBulletRoute => "sub_bullet_route",
            Self::SubBulletBlockId => "sub_bullet_block_id",
            Self::SubBulletSection => "sub_bullet_section",
            Self::TaskToggleRoute => "task_toggle_route",
            Self::TaskToggleBlockId => "task_toggle_block_id",
            Self::TaskTogglePomodoroName => "task_toggle_pomodoro_name",
            Self::TaskToggleExplicitToggle => "task_toggle_explicit_toggle",
            Self::ProjectNoteMarker => "project_note_marker",
            Self::ProjectTaskLinkMarker => "project_task_link_marker",
            Self::ProjectTaskBlockId => "project_task_block_id",
            Self::GlobalRoute => "global_route",
            Self::GlobalSubBulletRoute => "global_sub_bullet_route",
            Self::GlobalSubBulletBlockId => "global_sub_bullet_block_id",
            Self::PomodoroNote => "pomodoro_note",
            Self::NowTag => "now_tag",
            Self::Schedule => "schedule",
            Self::Priority => "priority",
            Self::Clipboard => "clipboard",
            Self::InteractivePlaceholder => "interactive_placeholder",
            Self::WikilinkDelimiter => "wikilink_delimiter",
            Self::WikilinkTarget => "wikilink_target",
            Self::WikilinkHeading => "wikilink_heading",
            Self::WikilinkBlockId => "wikilink_block_id",
            Self::WikilinkAlias => "wikilink_alias",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub(crate) struct Span {
    pub(crate) start: usize,
    pub(crate) end: usize,
    pub(crate) kind: SpanKind,
}

/// The full documented severity vocabulary of the `capture-parse` JSON
/// contract. Today's grammar only raises errors; `warning` and `info` stay
/// reserved so the wire format does not change when it starts to.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Severity {
    Error,
    Warning,
    Info,
}

impl Severity {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Error => "error",
            Self::Warning => "warning",
            Self::Info => "info",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct Diagnostic {
    pub(crate) severity: Severity,
    /// Stable snake_case identifier for programmatic handling.
    pub(crate) code: &'static str,
    pub(crate) message: String,
    pub(crate) range: Option<(usize, usize)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum EditorMode {
    Task,
    Bullet,
    PomodoroTask,
    PomodoroNote,
    SubBullet,
    TaskToggle,
    ProjectNote,
    PomodoroProjectNote,
    PomodoroAdjust,
    PomodoroShift,
    PomodoroLink,
    PomodoroClose,
    /// Whole-item `=`/`=<X>` start.
    PomodoroStart,
    Incomplete,
}

impl EditorMode {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Task => "task",
            Self::Bullet => "bullet",
            Self::PomodoroTask => "pomodoro_task",
            Self::PomodoroNote => "pomodoro_note",
            Self::SubBullet => "sub_bullet",
            Self::TaskToggle => "task_toggle",
            Self::ProjectNote => "project_note",
            Self::PomodoroProjectNote => "pomodoro_project_note",
            Self::PomodoroAdjust => "pomodoro_adjust",
            Self::PomodoroShift => "pomodoro_shift",
            Self::PomodoroLink => "pomodoro_link",
            Self::PomodoroClose => "pomodoro_close",
            Self::PomodoroStart => "pomodoro_start",
            Self::Incomplete => "incomplete",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Need {
    Route,
    Section,
    BlockId,
    PomodoroId,
    PomodoroName,
    Task,
    TaskSection,
    ActiveTask,
    TaskLink,
    PomodoroCloseTask,
    PomodoroStartTask,
    NowTag,
}

impl Need {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Route => "route",
            Self::Section => "section",
            Self::BlockId => "block_id",
            Self::PomodoroId => "pomodoro_id",
            Self::PomodoroName => "pomodoro_name",
            Self::Task => "task",
            Self::TaskSection => "task_section",
            Self::ActiveTask => "active_task",
            Self::TaskLink => "task_link",
            Self::PomodoroCloseTask => "pomodoro_close_task",
            Self::PomodoroStartTask => "pomodoro_start_task",
            Self::NowTag => "now_tag",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct EditorParse {
    pub(crate) body: String,
    pub(crate) mode: EditorMode,
    pub(crate) route: Option<String>,
    pub(crate) section: Option<String>,
    pub(crate) block_id: Option<String>,
    pub(crate) needs: Vec<Need>,
    /// Validated additive `@<route>:<block-id>[#<name>]=<X>` start suffix,
    /// when the resolved marker carries one. `None` for every older marker
    /// shape, so version-tolerant readers see no change.
    pub(crate) pomodoro_start: Option<PomodoroStartSpec>,
    /// Validated additive whole-item `+N`/`-N` adjustment spec, when the
    /// item is an exact signed-count adjustment. `None` for every older
    /// input, so version-tolerant readers see no change. Purely lexical:
    /// it never guesses current ledger times.
    pub(crate) pomodoro_adjust: Option<PomodoroAdjustSpec>,
    /// Validated additive whole-item `++N`/`--N` shift spec, when the item
    /// is an exact session-operator shift. `None` for every older input,
    /// so version-tolerant readers see no change. Purely lexical.
    pub(crate) pomodoro_shift: Option<PomodoroShiftSpec>,
    /// Validated additive `=x` close suffix, when the resolved marker or
    /// whole item carries one. `None` for every older input, so
    /// version-tolerant readers see no change. Purely lexical.
    pub(crate) pomodoro_close: Option<PomodoroCloseSpec>,
    pub(crate) spans: Vec<Span>,
    pub(crate) diagnostics: Vec<Diagnostic>,
    /// Normalized authored-child bodies plus semantic depth for every other
    /// valid, nonempty physical line, in source order. A malformed,
    /// orphaned, or empty-after-markers child line is reported as a
    /// diagnostic instead and excluded here.
    pub(crate) sub_bullets: Vec<AuthoredSubBullet>,
    pub(crate) items: Vec<EditorItemParse>,
    pub(crate) global_destination: Option<EditorGlobalDestination>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct EditorGlobalDestination {
    pub(crate) start: usize,
    pub(crate) end: usize,
    pub(crate) line: usize,
    pub(crate) mode: EditorMode,
    pub(crate) route: Option<String>,
    pub(crate) block_id: Option<String>,
    pub(crate) needs: Vec<Need>,
    pub(crate) inherit: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct EditorItemParse {
    pub(crate) index: usize,
    pub(crate) start: usize,
    pub(crate) end: usize,
    pub(crate) line_start: usize,
    pub(crate) line_end: usize,
    pub(crate) body: String,
    pub(crate) mode: EditorMode,
    pub(crate) route: Option<String>,
    pub(crate) section: Option<String>,
    pub(crate) block_id: Option<String>,
    pub(crate) needs: Vec<Need>,
    pub(crate) pomodoro_start: Option<PomodoroStartSpec>,
    pub(crate) pomodoro_adjust: Option<PomodoroAdjustSpec>,
    pub(crate) pomodoro_shift: Option<PomodoroShiftSpec>,
    pub(crate) pomodoro_close: Option<PomodoroCloseSpec>,
    pub(crate) spans: Vec<Span>,
    pub(crate) diagnostics: Vec<Diagnostic>,
    pub(crate) sub_bullets: Vec<AuthoredSubBullet>,
    pub(crate) has_local_destination: bool,
    /// Every *complete* (non-incomplete) local destination marker this item
    /// owns, in source order, across every one of its lines. `rewrite_draft`
    /// uses the length of this list to tell "no local marker" from "one" from
    /// "more than one" (Rule A6), and the sole entry's span to build the
    /// absorb-local-marker edit (Rule A1).
    pub(crate) local_destination_markers: Vec<LocalDestinationMarker>,
}

/// One complete local destination marker token an item owns, with the span
/// the marker text (`@route`, `@route+block-id`, `@route#Section`,
/// `@route^block-id`, `@route:block-id`, `@route:block-id#pomodoro`, or a
/// trailing bare `#`) occupies in the original draft.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LocalDestinationMarker {
    pub(crate) start: usize,
    pub(crate) end: usize,
    pub(crate) text: String,
    pub(crate) mode: EditorMode,
    pub(crate) route: Option<String>,
    pub(crate) block_id: Option<String>,
    pub(crate) section: Option<String>,
}

/// One `@...` token resolved for the editor. `requires_body` marks the plain
/// `@route` form, which only routes when body text sits on the other side --
/// the same rule `parse_capture_text_with_clip_control` applies.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct MarkerParse {
    pub(super) mode: EditorMode,
    pub(super) route: Option<String>,
    pub(super) section: Option<String>,
    pub(super) block_id: Option<String>,
    pub(super) needs: Vec<Need>,
    pub(super) spans: Vec<Span>,
    pub(super) requires_body: bool,
    /// Validated `@<route>:<block-id>[#<name>]=<X>` start suffix, when the
    /// marker carries one. Set even on incomplete markers (e.g. `@r:=3`
    /// still missing its block ID); invalid suffixes become an
    /// `invalid_pomodoro_start` diagnostic instead.
    pub(super) pomodoro_start: Option<PomodoroStartSpec>,
    /// Validated `@<route>:<block-id>=x` close suffix, when the marker
    /// carries one. Never coexists with `pomodoro_start`.
    pub(super) pomodoro_close: Option<PomodoroCloseSpec>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum TokenParse {
    Marker(MarkerParse),
    Invalid(Diagnostic),
}
