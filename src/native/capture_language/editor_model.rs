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
    /// A whole `#<type>` marker token, sigil and quotes included.
    TaskType,
    PomodoroStart,
    PomodoroStartDrop,
    PomodoroClose,
    PomodoroCloseInProgress,
    PomodoroClosePark,
    PomodoroCloseComplete,
    PomodoroCloseDrop,
    PomodoroCloseLogIndex,
    ActiveTaskRoute,
    ActiveTaskBlockId,
    PomodoroAdjust,
    PomodoroShift,
    PomodoroSplit,
    PomodoroMerge,
    SubBulletRoute,
    SubBulletBlockId,
    Drawer,
    TaskToggleRoute,
    TaskToggleBlockId,
    TaskToggleExplicitToggle,
    ProjectNoteMarker,
    ProjectNoteSeparator,
    #[allow(dead_code)]
    ProjectNoteOfferMarker,
    ProjectTaskLinkMarker,
    ProjectTaskBlockId,
    GlobalRoute,
    Schedule,
    Priority,
    /// A whole `%` directive token (`%c`/`%c<N>`, `%n`, `%l`, bare `%`).
    Directive,
    DependencySigil,
    DependencyNote,
    DependencyBlockId,
    BlockedTaskSigil,
    BlockedTaskNote,
    BlockedTaskBlockId,
    RelationSigil,
    RelationNote,
    RelationBlockId,
    RelationLabelSeparator,
    RelationLabel,
    /// A whole-item bare URL admitted by the routing policy.
    RefUrl,
    TaskCompleteSigil,
    TaskCompleteNote,
    TaskCompleteBlockId,
    InteractivePlaceholder,
    TemplatePlaceholder,
    TemplateField,
    WikilinkDelimiter,
    WikilinkTarget,
    WikilinkHeading,
    WikilinkBlockId,
    WikilinkAlias,
    WikilinkTaskDelimiter,
    WikilinkTaskQuery,
    /// An ALL-CAPS first-level drawer bullet (the title text, not a `#`
    /// shortcut).
    DrawerBullet,
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
            Self::TaskType => "task_type",
            Self::PomodoroStart => "pomodoro_start",
            Self::PomodoroStartDrop => "pomodoro_start_drop",
            Self::PomodoroClose => "pomodoro_close",
            Self::PomodoroCloseInProgress => "pomodoro_close_in_progress",
            Self::PomodoroClosePark => "pomodoro_close_park",
            Self::PomodoroCloseComplete => "pomodoro_close_complete",
            Self::PomodoroCloseDrop => "pomodoro_close_drop",
            Self::PomodoroCloseLogIndex => "pomodoro_close_log_index",
            Self::ActiveTaskRoute => "active_task_route",
            Self::ActiveTaskBlockId => "active_task_block_id",
            Self::PomodoroAdjust => "pomodoro_adjust",
            Self::PomodoroShift => "pomodoro_shift",
            Self::PomodoroSplit => "pomodoro_split",
            Self::PomodoroMerge => "pomodoro_merge",
            Self::SubBulletRoute => "sub_bullet_route",
            Self::SubBulletBlockId => "sub_bullet_block_id",
            Self::Drawer => "drawer",
            Self::TaskToggleRoute => "task_toggle_route",
            Self::TaskToggleBlockId => "task_toggle_block_id",
            Self::TaskToggleExplicitToggle => "task_toggle_explicit_toggle",
            Self::ProjectNoteMarker => "project_note_marker",
            Self::ProjectNoteSeparator => "project_note_separator",
            Self::ProjectNoteOfferMarker => "project_note_offer_marker",
            Self::ProjectTaskLinkMarker => "project_task_link_marker",
            Self::ProjectTaskBlockId => "project_task_block_id",
            Self::GlobalRoute => "global_route",
            Self::Schedule => "schedule",
            Self::Priority => "priority",
            Self::Directive => "directive",
            Self::DependencySigil => "dependency_sigil",
            Self::DependencyNote => "dependency_note",
            Self::DependencyBlockId => "dependency_block_id",
            Self::BlockedTaskSigil => "blocked_task_sigil",
            Self::BlockedTaskNote => "blocked_task_note",
            Self::BlockedTaskBlockId => "blocked_task_block_id",
            Self::RelationSigil => "relation_sigil",
            Self::RelationNote => "relation_note",
            Self::RelationBlockId => "relation_block_id",
            Self::RelationLabelSeparator => "relation_label_separator",
            Self::RelationLabel => "relation_label",
            Self::RefUrl => "ref_url",
            Self::TaskCompleteSigil => "task_complete_sigil",
            Self::TaskCompleteNote => "task_complete_note",
            Self::TaskCompleteBlockId => "task_complete_block_id",
            Self::InteractivePlaceholder => "interactive_placeholder",
            Self::TemplatePlaceholder => "template_placeholder",
            Self::TemplateField => "template_field",
            Self::WikilinkDelimiter => "wikilink_delimiter",
            Self::WikilinkTarget => "wikilink_target",
            Self::WikilinkHeading => "wikilink_heading",
            Self::WikilinkBlockId => "wikilink_block_id",
            Self::WikilinkAlias => "wikilink_alias",
            Self::WikilinkTaskDelimiter => "wikilink_task_delimiter",
            Self::WikilinkTaskQuery => "wikilink_task_query",
            Self::DrawerBullet => "drawer_bullet",
        }
    }
}

