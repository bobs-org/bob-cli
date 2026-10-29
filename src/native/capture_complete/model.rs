use serde::Serialize;

use crate::native::{
    capture_block_ids,
    capture_completable_tasks::{CompletableGroup, TodayInfo},
    capture_language::{CompletionContext, RelationDirection},
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
    pub(super) kind: RouteCandidateKind,
    pub(super) status: Option<String>,
    /// Additive alias-match marker: `Some("alias")` when the query matched
    /// a `project_name_aliases` entry and `replacement`/`route` carry the
    /// canonical route. Omitted for canonical matches so schema version 1
    /// is unchanged for older inputs.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) match_kind: Option<&'static str>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum RouteCandidateKind {
    Inbox,
    Area,
    Project,
    Template,
}

impl From<CaptureTargetKind> for RouteCandidateKind {
    fn from(kind: CaptureTargetKind) -> Self {
        match kind {
            CaptureTargetKind::Inbox => Self::Inbox,
            CaptureTargetKind::Area => Self::Area,
            CaptureTargetKind::Project => Self::Project,
        }
    }
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) block_id_suggestions: Option<Vec<String>>,
    pub(super) status_symbol: char,
    pub(super) status_name: String,
    pub(super) status_type: &'static str,
    pub(super) text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) disabled_reason: Option<String>,
    pub(super) section: Option<String>,
    pub(super) depth: usize,
    pub(super) child_count: usize,
    pub(super) line: usize,
    pub(super) pomodoro: Option<ActiveTaskPomodoroCandidate>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) task_kind: Option<&'static str>,
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

/// One `task_dependency` (`&` picker) candidate: any task in the
/// vault-wide catalog the discovery phase scans. The JSON keys match the
/// picker contract: `replacement` is the Bob-authored `&note:block-id`
/// (or quoted `&"Note":block-id`) an accept inserts -- empty for ID-less
/// or guarded rows, which clients must never insert directly but resolve
/// through the explicit Add block ID flow instead. `note_path` is the
/// exact vault-relative path including extension (never the lowercased
/// display route); `locator` is the short human display form;
/// `already_dependency` marks prerequisites already on the dependent's
/// line (accepting one is a harmless no-op); `disabled_reason` explains a
/// guarded row. The contract phase freezes this shape with an empty list;
/// the discovery phase populates it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct DependencyCandidate {
    pub(super) replacement: String,
    #[serde(rename = "ref")]
    pub(super) task_ref: String,
    pub(super) note_path: String,
    pub(super) locator: String,
    pub(super) group: String,
    /// A `#hide` task: the client renders it subdued. Always present
    /// so row shape never depends on vault contents.
    pub(super) hidden: bool,
    pub(super) block_id: Option<String>,
    pub(super) requires_block_id: bool,
    pub(super) block_id_suggestions: Vec<String>,
    #[serde(skip_serializing_if = "is_false")]
    pub(super) already_dependency: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) disabled_reason: Option<String>,
    pub(super) status_symbol: char,
    pub(super) status_name: String,
    pub(super) status_type: &'static str,
    pub(super) text: String,
    pub(super) section: Option<String>,
    pub(super) depth: usize,
    pub(super) line: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) task_kind: Option<&'static str>,
}

/// One `blocked_task` (`*` picker) candidate. It shares the identity,
/// display, ID-assignment, and task fields of `DependencyCandidate`, with
/// `already_blocked` reporting an existing or already-typed edge from this
/// task to the capture owner.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct BlockedTaskCandidate {
    pub(super) replacement: String,
    #[serde(rename = "ref")]
    pub(super) task_ref: String,
    pub(super) note_path: String,
    pub(super) locator: String,
    pub(super) group: String,
    pub(super) hidden: bool,
    pub(super) block_id: Option<String>,
    pub(super) requires_block_id: bool,
    pub(super) block_id_suggestions: Vec<String>,
    #[serde(skip_serializing_if = "is_false")]
    pub(super) already_blocked: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) disabled_reason: Option<String>,
    pub(super) status_symbol: char,
    pub(super) status_name: String,
    pub(super) status_type: &'static str,
    pub(super) text: String,
    pub(super) section: Option<String>,
    pub(super) depth: usize,
    pub(super) line: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) task_kind: Option<&'static str>,
}