fn is_no_project_note(form: &Option<super::model::ProjectNoteForm>) -> bool {
    form.is_none()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct Span {
    pub(crate) start: usize,
    pub(crate) end: usize,
    pub(crate) kind: SpanKind,
    #[serde(default, skip_serializing_if = "is_no_project_note")]
    pub(crate) project_note: Option<super::model::ProjectNoteForm>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) key: Option<String>,
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
    PomodoroSplit,
    PomodoroMerge,
    PomodoroLink,
    PomodoroClose,
    /// Whole-item `=`/`=<X>` start.
    PomodoroStart,
    /// A dependency-only action: prerequisites for an explicitly selected
    /// existing task, with no new task.
    TaskDependency,
    /// A whole-item `!note:block-id` task completion.
    TaskComplete,
    /// A whole-item bare URL admitted by the routing policy.
    Ref,
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
            Self::PomodoroSplit => "pomodoro_split",
            Self::PomodoroMerge => "pomodoro_merge",
            Self::PomodoroLink => "pomodoro_link",
            Self::PomodoroClose => "pomodoro_close",
            Self::PomodoroStart => "pomodoro_start",
            Self::TaskDependency => "task_dependency",
            Self::TaskComplete => "task_complete",
            Self::Ref => "ref",
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
    /// A `#` marker is still open: name the task type to finish it.
    TaskType,
    /// A `%` marker is still open: name the directive to finish it.
    Directive,
    Task,
    Drawer,
    DrawerText,
    /// A complete first-level drawer shortcut still needs nested notes.
    DrawerNotes,
    ActiveTask,
    TaskLink,
    /// A whole-draft `/query` Go To picker is still open.
    TaskGoto,
    /// A bare-plus parent-task selector has not been accepted yet.
    TaskParent,
    PomodoroCloseTask,
    PomodoroCloseLogText,
    PomodoroStartTask,
    PomodoroStartTiming,
    /// A prerequisite picker is still open: an `&` modifier is partial.
    TaskDependency,
    /// A blocked-task picker is still open: a `*` modifier is partial.
    BlockedTask,
    /// A relation picker is still open: a `~` / `~~` target is partial.
    RelationTarget,
    /// A relation label is still open: `|label` after a relation target.
    RelationLabel,
    /// No dependent was named: add task text or `@note+task-id`.
    DependencyTarget,
    /// A completion picker is still open: a `!` token is partial.
    TaskComplete,
    /// A `=/` split is still open: a trailing `#` needs a task type.
    PomodoroSplitType,
    /// A `=*` merge is still open: a destination after `#` is missing.
    PomodoroMergeDest,
    /// A template draft still needs a row to expand.
    TemplateRow,
}