/// One `relation_to` / `relation_from` task candidate. Its identity and
/// display fields mirror `DependencyCandidate`; relation metadata stays
/// additive and omitted when it does not apply.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct RelationCandidate {
    pub(super) replacement: String,
    #[serde(rename = "ref")]
    pub(super) task_ref: String,
    pub(super) note_path: String,
    pub(super) locator: String,
    pub(super) group: String,
    pub(super) hidden: bool,
    pub(super) block_id: Option<String>,
    pub(super) requires_block_id: bool,
    pub(super) block_id_suggestions: Vec<String>,
    #[serde(skip_serializing_if = "is_false")]
    pub(super) already_linked: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) existing_label: Option<String>,
    #[serde(skip_serializing_if = "is_false")]
    pub(super) also_prerequisite: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) disabled_reason: Option<String>,
    pub(super) status_symbol: char,
    pub(super) status_name: String,
    pub(super) status_type: &'static str,
    pub(super) text: String,
    pub(super) section: Option<String>,
    pub(super) depth: usize,
    pub(super) line: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) task_kind: Option<&'static str>,
}

/// One canonical relation label suggestion. Corpus rows carry an occurrence
/// count; a typed row is first and has `typed: true` instead.
/// `replacement` keeps the probe's pipe count (`|` or `||`);
/// `hide_replacement` is always the `||` form, so the app never builds
/// either token itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct RelationLabelSuggestion {
    pub(super) label: String,
    pub(super) key: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) inverse: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) count: Option<usize>,
    pub(super) replacement: String,
    pub(super) hide_replacement: String,
    #[serde(skip_serializing_if = "is_false")]
    pub(super) typed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) disabled_reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) existing: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) hidden: Option<bool>,
    /// True when the row's key is a configured cancel label
    /// (`relations.cancel_labels`). Omitted when false, so responses
    /// without cancel labels stay byte-identical.
    #[serde(skip_serializing_if = "is_false")]
    pub(super) cancels_source: bool,
    /// True when the row's key is a configured complete label
    /// (`relations.complete_labels`). Omitted otherwise.
    #[serde(skip_serializing_if = "is_false")]
    pub(super) completes_source: bool,
    /// True on zero-use overlay rows synthesized from config (never a
    /// vault use). Omitted when false.
    #[serde(skip_serializing_if = "is_false")]
    pub(super) configured: bool,
}

/// The cancel consequence of the `relation_label` token under the
/// cursor (`docs/task-relations.md` §17 CL7). Present only when at
/// least one cancel label is configured. It resolves only the token's
/// source — the item owner for `~`, the `~~` token's task for `~~` —
/// and never scans the vault.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct RelationCancel {
    pub(super) source: RelationCancelSource,
    /// `cancels` | `already_closed` | `recurring` | `new_task`.
    pub(super) verdict: &'static str,
    pub(super) message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) hint: Option<String>,
}

/// The complete consequence of the `relation_label` token under the
/// cursor (`docs/task-relations.md` §18 CP7).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct RelationComplete {
    pub(super) source: RelationCompleteSource,
    /// `completes` | `already_closed` | `recurring` | `new_task`.
    pub(super) verdict: &'static str,
    pub(super) message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) hint: Option<String>,
}

/// The source task a complete label would complete.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct RelationCompleteSource {
    pub(super) path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) block_id: Option<String>,
    pub(super) text: String,
    pub(super) status_symbol: char,
    pub(super) status_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) project_open_tasks: Option<usize>,
}

/// The relation source a cancel label would cancel: vault-relative
/// `path` with extension, the task `text`, and its status. `block_id`
/// is omitted for ID-less owners; `project_open_tasks` counts the
/// source note's open tasks when the source is a `^prj` project task.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct RelationCancelSource {
    pub(super) path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) block_id: Option<String>,
    pub(super) text: String,
    pub(super) status_symbol: char,
    pub(super) status_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) project_open_tasks: Option<usize>,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) task_kind: Option<&'static str>,
}