impl Need {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Route => "route",
            Self::Section => "section",
            Self::BlockId => "block_id",
            Self::PomodoroId => "pomodoro_id",
            Self::PomodoroName => "pomodoro_name",
            Self::TaskType => "task_type",
            Self::Directive => "directive",
            Self::Task => "task",
            Self::Drawer => "drawer",
            Self::DrawerText => "drawer_text",
            Self::DrawerNotes => "drawer_notes",
            Self::ActiveTask => "active_task",
            Self::TaskLink => "task_link",
            Self::TaskGoto => "task_goto",
            Self::TaskParent => "task_parent",
            Self::PomodoroCloseTask => "pomodoro_close_task",
            Self::PomodoroCloseLogText => "pomodoro_close_log_text",
            Self::PomodoroStartTask => "pomodoro_start_task",
            Self::PomodoroStartTiming => "pomodoro_start_timing",
            Self::TaskDependency => "task_dependency",
            Self::BlockedTask => "blocked_task",
            Self::RelationTarget => "relation_target",
            Self::RelationLabel => "relation_label",
            Self::DependencyTarget => "dependency_target",
            Self::TaskComplete => "task_complete",
            Self::PomodoroSplitType => "pomodoro_split_type",
            Self::PomodoroMergeDest => "pomodoro_merge_dest",
            Self::TemplateRow => "template_row",
        }
    }
}

/// One complete `&note:block-id` modifier on an item, in source order.
/// Additive schema-version-1 data: the discovery and writer phases add
/// resolution results without changing these lexical fields.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct DependencyEntry {
    /// Exact typed text, sigil included.
    pub(crate) raw: String,
    /// Vault-relative note identity (quotes removed, escapes decoded).
    pub(crate) note: String,
    /// Prerequisite block ID.
    pub(crate) block_id: String,
    /// Whether the note was a `"quoted component"`.
    pub(crate) quoted: bool,
    /// Whole-modifier range, sigil included.
    pub(crate) start: usize,
    pub(crate) end: usize,
}

/// One complete `*note:block-id` modifier. Its wire shape matches
/// `DependencyEntry`; the distinct field and span kinds carry direction.
pub(crate) type BlockedTaskEntry = DependencyEntry;

/// Direction of a `~` / `~~` relation modifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum RelationDirection {
    To,
    From,
}

/// One complete `~note:block-id[|label]` modifier on an item, in source
/// order. Additive schema-version-1 data alongside `dependencies`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct RelationEntry {
    /// Exact typed text, sigil included.
    pub(crate) raw: String,
    pub(crate) direction: RelationDirection,
    /// Vault-relative note identity (quotes removed, escapes decoded).
    pub(crate) note: String,
    /// Related task block ID.
    pub(crate) block_id: String,
    /// Whether the note was a `"quoted component"`.
    pub(crate) quoted: bool,
    /// True when the label separator was `||`. Omitted when false, so
    /// every older input keeps its exact JSON shape.
    #[serde(skip_serializing_if = "is_false")]
    pub(crate) hidden: bool,
    /// Normalised label, when `|label` was typed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) label: Option<String>,
    /// Whole-modifier range, sigil included.
    pub(crate) start: usize,
    pub(crate) end: usize,
    /// Label text range (inside quotes when quoted), when a label was typed.
    #[serde(skip)]
    pub(crate) label_start: Option<usize>,
    /// Label text end, when a label was typed.
    #[serde(skip)]
    pub(crate) label_end: Option<usize>,
}

/// One whole-item `!note:block-id` completion token, in lexical form.
/// Additive schema-version-1 data: the picker and writer phases add
/// resolution results without changing these lexical fields.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct TaskCompleteEntry {
    /// Exact typed text, sigil included.
    pub(crate) raw: String,
    /// Vault-relative note identity (quotes removed, escapes decoded).
    pub(crate) note: String,
    /// Task block ID.
    pub(crate) block_id: String,
    /// Whether the note was a `"quoted component"`.
    pub(crate) quoted: bool,
    /// Whole-token range, sigil included.
    pub(crate) start: usize,
    /// Whole-token range, sigil included.
    pub(crate) end: usize,
}

/// Which task owns an item's dependency modifiers: a task this capture
/// creates (`new_task`) or an explicitly selected existing task
/// (`existing_task`). Absent when no dependent is named yet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct DependencyTarget {
    pub(crate) kind: DependencyTargetKind,
    /// Resolved route for the dependent, when one is known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) route: Option<String>,
    /// Existing dependent's block ID, when one is known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) block_id: Option<String>,
    /// Whether the owner route came from an inherited `@@route` declaration
    /// rather than an explicit item marker. Omitted when false.
    #[serde(skip_serializing_if = "is_false")]
    pub(crate) inherited: bool,
}