/// One `task_goto` (Go To picker) candidate: any linkable open task in
/// the same routable catalog `task_link` uses, in the same rows and order.
/// The JSON keys match the picker contract: `replacement` is always `""`
/// (clients never insert it; it is kept so the shared decoder still
/// works), `note_path` is the exact vault-relative path with extension,
/// and `target` is the absolute path (`bob_dir.join(note_path)`). There
/// is no `requires_block_id`, `block_id_suggestions`, or `pulls_forward`:
/// every row opens directly, including ID-less ones. `scheduled` is shown
/// as context only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct TaskGotoCandidate {
    pub(super) replacement: String,
    #[serde(rename = "ref")]
    pub(super) task_ref: String,
    pub(super) route: String,
    pub(super) note_kind: CaptureTargetKind,
    pub(super) note_path: String,
    pub(super) target: String,
    pub(super) block_id: Option<String>,
    pub(super) status_symbol: char,
    pub(super) status_name: String,
    pub(super) status_type: &'static str,
    pub(super) text: String,
    pub(super) section: Option<String>,
    pub(super) depth: usize,
    pub(super) line: usize,
    pub(super) group: capture_link_tasks::LinkTaskGroup,
    pub(super) scheduled: Option<String>,
    pub(super) pomodoro: Option<ActiveTaskPomodoroCandidate>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) task_kind: Option<&'static str>,
}

/// One `task_parent` picker candidate: a linkable open task in the capture
/// target catalog, with a parent-task `@route+id` replacement. ID-less
/// candidates have no replacement and must go through Add block ID first.
/// Colon-only schedule pull-forward metadata is intentionally absent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct TaskParentCandidate {
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) disabled_reason: Option<String>,
    pub(super) section: Option<String>,
    pub(super) depth: usize,
    pub(super) line: usize,
    pub(super) group: capture_link_tasks::LinkTaskGroup,
    pub(super) pomodoro: Option<ActiveTaskPomodoroCandidate>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) task_kind: Option<&'static str>,
}

/// One `task_complete` (`!` picker) candidate: any open task in the
/// vault-wide completable catalog. The JSON keys match the picker
/// contract: `replacement` is the Bob-authored `!note:block-id` (or
/// quoted `!"Note":block-id`) an accept inserts — empty for ID-less
/// or guarded rows, which clients must never insert directly but
/// resolve through the explicit Add block ID flow instead.
/// `note_path` is the exact vault-relative path including extension
/// (never the lowercased display route); `locator` is the short human
/// display form; `group` is `today`, `in_progress`, `next`, or
/// `open`; `today` carries the Task Link placement for today rows
/// only. `recurring` and `already_selected` mark guarded rows with an
/// explanatory `disabled_reason` and no insertable replacement.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct TaskCompleteCandidate {
    pub(super) replacement: String,
    #[serde(rename = "ref")]
    pub(super) task_ref: String,
    pub(super) note_path: String,
    pub(super) locator: String,
    pub(super) group: CompletableGroup,
    /// A `#hide` task: the client renders it subdued. Always present
    /// so row shape never depends on vault contents.
    pub(super) hidden: bool,
    pub(super) block_id: Option<String>,
    pub(super) requires_block_id: bool,
    pub(super) block_id_suggestions: Vec<String>,
    #[serde(skip_serializing_if = "is_false")]
    pub(super) recurring: bool,
    #[serde(skip_serializing_if = "is_false")]
    pub(super) already_selected: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) disabled_reason: Option<String>,
    pub(super) status_symbol: char,
    pub(super) status_name: String,
    pub(super) status_type: &'static str,
    pub(super) text: String,
    pub(super) section: Option<String>,
    pub(super) depth: usize,
    pub(super) line: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) scheduled: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) today: Option<TodayInfo>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) task_kind: Option<&'static str>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum PickerKind {
    ParentTask,
    TaskComplete,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum PickerScope {
    Note,
    Vault,
}