fn is_false(value: &bool) -> bool {
    !*value
}

/// Ownership vocabulary for [`DependencyTarget`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum DependencyTargetKind {
    NewTask,
    ExistingTask,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct EditorParse {
    pub(crate) body: String,
    pub(crate) mode: EditorMode,
    pub(crate) route: Option<String>,
    pub(crate) section: Option<String>,
    pub(crate) block_id: Option<String>,
    pub(crate) needs: Vec<Need>,
    /// Whether `%l` requested companion audio on this `Ref` item. Always
    /// false for non-`Ref` modes.
    pub(crate) ref_listen: bool,
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
    /// Validated additive whole-item `=/` split spec, when the item is an
    /// exact split token. `None` for every older input.
    pub(crate) pomodoro_split: Option<PomodoroSplitSpec>,
    /// Validated additive whole-item `=*` merge spec, when the item is an
    /// exact merge token. `None` for every older input.
    pub(crate) pomodoro_merge: Option<PomodoroMergeSpec>,
    /// Validated additive `=x` close suffix, when the resolved marker or
    /// whole item carries one. `None` for every older input, so
    /// version-tolerant readers see no change. Purely lexical.
    pub(crate) pomodoro_close: Option<PomodoroCloseSpec>,
    /// Complete `&note:block-id` modifiers on the first item, in source
    /// order. Empty for every older input.
    pub(crate) dependencies: Vec<DependencyEntry>,
    /// Complete `*note:block-id` modifiers on the first item, in source
    /// order. Empty for every older input.
    pub(crate) blocked_tasks: Vec<BlockedTaskEntry>,
    /// Complete `~` / `~~` modifiers on the first item, in source order.
    /// Empty for every older input.
    pub(crate) relations: Vec<RelationEntry>,
    /// Ownership of the first item's modifiers, when a dependent is named.
    pub(crate) dependency_target: Option<DependencyTarget>,
    /// Whole-item `!note:block-id` completion token on the first item,
    /// when claimed. Mirrors how top-level `dependencies` is emitted.
    pub(crate) task_complete: Option<TaskCompleteEntry>,
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
    /// Whether `%l` requested companion audio on this `Ref` item. Always
    /// false for non-`Ref` modes.
    pub(crate) ref_listen: bool,
    pub(crate) pomodoro_start: Option<PomodoroStartSpec>,
    pub(crate) pomodoro_adjust: Option<PomodoroAdjustSpec>,
    pub(crate) pomodoro_shift: Option<PomodoroShiftSpec>,
    pub(crate) pomodoro_split: Option<PomodoroSplitSpec>,
    pub(crate) pomodoro_merge: Option<PomodoroMergeSpec>,
    pub(crate) pomodoro_close: Option<PomodoroCloseSpec>,
    /// Complete `&note:block-id` modifiers on this item, in source order.
    pub(crate) dependencies: Vec<DependencyEntry>,
    /// Complete `*note:block-id` modifiers on this item, in source order.
    pub(crate) blocked_tasks: Vec<BlockedTaskEntry>,
    /// Complete `~` / `~~` modifiers on this item, in source order.
    pub(crate) relations: Vec<RelationEntry>,
    /// Ownership of this item's modifiers, when a dependent is named.
    pub(crate) dependency_target: Option<DependencyTarget>,
    /// Whole-item `!note:block-id` completion token, when claimed.
    pub(crate) task_complete: Option<TaskCompleteEntry>,
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
    /// Where this item's first-level bullets land, when a bullet-start `#`
    /// would name a drawer there. Drives bullet drawer completion.
    pub(crate) drawer_bullet_target:
        Option<super::drawer_bullet::DrawerBulletTarget>,
}

/// One complete local destination marker token an item owns, with the span
/// the marker text (`@route`, `@route+block-id`, `@route#Section`,
/// `@route^block-id`, `@route:block-id`, or `@route:block-id#pomodoro`)
/// occupies in the original draft.
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
/// the same rule `parse_capture_text_with_options` applies.
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