/// Additive server-authored picker lifecycle metadata for parent-task
/// completion. All ranges are half-open UTF-8 byte offsets into `TEXT`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct PickerDescriptor {
    pub(super) kind: PickerKind,
    pub(super) scope: PickerScope,
    pub(super) scope_token: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) note_target: Option<String>,
    pub(super) marker_range: Replacement,
    pub(super) trigger_removal_range: Replacement,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) action_continuation_keys: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) relationship: Option<PickerRelationship>,
}

/// Complete typed relationships on the picker item's owner task. Text is
/// omitted when a marker does not resolve to one task in the vault.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct PickerRelationship {
    pub(super) dependencies: Vec<PickerRelationshipEntry>,
    pub(super) blocked_tasks: Vec<PickerRelationshipEntry>,
    pub(super) relations: Vec<PickerRelationEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct PickerRelationEntry {
    pub(super) direction: RelationDirection,
    pub(super) text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct PickerRelationshipEntry {
    pub(super) raw: String,
    pub(super) note_path: String,
    pub(super) block_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) text: Option<String>,
}

/// The relation token behind a `relation_label` completion: its
/// direction (`to` for `~`, `from` for `~~`, spelled like `capture-parse`
/// `relations[].direction`) plus the draft range from the sigil's first
/// byte to the `|` separator, so `range.end` always equals
/// `replacement.start` and clients never re-lex the draft to find the
/// token. Omitted from every other context.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub(super) struct CompletionRelation {
    pub(super) direction: RelationDirection,
    pub(super) range: Replacement,
}

/// Enriched completion owner. Flattening preserves the existing lexical
/// owner fields while adding optional resolved display metadata.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct CompletionOwner {
    #[serde(flatten)]
    pub(super) target: crate::native::capture_language::DependencyTarget,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) note_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) status_symbol: Option<char>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) status_name: Option<String>,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) task_kind: Option<&'static str>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct PomodoroNameCandidate {
    pub(super) replacement: String,
    #[serde(rename = "ref", skip_serializing_if = "Option::is_none")]
    pub(super) pomodoro_ref: Option<capture_pomodoros::PomodoroRef>,
    pub(super) name: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(super) themes: Vec<crate::native::pomodoro_themes::ThemeRef>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) origin: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) last_used: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) days_used: Option<u32>,
    #[serde(skip_serializing_if = "is_false")]
    pub(super) hidden: bool,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(super) similar_to: Vec<String>,
    pub(super) requires_name: bool,
    #[serde(skip_serializing_if = "is_false")]
    pub(super) creates_pomodoro: bool,
    /// Marks the row `bob capture` with a bare `=` would start. Only set
    /// in the `pomodoro_start_name` context; always false (and therefore
    /// omitted) in the `pomodoro_name` context so that output stays
    /// byte-identical.
    #[serde(skip_serializing_if = "is_false")]
    pub(super) next_up: bool,
    /// Task-aware menu row selected by the shared PT12 preselection rule.
    #[serde(skip_serializing_if = "is_false")]
    pub(super) suggested: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) suggestion_label: Option<String>,
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
    TaskGoto(Vec<TaskGotoCandidate>),
    TaskParent(Vec<TaskParentCandidate>),
    Dependency(Vec<DependencyCandidate>),
    BlockedTask(Vec<BlockedTaskCandidate>),
    Relation(Vec<RelationCandidate>),
    RelationLabel(Vec<RelationLabelSuggestion>),
    TaskComplete(Vec<TaskCompleteCandidate>),
    WikilinkNote(Vec<WikilinkNoteCandidate>),
    WikilinkHeading(Vec<WikilinkHeadingCandidate>),
    WikilinkBlock(Vec<WikilinkBlockCandidate>),
}

impl Candidates {
    pub(super) fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub(super) fn len(&self) -> usize {
        match self {
            Self::Route(items) => items.len(),
            Self::Section(items) => items.len(),
            Self::Task(items) => items.len(),
            Self::TaskSection(items) => items.len(),
            Self::PomodoroName(items) => items.len(),
            Self::ActiveTask(items) => items.len(),
            Self::TaskLink(items) => items.len(),
            Self::TaskGoto(items) => items.len(),
            Self::TaskParent(items) => items.len(),
            Self::Dependency(items) => items.len(),
            Self::BlockedTask(items) => items.len(),
            Self::Relation(items) => items.len(),
            Self::RelationLabel(items) => items.len(),
            Self::TaskComplete(items) => items.len(),
            Self::WikilinkNote(items) => items.len(),
            Self::WikilinkHeading(items) => items.len(),
            Self::WikilinkBlock(items) => items.len(),
        }
    }
}

/// The running Pomodoro a named `=` start would restart or swap. `pomodoro_name`
/// is absent for an unnamed running session; `line` is the 1-based day-file
/// line and `time_range` the ledger span the swap keeps (`0920-0945`).
/// `homes` is the derived R9 object for unnamed sessions only; named
/// sessions do no resolution and omit it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct OverrideRunningSession {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) pomodoro_name: Option<String>,
    pub(super) line: usize,
    pub(super) time_range: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) homes: Option<crate::native::pomodoro_homes::Homes>,
}

/// Additive override context for a named-start name field: present when
/// exactly one timed session runs and no earlier item or same-line chain
/// token is a whole-item session operator. `keeps_ledger` is true when the
/// `<X>` suffix is empty (a swap takes over the running ledger
/// byte-for-byte). `running` is always set when the object is present.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct OverrideCompletion {
    pub(super) keeps_ledger: bool,
    pub(super) running: OverrideRunningSession,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct CaptureCompleteResult {
    pub(super) ok: bool,
    pub(super) schema_version: u32,
    pub(super) cursor: usize,
    pub(super) replacement: Replacement,
    pub(super) context: Option<CompletionContext>,
    pub(super) candidates: Candidates,
    /// `None` outside `pomodoro_name`; `Some(None)` means that context
    /// ran without a task theme suggestion.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) theme_suggestion: Option<
        Option<crate::native::pomodoro_themes::preselect::MenuSuggestion>,
    >,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) r#override: Option<OverrideCompletion>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) block_id: Option<capture_block_ids::BlockIdField>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(super) warnings: Vec<String>,
    /// Search text for `task_dependency`, `blocked_task`, scoped `task`,
    /// bare-plus `task_parent`, `task_complete`, and relation target and
    /// label completion. Relation queries have their sigil,
    /// opening quote, and escapes decoded so clients never parse a quoted
    /// note component. Omitted from other contexts.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) query: Option<String>,
    /// Lexical owner of the `task_dependency`, `blocked_task`, `relation_to`,
    /// `relation_from`, or `relation_label` modifier
    /// under the cursor (the capture-parse `dependency_target` for the
    /// cursor's item), so clients never derive the owner themselves.
    /// Set only for those contexts.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) owner: Option<CompletionOwner>,
    /// The relation token behind a `relation_label` completion; omitted
    /// from every other context.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) relation: Option<CompletionRelation>,
    /// The cancel consequence of the `relation_label` token (§17 CL7).
    /// Present only when at least one cancel label is configured, and
    /// only in the `relation_label` context.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) relation_cancel: Option<RelationCancel>,
    /// The complete consequence of a configured complete-label token.
    /// Present only when complete labels are configured.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) relation_complete: Option<RelationComplete>,
    /// True when the `relation_label` probe sits in a `||` label.
    /// Omitted from every other context and from `|` probes, so older
    /// responses stay byte-identical.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) one_off: Option<bool>,
    /// Parent-task picker scope and editing ranges; omitted from every
    /// other completion context.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) picker: Option<PickerDescriptor>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) hidden_count: Option<usize>,
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
            theme_suggestion: None,
            r#override: None,
            block_id: None,
            warnings: Vec::new(),
            query: None,
            owner: None,
            relation: None,
            relation_cancel: None,
            relation_complete: None,
            picker: None,
            hidden_count: None,
            one_off: None,
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
