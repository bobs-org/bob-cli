//! Capture result/error types and human/JSON output.
use super::*;
use std::borrow::Cow;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Placement {
    Created,
    Inserted,
    Appended,
    Toggled,
    Linked,
    Closed,
    Started,
    Updated,
    Completed,
    Queued,
    Unchanged,
    Recorded,
}

/// Additive `ref` object on a reference item result: the classified
/// URL, its offline library verdict, the canonical resolved parent, the
/// staged job (real runs that queued only), and the parent fallback
/// (queued items only). `listen` is true when `%l` requested companion
/// audio; `listen_hint` explains an unchanged marked item's no-new-listen
/// outcome with the explicit retry command.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct RefItemJson {
    pub(super) url: String,
    pub(super) cleaned_url: String,
    pub(super) dedupe_key: String,
    pub(super) display: String,
    pub(super) route_hint: &'static str,
    #[serde(default, skip_serializing_if = "is_false")]
    pub(super) listen: bool,
    pub(super) library: RefLibraryJson,
    pub(super) parent: RefParentJson,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) job: Option<RefJobJson>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) fallback: Option<RefFallbackJson>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) listen_hint: Option<String>,
}

/// Canonical resolved reference parent: the route, its label, its kind,
/// how the input token was selected (`explicit`|`global`|`default`), and
/// the matched alias (or null).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct RefParentJson {
    pub(super) route: String,
    pub(super) label: String,
    pub(super) kind: &'static str,
    pub(super) source: &'static str,
    pub(super) alias: Option<String>,
}

/// Offline library verdict on a reference item.
///
/// All keys are always present (`null` when absent) per the capture JSON
/// contract; Bob Mac Capture decodes them with `decodeIfPresent`, so both
/// forms decode.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct RefLibraryJson {
    pub(super) verdict: &'static str,
    pub(super) path: Option<String>,
    pub(super) title: Option<String>,
    pub(super) reading_state: Option<String>,
    pub(super) message: Option<String>,
}

/// Staged ref job on a real run that queued the link.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct RefJobJson {
    pub(super) id: String,
    pub(super) state: &'static str,
}

/// Inbox fallback on a queued reference item: where the task goes
/// when the background clip fails.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct RefFallbackJson {
    pub(super) relative_target: String,
    pub(super) task_line: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct PomodoroCloseTimingJson {
    pub(super) start: String,
    pub(super) end: String,
    pub(super) duration_minutes: u64,
    pub(super) time_range: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct PomodoroCloseTaskLinkClockJson {
    pub(super) class: &'static str,
    pub(super) span: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct PomodoroCloseTaskLinkJson {
    pub(super) index: u32,
    pub(super) ledger_line: usize,
    pub(super) block_link: String,
    pub(super) block_id: String,
    pub(super) marker: &'static str,
    pub(super) outcome: &'static str,
    pub(super) source: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) clock: Option<PomodoroCloseTaskLinkClockJson>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct PomodoroCloseLogEntryJson {
    pub(super) index: u32,
    pub(super) text: String,
    /// Nested detail lines under the entry, omitted when empty.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(super) details: Vec<String>,
    /// `true` when this entry came from a `- *` star bullet that logs to
    /// every remaining worked task. Omitted when false.
    #[serde(default, skip_serializing_if = "is_false")]
    pub(super) all: bool,
}

fn typed_work_log_details_is_empty(details: &[Vec<String>]) -> bool {
    details.iter().all(Vec::is_empty)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct PomodoroCloseTaskJson {
    pub(super) role: &'static str,
    pub(super) block_link: String,
    pub(super) ledger_line: usize,
    pub(super) index: Option<u32>,
    pub(super) resolved: bool,
    // Explicit nulls on unresolved rows, matching the top-level
    // `route: null` / `scheduled: null` convention.
    pub(super) relative_target: Option<String>,
    pub(super) block_id: String,
    pub(super) text: Option<String>,
    pub(super) previous_status_symbol: Option<char>,
    pub(super) previous_status_name: Option<String>,
    pub(super) status_symbol: Option<char>,
    pub(super) status_name: Option<String>,
    pub(super) status_changed: bool,
    pub(super) carried: bool,
    pub(super) work_log: Vec<String>,
    pub(super) work_log_created: bool,
    /// Dated entries this close's typed Work Log entries produced, in typed
    /// order (a subset of `work_log`), omitted when empty.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(super) typed_work_log: Vec<String>,
    /// Detail lines written under each typed entry, aligned 1:1 with
    /// `typed_work_log`; omitted when no typed entry has details.
    #[serde(default, skip_serializing_if = "typed_work_log_details_is_empty")]
    pub(super) typed_work_log_details: Vec<Vec<String>>,
    pub(super) warning: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct PomodoroCloseCarriedJson {
    pub(super) kind: &'static str,
    pub(super) text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct PomodoroCloseNextJson {
    pub(super) line: usize,
    /// The continuation's name: the closed entry's name with the types
    /// whose last Task Link did not carry shed (R6).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) name: Option<String>,
    pub(super) time_range: Option<String>,
    pub(super) created: bool,
    /// True when a MEET close moved the created continuation after the
    /// last open Pomodoro below the closed entry. Omitted when false.
    #[serde(default, skip_serializing_if = "is_false")]
    pub(super) placed_last: bool,
    /// Type components the created continuation shed, omitted when none.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(super) shed: Vec<super::super::pomodoro_themes::ThemeRef>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct PomodoroCloseSizeFromJson {
    pub(super) time_range: String,
    pub(super) duration_minutes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct PomodoroCloseSizeJson {
    pub(super) raw: String,
    pub(super) blocks: u32,
    pub(super) minutes: u32,
    pub(super) from: PomodoroCloseSizeFromJson,
    pub(super) delta_minutes: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct PomodoroCloseSummaryJson {
    pub(super) raw: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) size: Option<PomodoroCloseSizeJson>,
    pub(super) in_progress: Option<Vec<u32>>,
    /// Parked `*<P>` list, omitted when empty. Parked links get normal
    /// In Progress work effects but are not carried forward.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(super) park: Vec<u32>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub(super) park_all: bool,
    pub(super) complete: Vec<u32>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub(super) complete_all: bool,
    /// Dropped `~<K>` list, omitted when empty.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(super) drop: Vec<u32>,
    /// Typed Work Log entries in typed order, omitted when empty.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(super) log: Vec<PomodoroCloseLogEntryJson>,
    pub(super) task_links: Vec<PomodoroCloseTaskLinkJson>,
    pub(super) pomodoro_line: usize,
    /// Original entry position relative to the Pomodoros section. This is
    /// planner-only identity used to recover the same row after Work Log
    /// insertion shifts the section.
    #[serde(skip)]
    pub(super) pomodoro_section_offset: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) pomodoro_name: Option<String>,
    /// The completed entry's name after the close (R6 narrow); equal to
    /// `pomodoro_name` when nothing narrows. Present when the entry is
    /// named.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) closed_name: Option<String>,
    // Vault-relative day file, so human output and clients name the day
    // file even on link forms (whose `relative_target` is the route note).
    pub(super) day_relative: String,
    pub(super) entry_line: String,
    pub(super) planned: PomodoroCloseTimingJson,
    pub(super) closed: PomodoroCloseTimingJson,
    pub(super) closed_at: String,
    pub(super) remaining_minutes: i64,
    pub(super) decremented_minutes: u64,
    /// Whether the closed session is off the work clock. Omitted when false.
    #[serde(skip_serializing_if = "is_false")]
    pub(super) off_clock: bool,
    /// Shared inherited note when the via rule applies. Omitted otherwise.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) clock_note: Option<String>,
    pub(super) tasks: Vec<PomodoroCloseTaskJson>,
    pub(super) carried: Vec<PomodoroCloseCarriedJson>,
    pub(super) notes: Vec<String>,
    pub(super) next_pomodoro: Option<PomodoroCloseNextJson>,
    /// Dependents this close fully unblocked and linked into the
    /// continuation, or recovered with the reason they were not linked
    /// (`docs/task-dependencies.md` §12.5). Empty when the close
    /// completed nothing with an `[id::]` identity.
    pub(super) unblocked: Vec<TaskCompleteUnblockedJson>,
    /// Keep-moving offers (`docs/task-dependencies.md` §12.15), always
    /// present and possibly empty, in schema version 1.
    pub(super) keep_moving: Vec<TaskCompleteUnblockedJson>,
    /// Relation-winning successor offers. Omitted when no relation event
    /// or configured edge applies (`docs/task-dependencies.md` §12.5).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(super) relation_successors: Vec<TaskCompleteUnblockedJson>,
    /// Relation-held sources that never become offers. Omitted when empty.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(super) relation_held: Vec<RelationHeldJson>,
    /// `"checked"` or `"unavailable"` when the relation lookup ran.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) relation_check: Option<&'static str>,
    /// Surviving relation groups, kept for the presentation phase.
    #[serde(skip)]
    pub(super) relation_groups:
        Vec<crate::native::task_complete::RelationOfferGroup>,
    /// Open dependents of the completed tasks that stay blocked.
    pub(super) still_blocked: Vec<TaskCompleteStillBlockedJson>,
    /// `"checked"` when the dependents lookup ran or the `[id::]` gate
    /// proved it unnecessary; `"unavailable"` when the snapshot could
    /// not be built and the close succeeded without linking. Keep-moving
    /// safe rows still offer under `"unavailable"` (§12.15 KM31).
    pub(super) unblocked_check: &'static str,
    /// Project-group folded counts from this close's successor plan.
    /// Never serialized.
    #[serde(skip)]
    pub(super) project_folds: Vec<crate::native::task_complete::ProjectFold>,
    /// Keep-moving folded counts from this close's successor plan.
    /// Never serialized.
    #[serde(skip)]
    pub(super) keep_moving_folds:
        Vec<crate::native::task_complete::KeepMovingFold>,
}

/// Recorded past session: the nested close summary plus record identity.
/// `trims` holds the edge trims attributed to the first overlapping member
/// in typed order; empty on members that report no trim. `segment` is the
/// resolved timeline segment, present only on the run member that carries
/// the suffix.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct PomodoroPastSummaryJson {
    pub(super) raw: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) name: Option<String>,
    pub(super) target: PomodoroPastTargetJson,
    pub(super) blocks: u32,
    pub(super) repair: bool,
    pub(super) stack: PomodoroPastStackJson,
    pub(super) anchor: String,
    pub(super) window: PomodoroPastWindowJson,
    pub(super) closed_at: String,
    pub(super) line: usize,
    #[serde(default)]
    pub(super) trims: Vec<PomodoroPastTrimJson>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) segment: Option<PomodoroPastSegmentJson>,
    pub(super) close: Box<PomodoroCloseSummaryJson>,
}

/// Resolved timeline segment for the suffix-carrying member: `raw` is the
/// typed suffix (`~`, `~2`, `~2+`, `~~`); `kind` is `pomodoro`/`gap`/`tail`/
/// `head`; `edge` is `start`/`end`; `start`/`end` are `HHMM`; `minutes` is
/// the true segment length. `index`, `name`, and `line` appear only when
/// they apply.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct PomodoroPastSegmentJson {
    pub(super) raw: String,
    pub(super) kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) index: Option<u32>,
    pub(super) edge: String,
    pub(super) start: String,
    pub(super) end: String,
    pub(super) minutes: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) line: Option<usize>,
}

/// One trimmed closed edge: the span rewrite staged before any member
/// recorded. `minutes` is the old span minus the new one (what the whole
/// stack took); the human line shows the remaining piece in parentheses.
/// `calendar` flags MEET rows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct PomodoroPastTrimJson {
    pub(super) line: usize,
    pub(super) name: String,
    pub(super) before: String,
    pub(super) after: String,
    pub(super) minutes: u64,
    pub(super) clock: String,
    pub(super) calendar: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct PomodoroPastTargetJson {
    pub(super) source: String,
    pub(super) line: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct PomodoroPastStackJson {
    pub(super) index: u32,
    pub(super) size: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) pin: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct PomodoroPastWindowJson {
    pub(super) start: String,
    pub(super) end: String,
    pub(super) minutes: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) requested_minutes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) cut: Option<String>,
    /// Wall name for `wall` cuts (supports the human `cut at WORK` note).
    /// Omitted otherwise, so unsuffixed JSON stays byte-identical.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) wall: Option<String>,
    /// Pinned suffix raw (e.g. `~2`) for `segment` cuts on run members that
    /// do not carry the suffix themselves (supports `cut to fit ~2`).
    /// Omitted when the member's own `segment.raw` already names it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) fit: Option<String>,
}

/// One note-free `x0` reset: the current Pomodoro returned to the
/// front of the future queue with its session ledger cleared. Reset and
/// `pomodoro_close` are mutually exclusive; the parser still reports the
/// syntactic close input since it cannot know the vault-dependent outcome.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct PomodoroResetSummaryJson {
    pub(super) raw: String,
    pub(super) day_relative: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) pomodoro_name: Option<String>,
    pub(super) previous_pomodoro_line: usize,
    pub(super) pomodoro_line: usize,
    pub(super) previous_entry_line: String,
    pub(super) entry_line: String,
    pub(super) previous_time_range: String,
    pub(super) time_range: Option<String>,
    pub(super) created_pomodoro: bool,
    pub(super) moved: bool,
}

fn is_false(value: &bool) -> bool {
    !value
}

fn is_zero(value: &usize) -> bool {
    *value == 0
}

/// One capture warning. Plain strings keep every older draft's JSON
/// byte-identical; structured variants let machines discriminate by
/// `code` while clients render the bob-authored `message`
/// (`docs/task-relations.md` §12).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(untagged)]
pub(crate) enum CaptureWarning {
    Text(String),
    Structured(StructuredCaptureWarning),
    Suggestion(super::super::pomodoro_themes::suggest::SuggestWarning),
}

impl From<String> for CaptureWarning {
    fn from(message: String) -> Self {
        Self::Text(message)
    }
}

impl CaptureWarning {
    /// The human rendering of this warning, for stderr and dedup.
    pub(crate) fn message(&self) -> Cow<'_, str> {
        match self {
            Self::Text(message) => Cow::Borrowed(message),
            Self::Structured(warning) => Cow::Borrowed(&warning.message),
            Self::Suggestion(warning) => Cow::Owned(
                warning
                    .path
                    .as_deref()
                    .map(|path| format!("{} · {path}", warning.code))
                    .unwrap_or_else(|| warning.code.clone()),
            ),
        }
    }

    /// The plain-text rendering, when this warning is one.
    pub(super) fn as_text(&self) -> Option<&str> {
        match self {
            Self::Text(message) => Some(message),
            Self::Structured(_) | Self::Suggestion(_) => None,
        }
    }
}

/// A machine-readable capture warning: `code` selects the handling,
/// `message` is the bob-authored rendering, and `label`/`count` carry
/// the pre-capture catalog facts for `hides_existing_relation_label`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct StructuredCaptureWarning {
    pub(super) code: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) label: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) count: Option<u64>,
    pub(super) message: String,
}

/// The task-theme field change made by one explicit named capture choice.
/// `theme`, `previous`, and `inherited` intentionally serialize as null when
/// the corresponding state has no single valid theme.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct CaptureTaskThemeChange {
    pub(super) action: &'static str,
    pub(super) theme: Option<super::super::pomodoro_themes::ThemeRef>,
    pub(super) previous: Option<super::super::pomodoro_themes::ThemeRef>,
    pub(super) inherited:
        Option<super::super::pomodoro_themes::suggest::ThemeSuggestion>,
}

/// Ledger relocation applied so a retyped task's live Task Link agrees
/// with its new type (D8). Additive: omitted when the ledger needed no
/// change, so older decoders never see it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct TypeRelocationJson {
    pub(super) outcome: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) from: Option<String>,
    pub(super) to: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct TemplateSavedJson {
    pub(super) id: String,
    pub(super) status: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct CaptureResult {
    #[serde(flatten)]
    pub(super) item: CaptureItemResult,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(super) captures: Vec<CaptureItemResult>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) global_destination: Option<GlobalDestinationSummary>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) template: Option<capture_language::template::TemplateSummary>,
    #[serde(skip)]
    pub(super) template_rows: Vec<Option<capture_language::TemplateRowSummary>>,
    #[serde(skip)]
    pub(super) template_saved: Option<TemplateSavedJson>,
    #[serde(skip)]
    pub(super) template_not_saved: Option<String>,
    #[serde(skip)]
    pub(super) template_origin: Option<String>,
    /// Internal terminal-rendering lookup; it never crosses the JSON API.
    #[serde(skip)]
    pub(super) theme_refs:
        BTreeMap<String, Vec<super::super::pomodoro_themes::ThemeRef>>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(super) warnings: Vec<CaptureWarning>,
    /// Before/after plan budget, present only when the batch changed
    /// today's Pomodoros section. Never per item.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) plan_budget: Option<CapturePlanBudget>,
    /// Batch-level Pomodoro blocks in first-touch order, present only
    /// when the batch touched a Pomodoro. Never per item.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(super) pomodoro_blocks: Vec<PomodoroBlockJson>,
    /// Batch-level parent-task blocks in first-touch order, present only
    /// when the batch wrote a sub-bullet. Never per item.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(super) task_blocks: Vec<TaskBlockJson>,
    /// Batch-level sections receiving a transfer merge, omitted when empty.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(super) section_blocks: Vec<SectionBlockJson>,
    /// Notes Control-Return should open. Always present (empty when
    /// nothing opens). Never per item.
    pub(super) open_notes: Vec<OpenNoteJson>,
    /// Draft-level successor-offer summary. Present on every success.
    pub(super) pull_in: PullInJson,
    /// Applied card verdicts, in flag order. Omitted when empty.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(super) offer_closes: Vec<OfferCloseJson>,
    /// Inbox tasks whose checkbox status changed during this batch.
    pub(super) inbox_route: InboxRouteReport,
    /// Move-choice report. Present on every success.
    pub(super) move_choice: super::move_choice::MoveChoiceReport,
    /// Pomodoro name agreements this batch applied to today's daily note.
    /// Additive and omitted when empty, so older decoders stay valid.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(super) type_agreement: Vec<TypeAgreementJson>,
    /// Planned record files this batch creates. Additive: omitted when
    /// empty so older capture JSON stays byte-identical.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(super) note_files: Vec<NoteFileJson>,
    /// Staged album records-index edits, in path order. Additive and
    /// omitted when empty; dry-run and commit share these bytes.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(super) album_files: Vec<AlbumFileJson>,
}

/// One Pomodoro name agreement applied to today's daily note.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct TypeAgreementJson {
    pub(super) line: usize,
    pub(super) before: String,
    pub(super) after: String,
    pub(super) added: Vec<String>,
    pub(super) removed: Vec<String>,
    pub(super) reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct GlobalDestinationSummary {
    pub(super) route: String,
}

impl CaptureResult {
    pub(super) fn from_items(
        items: Vec<CaptureItemResult>,
        global_destination: Option<GlobalDestinationSummary>,
        template: Option<capture_language::template::TemplateSummary>,
        template_rows: Vec<Option<capture_language::TemplateRowSummary>>,
        warnings: Vec<CaptureWarning>,
        plan_budget: Option<CapturePlanBudget>,
        pomodoro_blocks: Vec<PomodoroBlockJson>,
        task_blocks: Vec<TaskBlockJson>,
        section_blocks: Vec<SectionBlockJson>,
        open_notes: Vec<OpenNoteJson>,
        pull_in: PullInJson,
        offer_closes: Vec<OfferCloseJson>,
        inbox_route: InboxRouteReport,
        move_choice: super::move_choice::MoveChoiceReport,
        type_agreement: Vec<TypeAgreementJson>,
        album_files: Vec<AlbumFileJson>,
    ) -> Self {
        let note_files = items
            .iter()
            .filter_map(|item| {
                item.record_note
                    .as_ref()
                    .map(|summary| summary.note_file.clone())
                    .or_else(|| {
                        item.notebook
                            .as_ref()
                            .and_then(|summary| summary.note_file.clone())
                    })
            })
            .collect();
        let item = items
            .first()
            .cloned()
            .expect("capture batch always contains at least one item");
        let captures = if items.len() > 1 { items } else { Vec::new() };
        Self {
            item,
            captures,
            global_destination,
            template,
            template_rows,
            template_saved: None,
            template_not_saved: None,
            template_origin: None,
            theme_refs: BTreeMap::new(),
            warnings,
            plan_budget,
            pomodoro_blocks,
            task_blocks,
            section_blocks,
            open_notes,
            pull_in,
            offer_closes,
            inbox_route,
            move_choice,
            type_agreement,
            note_files,
            album_files,
        }
    }
}

/// Batch-level album index changes. Dry-run shows the exact planned album
/// bytes once per album; a committed run names each updated album.
/// Record cards stay record-oriented; this section covers the album side
/// effect after the items that caused it.
fn print_album_files(result: &CaptureResult) {
    if result.album_files.is_empty() {
        return;
    }
    let styler = Styler::detect();
    if result.dry_run {
        for album in &result.album_files {
            println!();
            println!(
                "{}  {} ({})",
                styler.dim("index"),
                album.path,
                album.state,
            );
            print!("{}", album.after);
            if !album.after.ends_with('\n') {
                println!();
            }
        }
        return;
    }
    for album in &result.album_files {
        println!("  {}  {}", styler.dim("index"), album.path);
    }
}

impl std::ops::Deref for CaptureResult {
    type Target = CaptureItemResult;

    fn deref(&self) -> &Self::Target {
        &self.item
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct ProjectTaskLinkJson {
    pub(super) block_id: String,
    pub(super) block_link: String,
    pub(super) text: String,
    pub(super) task_line: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct ProjectNotePlannedLink {
    pub(super) entry_name: String,
    pub(super) entry_line: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct ProjectNoteMovedTask {
    pub(super) sigil: char,
    pub(super) draft_range: (usize, usize),
    pub(super) item_line: usize,
    pub(super) source: String,
    pub(super) source_line: usize,
    pub(super) line: usize,
    pub(super) block_id: String,
    pub(super) text: String,
    pub(super) previous_status_symbol: char,
    pub(super) previous_status_name: String,
    pub(super) status_symbol: char,
    pub(super) status_name: String,
    pub(super) fresh: Option<String>,
    pub(super) planned: Option<ProjectNotePlannedLink>,
    pub(super) link: Option<ProjectTaskLinkJson>,
    pub(super) not_linked: Option<String>,
    pub(super) detail: Option<String>,
    pub(super) via: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct ProjectNoteMoveSummary {
    pub(super) parent: String,
    pub(super) moved: usize,
    pub(super) repaired_links: usize,
    pub(super) repaired_notes: usize,
    pub(super) needs_reconcile: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct ProjectNoteRepairedFile {
    pub(super) path: String,
    pub(super) link_count: usize,
    pub(super) dependency_token_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct ProjectNoteMoveHint {
    pub(super) parent: String,
    pub(super) open_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct ProjectNoteSummary {
    pub(super) basename: String,
    pub(super) parent_route: String,
    pub(super) parent_link: String,
    pub(super) tasks: usize,
    pub(super) sections: Vec<String>,
    pub(super) task_links: Vec<ProjectTaskLinkJson>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(super) moved_tasks: Vec<ProjectNoteMovedTask>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) move_summary: Option<ProjectNoteMoveSummary>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) repaired_files: Option<Vec<ProjectNoteRepairedFile>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) move_hint: Option<ProjectNoteMoveHint>,
    /// `"offer"` for `@route:id@`; omitted on Create.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) form: Option<&'static str>,
    /// Successor-offer rows for the Offer form, including `[]`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) successors: Option<Vec<TaskCompleteUnblockedJson>>,
    /// Project-group folded counts for Offer aggregation. Never serialized.
    #[serde(skip)]
    pub(super) project_folds: Vec<crate::native::task_complete::ProjectFold>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct CaptureItemResult {
    pub(super) ok: bool,
    pub(super) dry_run: bool,
    pub(super) routed: bool,
    pub(super) route: Option<String>,
    pub(super) route_label: String,
    pub(super) relative_target: String,
    pub(super) target: String,
    pub(super) text: String,
    pub(super) task_line: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) task_theme: Option<CaptureTaskThemeChange>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) type_relocation: Option<TypeRelocationJson>,
    pub(super) kind: &'static str,
    /// Source target-run context. Ordinary captures omit this additive
    /// field so their JSON stays byte-compatible.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) fanout: Option<capture_language::FanoutContext>,
    pub(super) created: String,
    pub(super) scheduled: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) priority: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) priority_label: Option<String>,
    pub(super) placement: Placement,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(super) sub_bullets: Vec<String>,
    /// Multi-note notes in draft order, present only on a note-less
    /// `sub_bullet` item. Single-note output stays byte-identical by
    /// leaving both empty.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(super) notes: Vec<SubBulletNoteJson>,
    /// Every drawer a multi-note item filed into, in first-touch order.
    /// Empty for single-note items and for multi-note items that filed
    /// no drawer.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(super) drawers: Vec<DrawerJson>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) clip: Option<capture_clip::ClipOutput>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) schedule_log: Option<capture_schedule_log::ScheduleLog>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) block_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) day_file: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) block_link: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) pomodoro_link_placement: Option<Placement>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) parent_line: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) parent_text: Option<String>,
    pub(super) drawer: Option<DrawerJson>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) parent_status_symbol: Option<char>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) parent_status_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) toggle_direction: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) previous_task_line: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) status_symbol: Option<char>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) status_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) previous_status_symbol: Option<char>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) previous_status_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) pomodoro_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) creates_pomodoro: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) pomodoro_already_linked: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) removed_pomodoro_links: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) removed_scheduled: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) pomodoro_selector_unused: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) toggle_behavior: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) status_changed: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) pomodoro_link_action: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) pomodoro_link_source: Option<PomodoroLinkEndpoint>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) pomodoro_link_destination: Option<PomodoroLinkEndpoint>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) project_note: Option<ProjectNoteSummary>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) pomodoro_start: Option<PomodoroStartSummary>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) pomodoro_adjust: Option<PomodoroAdjustSummary>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) pomodoro_shift: Option<PomodoroShiftSummary>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) pomodoro_split: Option<PomodoroSplitSummary>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) pomodoro_merge: Option<PomodoroMergeSummary>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) pomodoro_close: Option<PomodoroCloseSummaryJson>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) pomodoro_past: Option<PomodoroPastSummaryJson>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) pomodoro_reset: Option<PomodoroResetSummaryJson>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) dependency_update: Option<DependencyUpdateJson>,
    /// Live Task Links consumed by this item while it left tasks Blocked.
    /// Additive and omitted when empty.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(super) consumed_links: Vec<ConsumedLinkJson>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) task_complete: Option<TaskCompleteSummaryJson>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) r#ref: Option<RefItemJson>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(super) gmail: Vec<crate::native::gmail::capture::Preview>,
    #[serde(skip)]
    pub(super) toggle_task_description: Option<String>,
    /// Link-guard report (§13.5, PDG vectors): present exactly when a
    /// gesture that would create a live Task Link found its target
    /// Blocked and created it unlinked instead. Additive: older
    /// clients ignore it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) link_guard: Option<LinkGuardJson>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(super) dropped_dependencies: Vec<DroppedDependencyJson>,
    /// BLOCKS-mirror rows staged by this item (`&` adds and dependency
    /// drops, `docs/task-dependencies.md` §14.11). Additive: omitted
    /// when empty, so Bob Mac Capture decoders never change.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(super) blocks_line_updates: Vec<BlocksLineUpdateJson>,
    /// The dependent `^block-id` minted for this item's BLOCKS entries,
    /// if any. Additive: omitted when no mint happens.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) minted_dependent_block_id: Option<MintedDependentBlockIdJson>,
    /// Every dependency update this item made: the item's own `&` update
    /// first, then one per `*` target in typed order. Present whenever
    /// at least one update exists, so `&`-only items carry it too. The
    /// singular `dependency_update` equals the first entry.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(super) dependency_updates: Vec<DependencyUpdateJson>,
    /// N's `^block-id` minted so a `*` write can name it. Never reuse
    /// `minted_dependent_block_id`, which is `&`'s BLOCKS mint for N as
    /// dependent. Additive: omitted when the ID was typed, not minted.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) minted_blocker_block_id: Option<MintedDependentBlockIdJson>,
    /// Per-item `~` / `~~` writes (`docs/task-relations.md` LX). Additive:
    /// omitted when empty.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(super) relation_updates: Vec<RelationUpdateJson>,
    /// Per-item cancel-label outcomes (`docs/task-relations.md` §17
    /// CL7): one JSON row per source this item cancelled. Already-closed
    /// sources stay in the vec for the muted human line and serialize
    /// away. Additive: omitted when nothing cancelled, so older output
    /// stays byte-identical.
    #[serde(
        skip_serializing_if = "relation_cancels_is_empty",
        serialize_with = "serialize_relation_cancels"
    )]
    pub(super) relation_cancels: Vec<RelationCancelJson>,
    /// Per-item complete-label outcomes (`docs/task-relations.md` §18
    /// CP7): one JSON row per source this item completed.
    /// Already-closed sources stay in the vec for the muted human line
    /// and serialize away. Additive: omitted when nothing completed, so
    /// older output stays byte-identical.
    #[serde(
        skip_serializing_if = "relation_completes_is_empty",
        serialize_with = "serialize_relation_completes"
    )]
    pub(super) relation_completes: Vec<RelationCompleteJson>,
    /// Relation-label merges and their hidden pull-in rows. The public
    /// `merges` key is flattened to preserve the capture item contract.
    #[serde(flatten)]
    pub(super) relation_merge_output: RelationMergeOutput,
    /// Bare-note `>` transfer moves and vault-wide repairs. Flattened
    /// and defaulted so ordinary items omit both keys.
    #[serde(flatten)]
    pub(super) transfer_move_output: super::transfer::TransferMoveOutput,
    /// Per-item `||` one-off hides (`docs/task-relations.md` RV6): one
    /// entry per distinct hidden label key this item typed, in typed
    /// order. Present on dry runs too; the journal append happens only
    /// on commit. Additive: omitted when empty.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(super) relation_label_hides: Vec<RelationLabelHideJson>,
    /// Courtesy LINKED FROM rows staged by this item. Same shape as
    /// `blocks_line_updates`. Additive: omitted when empty.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(super) linked_from_line_updates: Vec<BlocksLineUpdateJson>,
    /// Block ids minted so a relation edge can name the source or the
    /// `~~` target. Additive: omitted when empty.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(super) minted_relation_block_ids: Vec<MintedRelationBlockIdJson>,
    /// LINKED FROM courtesy writes this item skipped. Additive: omitted
    /// when empty.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(super) mirror_skipped: Vec<MirrorSkippedJson>,
    /// The capture list this bullet belongs to, when split from a lone
    /// `@route` / `@route#section` header. Additive: omitted otherwise.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) list: Option<ListContext>,
    /// Item-level relation successor offers for an `@route+id!` unlink of
    /// a Pending task (`docs/task-dependencies.md` §12.18 SA31). Omitted
    /// when empty so existing items stay byte-identical.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(super) relation_successors: Vec<TaskCompleteUnblockedJson>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(super) relation_held: Vec<RelationHeldJson>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) relation_check: Option<&'static str>,
    #[serde(skip)]
    pub(super) relation_groups:
        Vec<crate::native::task_complete::RelationOfferGroup>,
    /// Note-pull result for a lone `@route` item
    /// (`docs/task-dependencies.md` §12.20). Additive: omitted otherwise.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) note_pull: Option<NotePullSummaryJson>,
    /// Record-note summary used for human output and to collect
    /// top-level `note_files`. Additive per-item metadata so Mac Capture
    /// can render the committed success line without the full Markdown.
    /// Ordinary items omit it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) record_note: Option<RecordNoteSummary>,
    /// Notebook summary for `notebook_note` / `notebook_sub_note` items.
    /// Additive: omitted otherwise so older output stays byte-identical.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) notebook: Option<super::notebook_note::NotebookSummary>,
}

/// Album identity carried on album-backed record payloads.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct RecordAlbumJson {
    pub(super) stem: String,
    pub(super) title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) owner: Option<String>,
    pub(super) breadcrumb: Vec<String>,
}

/// One staged album records-index edit in top-level `album_files`.
///
/// The album is an ordinary additional file change, never another capture
/// item: `note_files` and item cards stay record-oriented. `before` and
/// `after` carry the exact planned bytes so dry-run and commit agree.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct AlbumFileJson {
    pub(super) path: String,
    pub(super) state: String,
    pub(super) before: String,
    pub(super) after: String,
}

/// One planned record file in top-level `note_files`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct NoteFileJson {
    pub(super) path: String,
    pub(super) action: &'static str,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub(super) record_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) source: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) album: Option<RecordAlbumJson>,
    pub(super) properties: Vec<NoteFilePropertyJson>,
    pub(super) body: String,
    pub(super) markdown: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(super) lines: Vec<BlockLineJson>,
}

/// One schema property on a planned record file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct NoteFilePropertyJson {
    pub(super) key: String,
    pub(super) label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) value: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) display: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) source: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) before: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) before_display: Option<String>,
}

/// Human-facing record create/update summary plus the `note_files` row.
///
/// The wire shape carries only committed metadata (`action`, `label`,
/// optional `emoji`, `path`, `notes`, `properties`); the full Markdown
/// stays in top-level `note_files[].markdown` and is never duplicated
/// here. Internal `markdown`/`note_file` members stay skipped.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct RecordNoteSummary {
    pub(super) action: &'static str,
    pub(super) label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) emoji: Option<String>,
    pub(super) path: String,
    pub(super) notes: usize,
    pub(super) properties: Vec<NoteFilePropertyJson>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) source: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) album: Option<RecordAlbumJson>,
    #[serde(skip)]
    pub(super) markdown: String,
    #[serde(skip)]
    pub(super) note_file: NoteFileJson,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub(super) struct RelationMergeOutput {
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(super) merges: Vec<crate::native::task_merge::MergeRecord>,
    #[serde(rename = "merge_offers", skip_serializing_if = "Vec::is_empty")]
    pub(super) offer_rows: Vec<TaskCompleteUnblockedJson>,
    #[serde(skip)]
    pub(super) merge_line_origins:
        Vec<crate::native::task_merge::MergeLineOrigin>,
}

/// Note-pull payload on a lone `@route` item
/// (`docs/task-dependencies.md` §12.20).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct NotePullSummaryJson {
    pub(super) note: String,
    pub(super) route: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(super) successors: Vec<TaskCompleteUnblockedJson>,
    pub(super) folded: PullInFoldedJson,
}

/// Per-item drawer report: the matched or created drawer title, its key,
/// and whether this item created it. `None` serializes as `null` for a
/// plain child.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct DrawerJson {
    pub(super) title: String,
    pub(super) key: String,
    pub(super) created: bool,
}

/// One note filed by a note-less `@route+id` multi-note item. Each
/// first-level bullet (and, for drawer bullets, each nested bullet under
/// the drawer header) becomes one entry in draft order, with the same
/// `task_line` and `sub_bullets` rendering as a single-note capture and
/// the drawer it landed in (or null for a plain note).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct SubBulletNoteJson {
    pub(super) text: String,
    pub(super) task_line: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(super) sub_bullets: Vec<String>,
    pub(super) drawer: Option<DrawerJson>,
}

impl CaptureItemResult {
    /// The array is present whenever any update exists, and the singular
    /// field equals `[0]`. Close/reset paths historically set only the
    /// singular field; fold them here so every JSON item holds the pair.
    pub(super) fn normalize_dependency_updates(&mut self) {
        if self.dependency_updates.is_empty()
            && let Some(update) = self.dependency_update.clone()
        {
            self.dependency_updates.push(update);
        }
    }
}

/// Why a link gesture left its target unlinked (§13.5 gesture rule):
/// the §13.9 reason kinds for machines, the promoted prerequisite
/// identities (`route#^block`) for machines, and one sentence for
/// humans (Mac Capture renders it).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct LinkGuardJson {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(super) reasons: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(super) promoted: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) detail: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) drop_refused: Option<&'static str>,
    #[serde(skip)]
    pub(super) removed_pomodoro_links: Option<usize>,
    #[serde(skip)]
    pub(super) day_file: Option<String>,
}

impl LinkGuardJson {
    pub(super) fn not_linked(
        reasons: &[BlockedReason],
        promoted: Vec<String>,
        drop_refused: Option<&'static str>,
    ) -> Self {
        let promoted_text = if promoted.is_empty() {
            String::new()
        } else {
            format!(" · promoted {}", promoted.join(", "))
        };
        let refusal = drop_refused
            .and_then(drop_refused_phrase)
            .map(|phrase| format!(" · dependencies kept: {phrase}"))
            .unwrap_or_default();
        Self {
            reasons: reasons
                .iter()
                .map(|reason| reason.kind.to_string())
                .collect(),
            detail: Some(format!(
                "not linked ({}){promoted_text}{refusal}",
                reasons
                    .iter()
                    .map(|reason| reason.label.as_str())
                    .collect::<Vec<_>>()
                    .join("; "),
            )),
            promoted,
            drop_refused,
            removed_pomodoro_links: None,
            day_file: None,
        }
    }
}

fn drop_refused_phrase(code: &'static str) -> Option<&'static str> {
    match code {
        "project_blocked" => Some("its project is blocked"),
        "project_task" => {
            Some("edit a project's prerequisites from its ^prj Task Card")
        }
        "schedule" => Some("several [scheduled::] fields still block it"),
        "dependency_line" => Some("repair its Depends-On line first"),
        _ => None,
    }
}

/// One prerequisite removed from a task's Depends-On line by capture.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct DroppedDependencyJson {
    pub(super) note_path: String,
    pub(super) block_id: String,
    pub(super) link: String,
    pub(super) text: String,
    pub(super) status_symbol: char,
    pub(super) status_name: String,
}

/// One `task_complete` result: what completing a whole-item `!` wrote.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct TaskCompleteSummaryJson {
    pub(super) raw: String,
    pub(super) note: String,
    pub(super) note_path: String,
    pub(super) block_id: String,
    pub(super) action: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) completion_date: Option<String>,
    /// Clean display text for the root task: the body after the status
    /// box with the configured global filter, inline fields, and the
    /// trailing block ID removed. Same string the human output prints.
    pub(super) text: String,
    pub(super) subtasks: Vec<TaskCompleteSubtaskJson>,
    pub(super) subtasks_left_open: Vec<TaskCompleteLeftOpenJson>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) ledger: Option<TaskCompleteLedgerJson>,
    pub(super) unblocked: Vec<TaskCompleteUnblockedJson>,
    pub(super) keep_moving: Vec<TaskCompleteUnblockedJson>,
    /// Relation-winning successor offers. Omitted when inapplicable.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(super) relation_successors: Vec<TaskCompleteUnblockedJson>,
    /// Relation-held sources that never become offers. Omitted when empty.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(super) relation_held: Vec<RelationHeldJson>,
    /// `"checked"` or `"unavailable"` when the relation lookup ran.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) relation_check: Option<&'static str>,
    /// Surviving relation groups, kept for the presentation phase.
    #[serde(skip)]
    pub(super) relation_groups:
        Vec<crate::native::task_complete::RelationOfferGroup>,
    pub(super) still_blocked: Vec<TaskCompleteStillBlockedJson>,
    pub(super) unblocked_check: &'static str,
    /// Project-group folded counts from this item's successor plan.
    /// Never serialized.
    #[serde(skip)]
    pub(super) project_folds: Vec<crate::native::task_complete::ProjectFold>,
    /// Keep-moving folded counts from this item's successor plan.
    /// Never serialized.
    #[serde(skip)]
    pub(super) keep_moving_folds:
        Vec<crate::native::task_complete::KeepMovingFold>,
}

/// One embedded subtask closed by a `task_complete` item.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct TaskCompleteSubtaskJson {
    pub(super) note_path: String,
    pub(super) block_id: String,
    pub(super) line: usize,
    pub(super) text: String,
    pub(super) previous_status_symbol: char,
    pub(super) previous_status_name: String,
    pub(super) status_symbol: char,
    pub(super) status_name: String,
}

/// One descendant left open by a `task_complete` item.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct TaskCompleteLeftOpenJson {
    pub(super) note_path: String,
    pub(super) block_id: String,
    pub(super) line: usize,
    pub(super) text: String,
    pub(super) status_symbol: char,
    pub(super) status_name: String,
    pub(super) reason: String,
}

/// Ledger retirement for a `task_complete` item, omitted when the daily
/// note was untouched.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct TaskCompleteLedgerJson {
    pub(super) day_file: String,
    pub(super) struck: usize,
    /// One entry per ledger entry with an in-place strike, always
    /// present (possibly empty) whenever `ledger` is present.
    pub(super) struck_in: Vec<TaskCompleteLedgerEndpointJson>,
    pub(super) moved: Vec<TaskCompleteLedgerMoveJson>,
    pub(super) deduplicated: usize,
    /// One entry per deduplicated bullet, always present (possibly
    /// empty) whenever `ledger` is present.
    pub(super) dropped: Vec<TaskCompleteLedgerDroppedJson>,
    pub(super) removed_placeholders: Vec<TaskCompleteRemovedPlaceholderJson>,
}

/// One moved bullet's source and destination entries.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct TaskCompleteLedgerMoveJson {
    pub(super) from: TaskCompleteLedgerEndpointJson,
    pub(super) to: TaskCompleteLedgerEndpointJson,
}

/// One deduplicated bullet's source and destination entries.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct TaskCompleteLedgerDroppedJson {
    pub(super) from: TaskCompleteLedgerEndpointJson,
    pub(super) to: TaskCompleteLedgerEndpointJson,
}

/// One ledger entry endpoint: 1-based line, short name, entry status.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct TaskCompleteLedgerEndpointJson {
    pub(super) line: usize,
    pub(super) name: String,
    pub(super) status: String,
}

/// One placeholder removed by a `task_complete` item.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct TaskCompleteRemovedPlaceholderJson {
    pub(super) name: String,
}

/// One relation target named on a successor or held row
/// (`docs/task-dependencies.md` §12.5).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct RelationTargetJson {
    pub(super) note_path: String,
    pub(super) block_id: String,
    pub(super) text: String,
}

/// Winning or second-reason relation metadata on an offered row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct RelationMetaJson {
    pub(super) label: String,
    pub(super) key: String,
    pub(super) event: &'static str,
    pub(super) after: Vec<RelationTargetJson>,
}

/// A relation source that never becomes an offer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct RelationHeldJson {
    pub(super) note_path: String,
    pub(super) block_id: String,
    pub(super) line: usize,
    pub(super) text: String,
    pub(super) status_symbol: char,
    pub(super) status_name: String,
    pub(super) reason: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) waits_on: Option<Vec<RelationTargetJson>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) scheduled: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) project: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) waits_for: Option<Vec<RelationTargetJson>>,
    pub(super) relation: RelationMetaJson,
}

/// One dependent unblocked by a `task_complete` item: linked into its
/// predecessor's slot, or recovered with the reason it was not linked
/// (`docs/task-dependencies.md` §12.5).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct TaskCompleteUnblockedJson {
    pub(super) note_path: String,
    pub(super) block_id: String,
    pub(super) line: usize,
    pub(super) text: String,
    pub(super) previous_status_symbol: char,
    pub(super) previous_status_name: String,
    pub(super) status_symbol: char,
    pub(super) status_name: String,
    pub(super) inbox: bool,
    pub(super) unblocked_by: Vec<TaskCompleteUnblockedCauseJson>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) link: Option<TaskCompleteSuccessorLinkJson>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) not_linked: Option<String>,
    pub(super) trigger: &'static str,
    pub(super) hide_removed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) offer: Option<TaskCompleteOfferJson>,
    /// Winning or second-reason relation metadata.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) relation: Option<RelationMetaJson>,
}

/// One predecessor behind an unblocked row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct TaskCompleteUnblockedCauseJson {
    pub(super) note_path: String,
    pub(super) block_id: String,
    pub(super) text: String,
}

/// Placement of one linked successor (`docs/task-dependencies.md`
/// §12.5): 1-based lines in the day text after the gesture.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct TaskCompleteSuccessorLinkJson {
    pub(super) day_file: String,
    pub(super) entry_name: String,
    pub(super) entry_line: usize,
    pub(super) entry_created: bool,
    pub(super) next_up: bool,
    pub(super) line: usize,
    pub(super) block_link: String,
    pub(super) block_id_created: bool,
}

/// Offer fields on an unblocked row (`docs/task-dependencies.md` §12.5).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct TaskCompleteOfferJson {
    pub(super) key: String,
    pub(super) n: usize,
    pub(super) group: &'static str,
    pub(super) selected: bool,
    /// Card verdict outcome (`done`/`cancelled`); present only on rows
    /// this gesture resolved instead of pulling in (§12.5, §12.22).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) verdict: Option<&'static str>,
    /// Human receipt line for a resolved row; never serialized. Human
    /// output prints it in place of the linked / not-pulled-in line.
    #[serde(skip)]
    pub(super) verdict_message: Option<String>,
    /// `↳` tickler lines for dependents a resolved row recovered;
    /// never serialized.
    #[serde(skip)]
    pub(super) verdict_ticklers: Vec<String>,
    pub(super) block_id_needed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) project: Option<String>,
    /// Group target for a relation offer (`note.md#^id`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) after: Option<String>,
    /// Source parent for a moved project offer (`cash.md`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) moved_from: Option<String>,
    /// Merge source for a hand-off offer (`old-note.md#^source`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) merged_from: Option<String>,
    /// Requested source note of a note-pull offer (`work.md`). Frozen at
    /// offer time like `key`: a later move updates the row's `note_path`
    /// to the task's final home, while this keeps naming the `@note` the
    /// row was pulled from, so `pull_in.groups[].note` membership survives.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) note: Option<String>,
    pub(super) destination: TaskCompleteOfferDestinationJson,
    /// Bob-authored per-row Done/Cancel eligibility and consequence copy.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) close:
        Option<crate::native::task_complete::offer_close::OfferCloseDescriptor>,
    /// Crate-private draft order; never a JSON field.
    #[serde(skip)]
    pub(super) order_key: crate::native::task_complete::OfferOrderKey,
    /// Crate-private identity of an ID-less project offer: its 1-based
    /// position among the project's equal-description tasks that are still
    /// ID-less once this capture's picks have minted their IDs. The public
    /// `key` ordinal counts the pre-pick render and so cannot address the
    /// shrunken ID-less population; never a JSON field.
    #[serde(skip)]
    pub(super) settled_ordinal: Option<usize>,
}

/// Destination reported on an offer row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct TaskCompleteOfferDestinationJson {
    pub(super) day_file: String,
    pub(super) entry_name: String,
    pub(super) entry_role: &'static str,
    pub(super) fallback: bool,
}

/// Top-level successor-offer summary, present on every success.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct PullInJson {
    pub(super) supported: bool,
    pub(super) offer_count: usize,
    pub(super) choice: Option<String>,
    pub(super) picked_count: usize,
    /// Offer-close capability and verdict counts. Present on every
    /// success from a bob that accepts `-x`/`-X` (§12.5, §12.14); older
    /// bobs omit it, which is the Mac's feature gate.
    pub(super) close: PullInCloseJson,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(super) groups: Vec<PullInGroupJson>,
    /// Bob-authored picker title, capsule, and glyph. Omitted when there
    /// are no groups (`docs/task-dependencies.md` §12.5).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) copy: Option<PullInTopCopyJson>,
}

/// Offer-close support for `pull_in.close` (`docs/task-dependencies.md`
/// §12.5): the feature gate plus how many rows each verdict resolved.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct PullInCloseJson {
    pub(super) supported: bool,
    pub(super) done_count: usize,
    pub(super) cancelled_count: usize,
}

impl PullInJson {
    pub(super) fn empty() -> Self {
        Self {
            supported: true,
            offer_count: 0,
            choice: None,
            picked_count: 0,
            close: PullInCloseJson {
                supported: true,
                done_count: 0,
                cancelled_count: 0,
            },
            groups: Vec::new(),
            copy: None,
        }
    }
}

/// One applied card verdict (`docs/task-dependencies.md` §12.5
/// `offer_closes[]`): the pre-write offer identity plus the exact joint
/// result, so a client can reopen the card with choices intact.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct OfferCloseJson {
    pub(super) n: usize,
    pub(super) key: String,
    pub(super) outcome: &'static str,
    /// An authored Cancel reason only; omitted for Done and blank Cancel.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) reason: Option<String>,
    pub(super) note_path: String,
    /// 1-based line of the resolved row in the staged text.
    pub(super) line: usize,
    /// Empty when the task has no block ID.
    pub(super) block_id: String,
    pub(super) text: String,
    pub(super) previous_status: char,
    pub(super) previous_status_name: String,
    /// Cancel Log write for Cancel rows: `written`, `created`,
    /// `fallback`, or `none`. Omitted for Done.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) cancel_log: Option<&'static str>,
    pub(super) recovered: Vec<OfferCloseRecoveredJson>,
    /// The canonical receipt row human output prints for this verdict.
    pub(super) message: String,
}

/// One dependent a card verdict recovered (`docs/task-dependencies.md`
/// §12.5 `offer_closes[].recovered[]`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct OfferCloseRecoveredJson {
    pub(super) note_path: String,
    pub(super) block_id: String,
    pub(super) text: String,
    pub(super) previous_status_name: String,
    pub(super) status_name: String,
    pub(super) unblocked_stamped: bool,
}

/// Top-level `pull_in.copy` (`docs/task-dependencies.md` §12.5).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct PullInTopCopyJson {
    pub(super) title: String,
    pub(super) capsule: String,
    pub(super) glyph: &'static str,
}

/// Per-group `copy: {title, why?, tag?, fold?, glyph}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct PullInGroupCopyJson {
    pub(super) title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) why: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) tag: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) fold: Option<String>,
    pub(super) glyph: &'static str,
}

/// Relation-group `after` identity (`docs/task-dependencies.md` §12.5).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct PullInAfterJson {
    pub(super) key: String,
    pub(super) note_path: String,
    pub(super) block_id: String,
    pub(super) text: String,
}

/// One offered group in `pull_in.groups`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct PullInGroupJson {
    pub(super) group: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) project: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) after: Option<PullInAfterJson>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) label: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) event: Option<&'static str>,
    /// Source note for a note-pull group. Omitted for every other group.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) note: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) folded: Option<PullInFoldedJson>,
    pub(super) copy: PullInGroupCopyJson,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct PullInFoldedJson {
    pub(super) hidden: usize,
    pub(super) already_planned: usize,
    pub(super) sub_projects: usize,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub(super) blocked: usize,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub(super) scheduled: usize,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub(super) already_started: usize,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub(super) waits_for: usize,
}

/// One open dependent of the completed tasks that stays blocked
/// (`docs/task-dependencies.md` §12.5 `still_blocked[]`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct TaskCompleteStillBlockedJson {
    pub(super) note_path: String,
    pub(super) block_id: String,
    pub(super) line: usize,
    pub(super) text: String,
    pub(super) status_symbol: char,
    pub(super) status_name: String,
    pub(super) reason: &'static str,
    pub(super) waits_on: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) scheduled: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) project: Option<String>,
    pub(super) trigger: &'static str,
}

/// Whether this update is the item's own `&` write or a `*` target.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum DependencyUpdateRole {
    Dependent,
    /// One `*` target: T is the dependent, N is the prerequisite.
    BlockedTask,
}

/// Summary JSON for one item's dependency effects, reported as
/// `dependency_update` on the capture result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct DependencyUpdateJson {
    pub(super) role: DependencyUpdateRole,
    pub(super) dependent_note: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) dependent_block_id: Option<String>,
    pub(super) dependent_text: String,
    pub(super) new_task: bool,
    pub(super) added: usize,
    pub(super) already_present: usize,
    pub(super) open_prerequisites: usize,
    pub(super) prerequisites: Vec<PrerequisiteJson>,
    pub(super) dependent_status: char,
    pub(super) dependent_status_name: String,
    pub(super) status_changed: bool,
    pub(super) previous_status: char,
    pub(super) previous_status_name: String,
    /// `^prj` residents this write newly blocked. Omitted when 0.
    #[serde(skip_serializing_if = "is_zero")]
    pub(super) blocked_residents: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct ConsumedLinkJson {
    pub(super) day_file: String,
    pub(super) line: usize,
    pub(super) link: String,
    pub(super) note: String,
    pub(super) block_id: String,
    pub(super) text: String,
    pub(super) action: ConsumedLinkAction,
    pub(super) pomodoro: ConsumedLinkPomodoroJson,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum ConsumedLinkAction {
    Removed,
    TokenRemoved,
    NotesKept,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct ConsumedLinkPomodoroJson {
    pub(super) line: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) time_range: Option<String>,
    pub(super) status: ConsumedPomodoroStatus,
    pub(super) removed: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum ConsumedPomodoroStatus {
    Running,
    Queued,
}

/// One prerequisite behind a `dependency_update` summary: its note,
/// block ID, canonical link, and status with text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct PrerequisiteJson {
    pub(super) note: String,
    pub(super) block_id: String,
    pub(super) link: String,
    pub(super) status_symbol: char,
    pub(super) status_name: String,
    pub(super) text: String,
    pub(super) open: bool,
    /// True when this prerequisite was already on the Depends-On line.
    /// Omitted when false so older decoders stay valid.
    #[serde(skip_serializing_if = "is_false")]
    pub(super) already_present: bool,
    /// Pre-write status, present only when this write changed it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) previous_status: Option<char>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) previous_status_name: Option<String>,
}

/// One BLOCKS-line projection write (`docs/task-dependencies.md` §14.11,
/// same shape as reconcile's `blocks_line_updates` rows). `kind` is one
/// of `inserted`, `rewritten`, `canonicalized`, `removed`, or
/// `deduplicated`. Additive: omitted from capture JSON when empty, so
/// Bob Mac Capture decoders never change.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct BlocksLineUpdateJson {
    pub(super) path: String,
    pub(super) line: usize,
    pub(super) task: String,
    pub(super) kind: String,
    pub(super) added: Vec<String>,
    pub(super) removed: Vec<String>,
    /// Mention subset of `added`. Omitted when empty so BLOCKS and
    /// relation rows stay byte-identical for older decoders.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(super) mentions_added: Vec<String>,
    /// Mention subset of `removed`. Omitted when empty.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(super) mentions_removed: Vec<String>,
}

/// One dependent `^block-id` minted so a BLOCKS entry can name it
/// (`docs/task-dependencies.md` §14.4), reported as the singular
/// `minted_dependent_block_id` on the owning capture item. Additive:
/// omitted when no mint happens.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct MintedDependentBlockIdJson {
    pub(super) path: String,
    pub(super) line: usize,
    pub(super) block_id: String,
}

/// One `~` / `~~` edge this item wrote or confirmed
/// (`docs/task-relations.md` Rule 12).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct RelationUpdateJson {
    pub(super) direction: RelationDirection,
    pub(super) source: RelationEndpointJson,
    pub(super) target: RelationEndpointJson,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) label: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) previous_label: Option<String>,
    pub(super) action: RelationAction,
    /// Cancel-label outcome for an Added/Relabeled edge whose label is
    /// configured (`docs/task-relations.md` §17 CL7). Omitted for
    /// unchanged edges and labels that cancel nothing.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) cancel_effect: Option<RelationCancelEffect>,
    /// Complete-label outcome for an Added/Relabeled edge whose label is
    /// configured (`docs/task-relations.md` §18 CP7). Omitted for
    /// unchanged edges and labels that complete nothing.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) complete_effect: Option<RelationCompleteEffect>,
    /// Merge-label outcome for an Added/Relabeled edge.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) merge_effect: Option<RelationMergeEffect>,
    /// Successor-label consequence at authoring time. Omitted when the
    /// label is not configured or the target state could not be resolved.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) offer_effect: Option<RelationOfferEffect>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum RelationOfferEffect {
    Offers,
    AlreadyParked,
    AlreadyClosed,
}

/// What a cancel-label edge did to its source
/// (`docs/task-relations.md` §17 CL5): the link was written and the
/// source cancelled, or the link was written and the already-closed
/// source was left alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum RelationCancelEffect {
    Cancelled,
    AlreadyClosed,
}

/// What a complete-label edge did to its source
/// (`docs/task-relations.md` §18 CP7): the link was written and the
/// source completed, or the link was written and the already-closed
/// source was left alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum RelationCompleteEffect {
    Completed,
    AlreadyClosed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum RelationMergeEffect {
    Merged,
    AlreadyClosed,
}

/// One cancel-label source this item cancelled
/// (`docs/task-relations.md` §17 CL7): the staged source cancel, one
/// row per source no matter how many cancel edges named it.
/// Already-closed sources keep `effect: AlreadyClosed` for the human
/// line and are omitted from JSON.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct RelationCancelJson {
    #[serde(skip)]
    pub(super) effect: RelationCancelEffect,
    pub(super) source: RelationCancelSourceJson,
    pub(super) target: RelationCancelTargetJson,
    pub(super) labels: Vec<String>,
    pub(super) action: RelationAction,
    pub(super) previous_status: char,
    pub(super) previous_status_name: String,
    /// Open tasks that stay open when the cancelled source is a `^prj`.
    /// Omitted for non-project sources.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) project_open_tasks: Option<usize>,
    pub(super) cancel_log: &'static str,
    pub(super) pomodoro_links_removed: usize,
    pub(super) recovered: Vec<RelationCancelRecoveredJson>,
    pub(super) message: String,
}

fn relation_cancels_is_empty(cancels: &[RelationCancelJson]) -> bool {
    cancels
        .iter()
        .all(|cancel| cancel.effect != RelationCancelEffect::Cancelled)
}

fn serialize_relation_cancels<S>(
    cancels: &[RelationCancelJson],
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    cancels
        .iter()
        .filter(|cancel| cancel.effect == RelationCancelEffect::Cancelled)
        .collect::<Vec<_>>()
        .serialize(serializer)
}

fn relation_completes_is_empty(completes: &[RelationCompleteJson]) -> bool {
    completes
        .iter()
        .all(|complete| complete.effect != RelationCompleteEffect::Completed)
}

fn serialize_relation_completes<S>(
    completes: &[RelationCompleteJson],
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    completes
        .iter()
        .filter(|complete| complete.effect == RelationCompleteEffect::Completed)
        .collect::<Vec<_>>()
        .serialize(serializer)
}

/// The cancelled source of one `relation_cancels[]` row: where S sits
/// after this item, with its pre-cancel description.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct RelationCancelSourceJson {
    pub(super) path: String,
    pub(super) line: usize,
    pub(super) block_id: String,
    pub(super) text: String,
}

/// The surviving target of one `relation_cancels[]` row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct RelationCancelTargetJson {
    pub(super) path: String,
    pub(super) block_id: String,
    pub(super) text: String,
}

/// One dependent this cancel recovered: the post-recovery location and
/// clean text, in vault order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct RelationCancelRecoveredJson {
    pub(super) path: String,
    pub(super) line: usize,
    pub(super) block_id: String,
    pub(super) text: String,
}

/// One complete-label source this item completed
/// (`docs/task-relations.md` §18 CP7): the staged source completion, one
/// row per source no matter how many complete edges named it.
/// Already-closed sources keep `effect: AlreadyClosed` for the human
/// line and are omitted from JSON.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct RelationCompleteJson {
    #[serde(skip)]
    pub(super) effect: RelationCompleteEffect,
    pub(super) source: RelationCompleteSourceJson,
    pub(super) target: RelationCompleteTargetJson,
    pub(super) labels: Vec<String>,
    pub(super) action: RelationAction,
    pub(super) previous_status: char,
    pub(super) previous_status_name: String,
    /// Open tasks that stay open when the completed source is a `^prj`.
    /// Omitted for non-project sources.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) project_open_tasks: Option<usize>,
    pub(super) task_complete: TaskCompleteSummaryJson,
    pub(super) message: String,
}

/// The completed source of one `relation_completes[]` row: where S sits
/// after this item, with its pre-complete description.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct RelationCompleteSourceJson {
    pub(super) path: String,
    pub(super) line: usize,
    pub(super) block_id: String,
    pub(super) text: String,
}

/// The surviving target of one `relation_completes[]` row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct RelationCompleteTargetJson {
    pub(super) path: String,
    pub(super) block_id: String,
    pub(super) text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct RelationEndpointJson {
    pub(super) path: String,
    pub(super) line: usize,
    pub(super) block_id: String,
    pub(super) text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) archived: Option<bool>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum RelationAction {
    Added,
    Relabeled,
    Unchanged,
}

/// One `||` one-off label hide this item requested
/// (`docs/task-relations.md` RV6): the label as typed (display spelling
/// when the key already exists), its catalog key, the pre-capture link
/// count, whether it was already hidden, and what the commit does.
/// Counts come from the pre-capture catalog, whatever later items in
/// the batch also hide.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct RelationLabelHideJson {
    pub(super) label: String,
    pub(super) key: String,
    pub(super) existing_count: u64,
    pub(super) was_hidden: bool,
    pub(super) action: RelationLabelHideAction,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum RelationLabelHideAction {
    Hide,
    AlreadyHidden,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum MintedRelationRole {
    Source,
    Target,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct MintedRelationBlockIdJson {
    pub(super) path: String,
    pub(super) line: usize,
    pub(super) block_id: String,
    pub(super) role: MintedRelationRole,
    /// `"mention"` when capture minted this id so a text-mention mirror
    /// can name S. Omitted for relation mints so existing JSON stays
    /// byte-identical. A source minted for both reports as a relation
    /// mint (no reason).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) reason: Option<&'static str>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct MirrorSkippedJson {
    pub(super) path: String,
    pub(super) block_id: String,
    pub(super) reason: &'static str,
}

/// BLOCKS-mirror side effects staged inside one capture batch item:
/// the projection rows, the dependent mint (at most one dependent per
/// item), and the touched owners reported as `dependency_target` task
/// blocks. Never serialized directly; its pieces ride on
/// [`CaptureItemResult`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct BlocksMirrorReports {
    pub(super) updates: Vec<BlocksLineUpdateJson>,
    pub(super) minted: Option<MintedDependentBlockIdJson>,
    pub(super) target_refs: Vec<TaskBlockRef>,
}

pub(super) fn print_success(
    result: &CaptureResult,
    output_format: OutputFormat,
) {
    match output_format {
        OutputFormat::Human => {
            print_capture_warnings(result);
            print_human_success(result);
            print_plan_budget(result);
            let items = if result.captures.is_empty() {
                std::slice::from_ref(&result.item)
            } else {
                &result.captures
            };
            for item in items {
                for gmail in &item.gmail {
                    println!("  {}", gmail.message);
                }
            }
        }
        OutputFormat::Json => println!("{}", success_json(result)),
    }
}

pub(super) fn print_capture_warnings(result: &CaptureResult) {
    if result.warnings.is_empty() {
        return;
    }
    let styler = Styler::detect();
    let has_structured_consumption = if result.captures.is_empty() {
        !result.item.consumed_links.is_empty()
    } else {
        result
            .captures
            .iter()
            .any(|item| !item.consumed_links.is_empty())
    };
    for warning in &result.warnings {
        // The cancel-label ⊘ row already names the cancel in stdout; the
        // matching structured warning stays JSON-only (§17 CL7).
        if let CaptureWarning::Structured(warning) = warning
            && (warning.code == "relation_cancelled_source"
                || warning.code == "relation_completed_source")
        {
            continue;
        }
        if has_structured_consumption
            && warning
                .as_text()
                .is_some_and(|text| text.starts_with("removed "))
            && warning
                .message()
                .ends_with(" live Pomodoro link(s) to newly Blocked task(s)")
        {
            continue;
        }
        eprintln!(
            "{COMMAND_NAME}: {}: {}",
            styler.warning_prefix(),
            warning.message()
        );
    }
}

fn print_consumed_links(result: &CaptureItemResult, styler: &Styler) {
    for consumed in &result.consumed_links {
        let pomodoro = &consumed.pomodoro;
        let name = pomodoro
            .name
            .as_deref()
            .map(str::to_string)
            .unwrap_or_else(|| format!("line {}", pomodoro.line));
        let range = pomodoro
            .time_range
            .as_deref()
            .map(format_consumed_time_range)
            .map(|range| format!(" {range}"))
            .unwrap_or_default();
        let running = if pomodoro.status == ConsumedPomodoroStatus::Running {
            " (running)"
        } else {
            " (queued)"
        };
        let action = match consumed.action {
            ConsumedLinkAction::NotesKept => {
                let verb = if result.dry_run { "would keep" } else { "kept" };
                println!(
                    "  {} {} its notes under {}{} as plain text",
                    styler.dim("−"),
                    verb,
                    name,
                    range,
                );
                continue;
            }
            ConsumedLinkAction::Removed | ConsumedLinkAction::TokenRemoved => {
                if result.dry_run {
                    "would remove"
                } else {
                    "removed"
                }
            }
        };
        let deletion = if pomodoro.removed {
            if result.dry_run {
                " · would delete the empty Pomodoro"
            } else {
                " · deleted the empty Pomodoro"
            }
        } else {
            ""
        };
        println!(
            "  {} {} its Task Link from {}{}{}{}",
            styler.dim("−"),
            action,
            name,
            range,
            running,
            deletion,
        );
    }
}

fn format_consumed_time_range(range: &str) -> String {
    let Some((start, end)) = range.split_once('-') else {
        return range.to_string();
    };
    let format = |time: &str| {
        if time.len() == 4 && time.bytes().all(|byte| byte.is_ascii_digit()) {
            format!("{}:{}", &time[..2], &time[2..])
        } else {
            time.to_string()
        }
    };
    format!("{}–{}", format(start), format(end))
}

/// Plan-budget meter line (stdout) plus one stderr warning per fired
/// cap. Budget warnings stay out of the `warnings` vector: they
/// describe the ledger, not the captured item.
pub(super) fn print_plan_budget(result: &CaptureResult) {
    let Some(budget) = result.plan_budget.as_ref() else {
        return;
    };
    let styler = Styler::detect();
    let links = format!("{}/{} links", budget.links.count, budget.links.cap);
    let links = if budget.links.over {
        styler.red(&links)
    } else {
        styler.green(&links)
    };
    println!("plan {links}");
    for warning in &budget.warnings {
        eprintln!(
            "{COMMAND_NAME}: {}: {}",
            styler.warning_prefix(),
            warning.message
        );
    }
}

/// Additive D7 suffix for a destination Bob placed by type: the cascade
/// label from the `type` object (` · from dev`, ` · default type`).
/// Legacy and named destinations render unchanged.
pub(super) fn format_placed_type_suffix(
    destination: &PomodoroLinkEndpoint,
) -> String {
    destination
        .placed_type
        .as_ref()
        .map(|placed| format!(" · {}", placed.label))
        .unwrap_or_default()
}

/// Additive R2 suffix for a destination that gained types on landing:
/// ` · merged FIX` (comma-joined when several types landed at once).
pub(super) fn format_merged_suffix(
    destination: &PomodoroLinkEndpoint,
) -> String {
    destination
        .merged
        .as_ref()
        .map_or_else(String::new, |merged| {
            let names = merged
                .added
                .iter()
                .map(|theme| theme.name.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            if names.is_empty() {
                String::new()
            } else {
                format!(" · merged {names}")
            }
        })
}

/// Destination arrow for `<text> @route:id[#NAME]` Pomodoro-task
/// captures: where the new Task Link landed.
pub(super) fn format_pomodoro_task_destination(
    destination: &PomodoroLinkEndpoint,
) -> String {
    let target = destination
        .name
        .as_deref()
        .map(str::to_string)
        .unwrap_or_else(|| format_pomodoro_endpoint(destination));
    let suffix = format!(
        "{}{}",
        format_placed_type_suffix(destination),
        format_merged_suffix(destination)
    );
    match destination.role {
        Some("created") => format!("→ new Pomodoro {target}{suffix}"),
        Some("current") => match destination.time_range.as_deref() {
            Some(range) => format!("→ into running {target} ({range}){suffix}"),
            None => format!("→ into running {target}{suffix}"),
        },
        Some("named") => format!("→ under {target} (named){suffix}"),
        _ => format!("→ under {target} (next up){suffix}"),
    }
}

fn format_styled_pomodoro_task_destination(
    destination: &PomodoroLinkEndpoint,
    theme_refs: &BTreeMap<String, Vec<super::super::pomodoro_themes::ThemeRef>>,
    styler: &Styler,
) -> String {
    let plain = format_pomodoro_task_destination(destination);
    let Some(name) = destination.name.as_deref() else {
        return plain;
    };
    let key = plan_budget::normalize_component(name);
    let Some(rendered) = theme_refs.get(&key).and_then(|refs| {
        super::super::pomodoro_themes::swatch_name(refs, " + ", styler)
    }) else {
        return plain;
    };
    plain.replacen(name, &rendered, 1)
}

fn past_trim_remaining(after: &str) -> u64 {
    let Some((s, e)) = after.split_once('-') else {
        return 0;
    };
    let parse = |v: &str| -> Option<i64> {
        if v.len() != 4 || !v.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        let h: i64 = v[..2].parse().ok()?;
        let m: i64 = v[2..].parse().ok()?;
        (h < 24 && m < 60).then_some(h * 60 + m)
    };
    let (Some(s), Some(e)) = (parse(s), parse(e)) else {
        return 0;
    };
    (e - s).rem_euclid(1440) as u64
}

fn print_human_past_trims(trims: &[PomodoroPastTrimJson]) {
    for trim in trims {
        let remaining = past_trim_remaining(&trim.after);
        let calendar_suffix = if trim.calendar { " (calendar)" } else { "" };
        println!(
            "⚠ took {}m from {} · {} → {} ({}m){} · line {}",
            trim.minutes,
            trim.name,
            trim.before,
            trim.after,
            remaining,
            calendar_suffix,
            trim.line,
        );
    }
}

/// Dim human-only tip for an adjust + unsized whole-item close pair that
/// could have been one sized close. Returns the tip line without styling.
fn sized_close_tip(
    adjust_item: &CaptureItemResult,
    close_item: &CaptureItemResult,
) -> Option<String> {
    let adjust = adjust_item.pomodoro_adjust.as_ref()?;
    if adjust_item.pomodoro_shift.is_some() {
        return None;
    }
    let close = close_item.pomodoro_close.as_ref()?;
    if close.size.is_some() {
        return None;
    }
    if close_item.placement != Placement::Closed {
        return None;
    }
    if close.decremented_minutes != 0 {
        return None;
    }
    let duration = close.closed.duration_minutes;
    if duration == 0 || duration % 5 != 0 {
        return None;
    }
    let blocks = duration / 5;
    if !(1..=99).contains(&blocks) {
        return None;
    }
    // Same daily-note row: the adjust target and the close entry share
    // the pre-close line and day file. Comparing refreshed post-close
    // lines or names alone can fail when logs move the section.
    if adjust.pomodoro_line != close.pomodoro_line {
        return None;
    }
    if adjust_item.relative_target != close.day_relative {
        return None;
    }
    let suffix = close.raw.strip_prefix('x').unwrap_or(&close.raw);
    let token = format!("{blocks}x{suffix}");
    Some(format!("tip: {token} closes at {duration}m in one step"))
}

pub(super) fn print_human_success(result: &CaptureResult) {
    if let Some(global) = &result.global_destination {
        print_global_destination_summary(global);
    }
    if let Some(summary) = &result.template {
        print_template_summary(
            summary,
            &result.template_rows,
            result.template_saved.as_ref(),
            result.template_not_saved.as_deref(),
            result.template_origin.as_deref(),
        );
    }
    if result.captures.is_empty() {
        print_list_summary(&result.item);
        print_human_item_success(result, None, &result.theme_refs);
        print_human_merge_block_unless_task_merge(&result.item);
        if let Some(past) = result.item.pomodoro_past.as_ref() {
            print_human_past_trims(&past.trims);
        }
        let rows = note_pull_rows(result);
        print_human_note_pull_groups(
            &result.pull_in.groups,
            &rows,
            result.dry_run,
            &Styler::detect(),
        );
        print_pull_in_hint(result);
        print_inbox_route_hint(result);
        print_move_choice_report(result);
        print_type_agreement(result);
        print_album_files(result);
        return;
    }

    let total = result.captures.len();
    for (index, item) in result.captures.iter().enumerate() {
        if index > 0 {
            println!();
        }
        print_fanout_summary(&result.captures, item);
        print_list_summary(item);
        print_human_item_success(
            item,
            Some((index + 1, total)),
            &result.theme_refs,
        );
        print_human_merge_block_unless_task_merge(item);
        if index > 0
            && let Some(tip) =
                sized_close_tip(&result.captures[index - 1], item)
        {
            println!("  {}", Styler::detect().dim(&tip));
        }
    }
    let past_trims: Vec<PomodoroPastTrimJson> = result
        .captures
        .iter()
        .filter_map(|item| item.pomodoro_past.as_ref())
        .flat_map(|past| past.trims.clone())
        .collect();
    print_human_past_trims(&past_trims);
    let rows = note_pull_rows(result);
    print_human_note_pull_groups(
        &result.pull_in.groups,
        &rows,
        result.dry_run,
        &Styler::detect(),
    );
    print_pull_in_hint(result);
    print_inbox_route_hint(result);
    print_move_choice_report(result);
    print_type_agreement(result);
    print_album_files(result);
}

fn print_type_agreement(result: &CaptureResult) {
    if result.type_agreement.is_empty() {
        return;
    }
    let verb = if result.dry_run {
        "would rename"
    } else {
        "renamed"
    };
    for row in &result.type_agreement {
        println!(
            "  ≈ {} — {} → — {} ({})",
            verb, row.before, row.after, row.reason
        );
    }
}

fn print_move_choice_report(result: &CaptureResult) {
    let Some(slots) = result.move_choice.slots.as_ref() else {
        return;
    };
    let styler = Styler::detect();
    for slot in slots {
        if slot.answered {
            continue;
        }
        println!(
            "- ^^ on line {} asks which of {}'s {} open tasks move in:",
            slot.item_line, slot.parent, slot.open_count
        );
        for row in &slot.rows {
            let id_part = if let Some(id) = row.block_id.as_deref() {
                format!(" ^{id}")
            } else if let Some(mint) = row.mint.as_deref() {
                format!(" adds ^{mint}")
            } else {
                String::new()
            };
            let tail = if let Some(lock) = row.locked.as_ref() {
                format!("moving on line {}", lock.item_line)
            } else if let Some(reason) = row.disabled_reason.as_deref() {
                reason.to_string()
            } else {
                // `^` consequence suffix after the head, without
                // duplicating the `adds ^mint` already shown as the ID.
                let head = format!("Moves into {}", slot.destination);
                row.choices
                    .iter()
                    .find(|c| c.sigil == "^")
                    .map(|choice| {
                        let suffix = choice
                            .consequence
                            .strip_prefix(&head)
                            .unwrap_or(choice.consequence.as_str())
                            .strip_prefix(" · ")
                            .unwrap_or(
                                choice
                                    .consequence
                                    .strip_prefix(&head)
                                    .unwrap_or("")
                                    .trim(),
                            )
                            .to_string();
                        if let Some(mint) = row.mint.as_deref() {
                            let adds = format!("adds ^{mint}");
                            if suffix == adds {
                                return String::new();
                            }
                            // Strip a trailing ` · adds ^mint` when other
                            // suffixes precede it; the ID line already shows it.
                            if let Some(stripped) =
                                suffix.strip_suffix(&format!(" · {adds}"))
                            {
                                return stripped.to_string();
                            }
                            if let Some(stripped) =
                                suffix.strip_prefix(&format!("{adds} · "))
                            {
                                return stripped.to_string();
                            }
                        }
                        suffix
                    })
                    .filter(|suffix| !suffix.is_empty())
                    .map(|suffix| format!("· {suffix}"))
                    .unwrap_or_default()
            };
            let tail = if tail.is_empty() {
                String::new()
            } else {
                format!(" {tail}")
            };
            println!(
                "  {}",
                styler.dim(&format!(
                    "{} [{}] {}{}",
                    row.n, row.status_symbol, row.text, id_part
                )) + &styler.dim(&tail)
            );
        }
        println!("  {}", styler.dim(super::move_choice::choose_hint()));
    }
}

/// Print the complete target run once, immediately before its first
/// per-target result.
fn print_fanout_summary(items: &[CaptureItemResult], item: &CaptureItemResult) {
    let Some(fanout) = item.fanout.as_ref() else {
        return;
    };
    if fanout.position != 1 {
        return;
    }
    let targets = items
        .iter()
        .filter_map(|candidate| candidate.fanout.as_ref())
        .filter(|candidate| {
            candidate.line == fanout.line && candidate.count == fanout.count
        })
        .collect::<Vec<_>>();
    if targets.len() != fanout.count {
        return;
    }
    let mut markers = Vec::new();
    for target in targets {
        if !markers.contains(&target.marker.as_str()) {
            markers.push(target.marker.as_str());
        }
    }
    println!(
        "{}",
        Styler::detect().dim(&format!("targets  {}", markers.join(" · ")))
    );
}

/// Requested source note of a note-pull row: the frozen `offer.note`,
/// falling back to `note_path` for payloads that predate the field. A later
/// move changes `note_path` to the task's final home but never the source.
pub(super) fn note_pull_source(row: &TaskCompleteUnblockedJson) -> &str {
    row.offer
        .as_ref()
        .and_then(|offer| offer.note.as_deref())
        .unwrap_or(&row.note_path)
}

fn note_pull_rows(result: &CaptureResult) -> Vec<&TaskCompleteUnblockedJson> {
    let rows = if result.captures.is_empty() {
        all_note_pull(&result.item).collect::<Vec<_>>()
    } else {
        result.captures.iter().flat_map(all_note_pull).collect()
    };
    let mut rows: Vec<_> = rows
        .into_iter()
        .filter(|row| {
            row.offer
                .as_ref()
                .is_some_and(|offer| offer.group == "note_pull")
        })
        .collect();
    rows.sort_by_key(|row| {
        row.offer.as_ref().map(|offer| offer.n).unwrap_or(0)
    });
    rows
}

fn print_human_note_pull_groups(
    groups: &[PullInGroupJson],
    rows: &[&TaskCompleteUnblockedJson],
    dry_run: bool,
    styler: &Styler,
) {
    for group in groups.iter().filter(|group| group.group == "note_pull") {
        let Some(note) = group.note.as_deref() else {
            continue;
        };
        let group_rows: Vec<_> = rows
            .iter()
            .copied()
            .filter(|row| note_pull_source(row) == note)
            .collect();
        if group_rows.is_empty() {
            continue;
        }
        let stem = Path::new(note)
            .file_stem()
            .map(|stem| stem.to_string_lossy().into_owned())
            .unwrap_or_else(|| note.to_string());
        let offer_count = group_rows.len();
        let picked_count = group_rows
            .iter()
            .filter(|row| {
                row.offer.as_ref().is_some_and(|offer| offer.selected)
            })
            .count();
        if picked_count == 0 {
            let noun = if offer_count == 1 { "task" } else { "tasks" };
            println!(
                "{} · {offer_count} ready {noun}",
                styler.yellow(&format!("Pull from {stem}"))
            );
        } else {
            println!(
                "{} · {picked_count} of {offer_count} pulled in",
                styler.yellow(&format!("Pull from {stem}"))
            );
        }
        for row in group_rows {
            print_human_note_pull_row(row, picked_count > 0, dry_run, styler);
        }
        if let Some(fold) = group.copy.fold.as_deref() {
            println!("  {}", styler.dim(&format!("not offered  {fold}")));
        }
    }
}

fn print_human_note_pull_row(
    row: &TaskCompleteUnblockedJson,
    partial_pick: bool,
    dry_run: bool,
    styler: &Styler,
) {
    let Some(offer) = row.offer.as_ref() else {
        return;
    };
    if let Some(message) = offer.verdict_message.as_deref() {
        println!("  offer {}  {message}", offer.n);
        for tickler in &offer.verdict_ticklers {
            println!("  {tickler}");
        }
        return;
    }
    let text = if row.text.is_empty() {
        String::new()
    } else {
        format!(" {}", row.text)
    };
    let locator = task_complete_locator(&row.note_path, &row.block_id);
    if let Some(link) = row.link.as_ref() {
        let verb = if dry_run { "would link" } else { "linked" };
        let current = styled_note_pull_lane(row.status_symbol, styler);
        let lane = if row.previous_status_symbol == row.status_symbol {
            current
        } else {
            format!(
                "{} → {current}",
                styled_note_pull_lane(row.previous_status_symbol, styler)
            )
        };
        let minted = link
            .block_link
            .rsplit_once('^')
            .filter(|_| link.block_id_created)
            .map(|(_, id)| format!(" · added ^{}", id.trim_end_matches("]]")))
            .unwrap_or_default();
        println!(
            "  offer {}  {verb} {lane}{text}  {locator} → {}{minted}",
            offer.n,
            offer_destination_label(&offer.destination),
        );
    } else {
        let lane = styled_note_pull_lane(row.status_symbol, styler);
        let prefix = if partial_pick {
            format!("  not pulled in  offer {}  ", offer.n)
        } else {
            format!("  offer {}  ", offer.n)
        };
        println!(
            "{prefix}{lane}{text}  {locator} → {}",
            offer_destination_label(&offer.destination),
        );
    }
}

fn styled_note_pull_lane(symbol: char, styler: &Styler) -> String {
    match symbol {
        '/' => styler.blue("Pending"),
        '*' => styler.yellow("Next"),
        _ => styler.dim("Ready"),
    }
}

/// The one summary line that precedes each list's first item:
/// `list  dev.md · 3 tasks` or `list  dev.md#ideas · 2 notes`.
fn print_list_summary(item: &CaptureItemResult) {
    let Some(list) = item.list.as_ref() else {
        return;
    };
    if list.position != 1 {
        return;
    }
    let mut target = item.relative_target.clone();
    if let Some(section) = list.section.as_deref() {
        target.push('#');
        target.push_str(section);
    }
    let noun = if item.notebook.is_some() {
        if list.count == 1 {
            "note"
        } else {
            "notes"
        }
    } else {
        match (list.kind, list.count) {
            (ListKind::Tasks, 1) => "task",
            (ListKind::Notes, 1) => "note",
            (kind, _) => kind.label(),
        }
    };
    println!("list  {target} · {} {noun}", list.count);
}

fn print_inbox_route_hint(result: &CaptureResult) {
    if result.inbox_route.candidates.is_empty() {
        return;
    }
    let styler = Styler::detect();
    let mut pending = false;
    for candidate in &result.inbox_route.candidates {
        match candidate.outcome {
            "moved" => print_routed_row(&styler, candidate, result.dry_run),
            "kept" => println!(
                "  {}  {} · still in {}",
                styler.yellow("kept"),
                candidate.text,
                candidate.inbox
            ),
            _ => {
                pending = true;
                let block = if candidate.block_id.is_empty() {
                    String::new()
                } else {
                    format!(" ^{}", candidate.block_id)
                };
                let route_verb = if result.dry_run {
                    styler.yellow("would route")
                } else {
                    styler.yellow("inbox")
                };
                println!(
                    "  {route_verb}  [{}] → [{}] {}  {}{} · still in {}",
                    candidate.previous_status_symbol,
                    candidate.status_symbol,
                    candidate.text,
                    candidate.note_path,
                    block,
                    candidate.inbox
                );
            }
        }
    }
    if pending {
        println!("  route with -i 1=<note> · -i keep");
    }
    if result.inbox_route.needs_reconcile == Some(true) {
        println!(
            "  {}",
            styler.dim("run 'bob task reconcile' to reconcile Blocked state")
        );
    }
}

fn print_routed_row(
    styler: &Styler,
    candidate: &InboxRouteCandidate,
    dry_run: bool,
) {
    let moved = candidate.moved.as_ref();
    let destination = moved
        .and_then(|moved| moved.get("destination"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default();
    let route = destination
        .strip_suffix(".md")
        .unwrap_or(destination)
        .to_string();
    let verb = if dry_run {
        styler.yellow("would route")
    } else {
        styler.green("routed")
    };
    let mut row = format!(
        "  {verb}  {}  {} → {route}",
        candidate.text, candidate.inbox
    );
    let previous = moved
        .and_then(|moved| moved.get("previous_block_id"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default();
    let current = moved
        .and_then(|moved| moved.get("block_id"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default();
    if !previous.is_empty() && previous != current {
        row.push_str(&format!(" · ^{previous} renamed ^{current}"));
    }
    if let Some((links, deps)) = candidate.repaired {
        if links > 0 {
            let noun = if links == 1 { "link" } else { "links" };
            row.push_str(&format!(" · {links} {noun} repaired"));
        } else if deps > 0 {
            let noun = if deps == 1 { "token" } else { "tokens" };
            row.push_str(&format!(" · {deps} dependency {noun} repaired"));
        }
    }
    println!("{row}");
}

fn print_template_summary(
    summary: &capture_language::template::TemplateSummary,
    rows: &[Option<capture_language::TemplateRowSummary>],
    saved: Option<&TemplateSavedJson>,
    not_saved: Option<&str>,
    origin: Option<&str>,
) {
    let styler = Styler::detect();
    let row_label = if summary.row_count == 1 {
        "row"
    } else {
        "rows"
    };
    let line_range = match (
        rows.first().and_then(Option::as_ref),
        rows.last().and_then(Option::as_ref),
    ) {
        (Some(first), Some(last)) if first.line == last.line => {
            format!("line {}", first.line)
        }
        (Some(first), Some(last)) => {
            format!("lines {}–{}", first.line, last.line)
        }
        _ => format!("header line {}", summary.line),
    };
    let mut line = format!(
        "{}  {} {} · {}",
        styler.dim("template"),
        summary.row_count,
        row_label,
        line_range
    );
    if let Some(saved) = saved {
        if saved.status == "replaced" {
            let replaced = origin.unwrap_or(&saved.id);
            line.push_str(&format!(
                " · saved, replacing {}",
                styler.dim(replaced)
            ));
        } else {
            line.push_str(" · saved");
        }
    } else if not_saved.is_some() {
        line.push_str(&format!(" · {}", styler.yellow("not saved")));
    }
    println!("{line}");
}

fn print_pull_in_hint(result: &CaptureResult) {
    if result.pull_in.offer_count == 0 || result.pull_in.picked_count > 0 {
        return;
    }
    // A verdict is an explicit choice, but dry-run hints teach the flags
    // whenever an unresolved offer supports at least one verdict.
    let has_verdicts = !result.offer_closes.is_empty();
    let styler = Styler::detect();
    if result
        .pull_in
        .groups
        .iter()
        .all(|group| group.group == "note_pull")
    {
        let note_rows = note_pull_rows(result);
        let mut numbers: Vec<usize> = result
            .pull_in
            .groups
            .iter()
            .filter(|group| group.group == "note_pull")
            .flat_map(|group| {
                note_rows.iter().filter_map(|row| {
                    let offer = row.offer.as_ref()?;
                    (group.note.as_deref() == Some(note_pull_source(row)))
                        .then_some(offer.n)
                })
            })
            .collect();
        numbers.sort_unstable();
        numbers.dedup();
        let mut hint = match numbers.as_slice() {
            [] => return,
            [only] => format!("nothing pulled in · choose with -p {only}"),
            [first, .., last] => {
                format!("nothing pulled in · choose with -p {first},{last} or -p all")
            }
        };
        if result.dry_run && has_unresolved_close_eligible_offer(result) {
            hint.push_str(" · -x N done · -X N[=why] cancel");
        }
        println!("  {}", styler.dim(&hint));
        return;
    }
    if result.pull_in.choice.is_some() && !has_verdicts {
        return;
    }
    if result.dry_run {
        let mut numbers: Vec<usize> = Vec::new();
        let items = if result.captures.is_empty() {
            std::slice::from_ref(&result.item)
        } else {
            result.captures.as_slice()
        };
        for item in items {
            for row in all_offer_rows(item) {
                if let Some(offer) = row.offer.as_ref() {
                    numbers.push(offer.n);
                }
            }
        }
        numbers.sort_unstable();
        numbers.dedup();
        let list = numbers
            .iter()
            .map(usize::to_string)
            .collect::<Vec<_>>()
            .join(",");
        let mut hint = format!("-p {list} · -p all");
        if has_unresolved_close_eligible_offer(result) {
            hint.push_str(" · -x N done · -X N[=why] cancel");
        }
        println!("  {}", styler.dim(&hint));
    } else {
        // Card-resolved rows were handled; only remaining offers need
        // the link-later pointer.
        let items = if result.captures.is_empty() {
            std::slice::from_ref(&result.item)
        } else {
            result.captures.as_slice()
        };
        let pending = items.iter().flat_map(all_offer_rows).any(|row| {
            row.offer
                .as_ref()
                .is_some_and(|offer| offer.verdict.is_none())
        });
        if pending {
            println!(
                "  {}",
                styler.dim(
                    "offers were not pulled in; link later in Obsidian, Bob Mac Capture, or Ctrl+Shift+Enter"
                )
            );
        }
    }
}

fn has_unresolved_close_eligible_offer(result: &CaptureResult) -> bool {
    let items = if result.captures.is_empty() {
        std::slice::from_ref(&result.item)
    } else {
        result.captures.as_slice()
    };
    items.iter().flat_map(all_offer_rows).any(|row| {
        row.offer.as_ref().is_some_and(|offer| {
            offer.verdict.is_none()
                && offer
                    .close
                    .as_ref()
                    .is_some_and(|close| close.complete || close.cancel)
        })
    })
}

fn relation_target_json(
    target: &crate::native::task_complete::RelationTarget,
) -> RelationTargetJson {
    RelationTargetJson {
        note_path: display_relative(&target.note_path),
        block_id: target.block_id.clone(),
        text: target.text.clone(),
    }
}

fn relation_meta_json(
    label: &str,
    key: &str,
    event: crate::native::task_complete::RelationEventKind,
    after: &[crate::native::task_complete::RelationTarget],
) -> RelationMetaJson {
    RelationMetaJson {
        label: label.to_string(),
        key: key.to_string(),
        event: event.as_str(),
        after: after.iter().map(relation_target_json).collect(),
    }
}

fn relation_meta_from_info(
    info: &crate::native::task_complete::successors::RelationRowInfo,
) -> RelationMetaJson {
    relation_meta_json(&info.label, &info.key, info.event, &info.after)
}

/// Map a planner row into the shared unblocked JSON shape, including a
/// winning or second-reason `relation` object when the engine attached one.
pub(super) fn unblocked_row_json(
    dependent: &crate::native::task_complete::UnblockedRow,
    bob_dir: &Path,
    day_relative: &str,
    global_filter: &str,
) -> TaskCompleteUnblockedJson {
    let clean_text = note_tasks::clean_description(
        &dependent.text,
        global_filter,
        if dependent.block_id.is_empty() {
            None
        } else {
            Some(dependent.block_id.as_str())
        },
    );
    TaskCompleteUnblockedJson {
        note_path: display_relative(&dependent.note_path),
        block_id: dependent.block_id.clone(),
        line: dependent.line,
        text: clean_text,
        previous_status_symbol: dependent.previous_status_symbol,
        previous_status_name: status_name_for(
            bob_dir,
            dependent.previous_status_symbol,
        ),
        status_symbol: dependent.status_symbol,
        status_name: status_name_for(bob_dir, dependent.status_symbol),
        inbox: dependent.inbox,
        unblocked_by: dependent
            .unblocked_by
            .iter()
            .map(|cause| TaskCompleteUnblockedCauseJson {
                note_path: display_relative(&cause.note_path),
                block_id: cause.block_id.clone(),
                text: cause.text.clone(),
            })
            .collect(),
        link: dependent.link.as_ref().map(|link| {
            TaskCompleteSuccessorLinkJson {
                day_file: day_relative.to_string(),
                entry_name: link.entry_name.clone(),
                entry_line: link.entry_line,
                entry_created: link.entry_created,
                next_up: link.next_up,
                line: link.line,
                block_link: link.block_link.clone(),
                block_id_created: link.block_id_created,
            }
        }),
        not_linked: dependent.not_linked.map(str::to_string),
        trigger: dependent.trigger,
        hide_removed: dependent.hide_removed,
        offer: dependent.offer.as_ref().map(offer_json),
        relation: dependent.relation.as_ref().map(relation_meta_from_info),
    }
}

/// Held-row JSON, including label/event/source when no offer group exists.
pub(super) fn relation_held_row_json(
    held: &crate::native::task_complete::RelationHeldRow,
    bob_dir: &Path,
    global_filter: &str,
) -> RelationHeldJson {
    use crate::native::task_complete::RelationHeldReason;
    let text = note_tasks::clean_description(
        &held.source.text,
        global_filter,
        if held.source.block_id.is_empty() {
            None
        } else {
            Some(held.source.block_id.as_str())
        },
    );
    let mut waits_on = None;
    let mut scheduled = None;
    let mut project = None;
    let mut waits_for = None;
    let reason = match &held.reason {
        RelationHeldReason::AlreadyPlanned => "already_planned",
        RelationHeldReason::AlreadyStarted => "already_started",
        RelationHeldReason::Hidden => "hidden",
        RelationHeldReason::WaitsFor(targets) => {
            waits_for =
                Some(targets.iter().map(relation_target_json).collect());
            "waits_for"
        }
        RelationHeldReason::Blocked {
            waits_on: waiting,
            scheduled: date,
            project: project_path,
        } => {
            if !waiting.is_empty() {
                waits_on =
                    Some(waiting.iter().map(relation_target_json).collect());
            }
            scheduled = date.clone();
            project = project_path.as_ref().map(|path| display_relative(path));
            "blocked"
        }
        RelationHeldReason::Scheduled(date) => {
            scheduled = Some(date.clone());
            "blocked"
        }
        RelationHeldReason::Project => "blocked",
    };
    RelationHeldJson {
        note_path: display_relative(&held.source.note_path),
        block_id: held.source.block_id.clone(),
        line: held.source_line.saturating_add(1),
        text,
        status_symbol: held.status_symbol,
        status_name: status_name_for(bob_dir, held.status_symbol),
        reason,
        waits_on,
        scheduled,
        project,
        waits_for,
        relation: relation_meta_json(
            &held.label,
            &held.key,
            held.event,
            &held.triggering_targets,
        ),
    }
}

/// `"checked"` / `"unavailable"` when the lookup ran. An enabled
/// edge-free gesture (checked, empty successors and held) stays silent.
pub(super) fn relation_check_json(
    check: Option<crate::native::task_complete::RelationCheck>,
    successors: &[TaskCompleteUnblockedJson],
    held: &[RelationHeldJson],
) -> Option<&'static str> {
    match check {
        Some(crate::native::task_complete::RelationCheck::Checked)
            if successors.is_empty() && held.is_empty() =>
        {
            None
        }
        other => other.map(crate::native::task_complete::RelationCheck::as_str),
    }
}

/// Every successor row an item reports: whole-item `task_complete`, every
/// nested `relation_completes` summary, then `pomodoro_close`. Used for
/// draft-level numbering, probes, selection validation, pull-in, and
/// human offer summaries so nested completions share one number space.
pub(super) fn all_unblocked(
    item: &CaptureItemResult,
) -> impl Iterator<Item = &TaskCompleteUnblockedJson> {
    item.task_complete
        .iter()
        .flat_map(|summary| summary.unblocked.iter())
        .chain(
            item.relation_completes
                .iter()
                .flat_map(|complete| complete.task_complete.unblocked.iter()),
        )
        .chain(
            item.pomodoro_close
                .iter()
                .flat_map(|summary| summary.unblocked.iter()),
        )
        .chain(
            item.pomodoro_past
                .iter()
                .flat_map(|past| past.close.unblocked.iter()),
        )
        .chain(
            item.project_note
                .iter()
                .flat_map(|note| note.successors.iter().flatten()),
        )
}

/// Every still-blocked row an item reports, in the same order as
/// [`all_unblocked`].
#[allow(dead_code)]
pub(super) fn all_still_blocked(
    item: &CaptureItemResult,
) -> impl Iterator<Item = &TaskCompleteStillBlockedJson> {
    item.task_complete
        .iter()
        .flat_map(|summary| summary.still_blocked.iter())
        .chain(
            item.relation_completes.iter().flat_map(|complete| {
                complete.task_complete.still_blocked.iter()
            }),
        )
        .chain(
            item.pomodoro_close
                .iter()
                .flat_map(|summary| summary.still_blocked.iter()),
        )
        .chain(
            item.pomodoro_past
                .iter()
                .flat_map(|past| past.close.still_blocked.iter()),
        )
}

/// Every project fold an item reports, in the same order as
/// [`all_unblocked`].
pub(super) fn all_project_folds(
    item: &CaptureItemResult,
) -> impl Iterator<Item = &crate::native::task_complete::ProjectFold> {
    item.task_complete
        .iter()
        .flat_map(|summary| summary.project_folds.iter())
        .chain(
            item.relation_completes.iter().flat_map(|complete| {
                complete.task_complete.project_folds.iter()
            }),
        )
        .chain(
            item.pomodoro_close
                .iter()
                .flat_map(|summary| summary.project_folds.iter()),
        )
        .chain(
            item.pomodoro_past
                .iter()
                .flat_map(|past| past.close.project_folds.iter()),
        )
        .chain(
            item.project_note
                .iter()
                .flat_map(|note| note.project_folds.iter()),
        )
}

/// Mutable access to every unblocked vec an item reports, in the same
/// order as [`all_unblocked`]: whole-item `task_complete`, each nested
/// `relation_completes` summary, then `pomodoro_close`.
pub(super) fn unblocked_vecs_mut(
    item: &mut CaptureItemResult,
) -> Vec<&mut Vec<TaskCompleteUnblockedJson>> {
    let mut vecs = Vec::new();
    if let Some(summary) = item.task_complete.as_mut() {
        vecs.push(&mut summary.unblocked);
    }
    for complete in &mut item.relation_completes {
        vecs.push(&mut complete.task_complete.unblocked);
    }
    if let Some(summary) = item.pomodoro_close.as_mut() {
        vecs.push(&mut summary.unblocked);
    }
    if let Some(past) = item.pomodoro_past.as_mut() {
        vecs.push(&mut past.close.unblocked);
    }
    if let Some(note) = item.project_note.as_mut()
        && let Some(successors) = note.successors.as_mut()
    {
        vecs.push(successors);
    }
    vecs
}

/// Every keep-moving row an item reports: whole-item `task_complete`,
/// every nested `relation_completes` summary, then `pomodoro_close`.
/// Used for draft-level numbering, probes, selection validation,
/// pull-in, and net filtering so nested completions share one number
/// space with unblocked rows.
pub(super) fn all_keep_moving(
    item: &CaptureItemResult,
) -> impl Iterator<Item = &TaskCompleteUnblockedJson> {
    item.task_complete
        .iter()
        .flat_map(|summary| summary.keep_moving.iter())
        .chain(
            item.relation_completes
                .iter()
                .flat_map(|complete| complete.task_complete.keep_moving.iter()),
        )
        .chain(
            item.pomodoro_close
                .iter()
                .flat_map(|summary| summary.keep_moving.iter()),
        )
        .chain(
            item.pomodoro_past
                .iter()
                .flat_map(|past| past.close.keep_moving.iter()),
        )
}

/// Every relation-winning successor row an item reports: item-level
/// toggle containers, whole-item `task_complete`, nested
/// `relation_completes`, then `pomodoro_close`. Relation rows stay out
/// of `unblocked[]` and `keep_moving[]`.
pub(super) fn all_relation_successors(
    item: &CaptureItemResult,
) -> impl Iterator<Item = &TaskCompleteUnblockedJson> {
    item.relation_successors.iter().chain(
        item.task_complete
            .iter()
            .flat_map(|summary| summary.relation_successors.iter())
            .chain(item.relation_completes.iter().flat_map(|complete| {
                complete.task_complete.relation_successors.iter()
            }))
            .chain(
                item.pomodoro_close
                    .iter()
                    .flat_map(|summary| summary.relation_successors.iter()),
            )
            .chain(
                item.pomodoro_past
                    .iter()
                    .flat_map(|past| past.close.relation_successors.iter()),
            ),
    )
}

/// Every successor offer row an item reports: unblocked, then relation,
/// then keep-moving, then note-pull, in the same per-summary order as
/// [`all_unblocked`]. Used for draft-level numbering, probes, selection
/// validation, pull-in counts, and refusal payloads
/// (`docs/task-dependencies.md` §12.5, §12.15, §12.20).
pub(super) fn all_offer_rows(
    item: &CaptureItemResult,
) -> impl Iterator<Item = &TaskCompleteUnblockedJson> {
    all_unblocked(item)
        .chain(all_relation_successors(item))
        .chain(all_keep_moving(item))
        .chain(all_note_pull(item))
        .chain(item.relation_merge_output.offer_rows.iter())
}

/// Mutable access to the same capture offer rows as [`all_offer_rows`].
pub(super) fn for_each_offer_row_mut(
    item: &mut CaptureItemResult,
    mut apply: impl FnMut(&mut TaskCompleteUnblockedJson),
) {
    for vec in unblocked_vecs_mut(item) {
        for row in vec.iter_mut().filter(|row| row.offer.is_some()) {
            apply(row);
        }
    }
    for vec in relation_successors_vecs_mut(item) {
        for row in vec.iter_mut().filter(|row| row.offer.is_some()) {
            apply(row);
        }
    }
    for vec in keep_moving_vecs_mut(item) {
        for row in vec.iter_mut().filter(|row| row.offer.is_some()) {
            apply(row);
        }
    }
    for vec in note_pull_vecs_mut(item) {
        for row in vec.iter_mut().filter(|row| row.offer.is_some()) {
            apply(row);
        }
    }
    for row in item
        .relation_merge_output
        .offer_rows
        .iter_mut()
        .filter(|row| row.offer.is_some())
    {
        apply(row);
    }
}

/// Every note-pull row an item reports (`docs/task-dependencies.md`
/// §12.20). Rows ride the successor-offer protocol for numbering, keys,
/// selection, and atomic writes but have no predecessor.
pub(super) fn all_note_pull(
    item: &CaptureItemResult,
) -> impl Iterator<Item = &TaskCompleteUnblockedJson> {
    item.note_pull
        .iter()
        .flat_map(|summary| summary.successors.iter())
}

/// Mutable access to every note-pull vec an item reports, in the same
/// order as [`all_note_pull`].
pub(super) fn note_pull_vecs_mut(
    item: &mut CaptureItemResult,
) -> Vec<&mut Vec<TaskCompleteUnblockedJson>> {
    let mut vecs = Vec::new();
    if let Some(summary) = item.note_pull.as_mut() {
        vecs.push(&mut summary.successors);
    }
    vecs
}

/// Every surviving relation group an item reports, in the same summary
/// order as [`all_relation_successors`].
pub(super) fn all_relation_groups(
    item: &CaptureItemResult,
) -> impl Iterator<Item = &crate::native::task_complete::RelationOfferGroup> {
    item.relation_groups.iter().chain(
        item.task_complete
            .iter()
            .flat_map(|summary| summary.relation_groups.iter())
            .chain(item.relation_completes.iter().flat_map(|complete| {
                complete.task_complete.relation_groups.iter()
            }))
            .chain(
                item.pomodoro_close
                    .iter()
                    .flat_map(|summary| summary.relation_groups.iter()),
            )
            .chain(
                item.pomodoro_past
                    .iter()
                    .flat_map(|past| past.close.relation_groups.iter()),
            ),
    )
}

/// Every relation-held row an item reports, in the same summary order as
/// [`all_relation_successors`].
pub(super) fn all_relation_held(
    item: &CaptureItemResult,
) -> impl Iterator<Item = &RelationHeldJson> {
    item.relation_held.iter().chain(
        item.task_complete
            .iter()
            .flat_map(|summary| summary.relation_held.iter())
            .chain(item.relation_completes.iter().flat_map(|complete| {
                complete.task_complete.relation_held.iter()
            }))
            .chain(
                item.pomodoro_close
                    .iter()
                    .flat_map(|summary| summary.relation_held.iter()),
            )
            .chain(
                item.pomodoro_past
                    .iter()
                    .flat_map(|past| past.close.relation_held.iter()),
            ),
    )
}

/// `"checked"` / `"unavailable"` from every summary an item reports.
/// Unavailable wins so a refusal can print the muted uncertainty line.
pub(super) fn all_relation_check<'a>(
    items: impl IntoIterator<Item = &'a CaptureItemResult>,
) -> Option<&'static str> {
    let mut checked = None;
    for item in items {
        for value in item.relation_check.into_iter().chain(
            item.task_complete
                .iter()
                .filter_map(|summary| summary.relation_check)
                .chain(item.relation_completes.iter().filter_map(|complete| {
                    complete.task_complete.relation_check
                }))
                .chain(
                    item.pomodoro_close
                        .iter()
                        .filter_map(|summary| summary.relation_check),
                )
                .chain(
                    item.pomodoro_past
                        .iter()
                        .filter_map(|past| past.close.relation_check),
                ),
        ) {
            if value == "unavailable" {
                return Some(value);
            }
            checked = Some(value);
        }
    }
    checked
}

/// Mutable access to every relation-successor vec an item reports, in
/// the same order as [`all_relation_successors`].
pub(super) fn relation_successors_vecs_mut(
    item: &mut CaptureItemResult,
) -> Vec<&mut Vec<TaskCompleteUnblockedJson>> {
    let mut vecs = Vec::new();
    vecs.push(&mut item.relation_successors);
    if let Some(summary) = item.task_complete.as_mut() {
        vecs.push(&mut summary.relation_successors);
    }
    for complete in &mut item.relation_completes {
        vecs.push(&mut complete.task_complete.relation_successors);
    }
    if let Some(summary) = item.pomodoro_close.as_mut() {
        vecs.push(&mut summary.relation_successors);
    }
    if let Some(past) = item.pomodoro_past.as_mut() {
        vecs.push(&mut past.close.relation_successors);
    }
    vecs
}

/// Mutable access to every relation group vec an item reports, in
/// the same summary order as [`all_relation_successors`]: item-level
/// toggle containers, whole-item `task_complete`, nested
/// `relation_completes`, then `pomodoro_close`. The batches net pass
/// prunes invalidated parked groups so a dropped group leaves no stale
/// internal entry for the presentation phase.
pub(super) fn relation_groups_mut(
    item: &mut CaptureItemResult,
) -> Vec<&mut Vec<crate::native::task_complete::RelationOfferGroup>> {
    let mut vecs = Vec::new();
    vecs.push(&mut item.relation_groups);
    if let Some(summary) = item.task_complete.as_mut() {
        vecs.push(&mut summary.relation_groups);
    }
    for complete in &mut item.relation_completes {
        vecs.push(&mut complete.task_complete.relation_groups);
    }
    if let Some(summary) = item.pomodoro_close.as_mut() {
        vecs.push(&mut summary.relation_groups);
    }
    if let Some(past) = item.pomodoro_past.as_mut() {
        vecs.push(&mut past.close.relation_groups);
    }
    vecs
}

/// Mutable access to every keep-moving vec an item reports, in the same
/// order as [`all_keep_moving`].
pub(super) fn keep_moving_vecs_mut(
    item: &mut CaptureItemResult,
) -> Vec<&mut Vec<TaskCompleteUnblockedJson>> {
    let mut vecs = Vec::new();
    if let Some(summary) = item.task_complete.as_mut() {
        vecs.push(&mut summary.keep_moving);
    }
    for complete in &mut item.relation_completes {
        vecs.push(&mut complete.task_complete.keep_moving);
    }
    if let Some(summary) = item.pomodoro_close.as_mut() {
        vecs.push(&mut summary.keep_moving);
    }
    if let Some(past) = item.pomodoro_past.as_mut() {
        vecs.push(&mut past.close.keep_moving);
    }
    vecs
}

/// Mutable access to every keep-moving vec an item reports together with
/// its folded counts, in the same order as [`all_keep_moving`]. The net
/// pass prunes both at once so a dropped group leaves no stale counts.
pub(super) fn keep_moving_parts_mut(
    item: &mut CaptureItemResult,
) -> Vec<(
    &mut Vec<TaskCompleteUnblockedJson>,
    &mut Vec<crate::native::task_complete::KeepMovingFold>,
)> {
    let mut parts = Vec::new();
    if let Some(summary) = item.task_complete.as_mut() {
        parts.push((&mut summary.keep_moving, &mut summary.keep_moving_folds));
    }
    for complete in &mut item.relation_completes {
        let summary = &mut complete.task_complete;
        parts.push((&mut summary.keep_moving, &mut summary.keep_moving_folds));
    }
    if let Some(summary) = item.pomodoro_close.as_mut() {
        parts.push((&mut summary.keep_moving, &mut summary.keep_moving_folds));
    }
    if let Some(past) = item.pomodoro_past.as_mut() {
        parts.push((
            &mut past.close.keep_moving,
            &mut past.close.keep_moving_folds,
        ));
    }
    parts
}

/// Every keep-moving fold an item reports, in the same order as
/// [`all_keep_moving`].
pub(super) fn all_keep_moving_folds(
    item: &CaptureItemResult,
) -> impl Iterator<Item = &crate::native::task_complete::KeepMovingFold> {
    item.task_complete
        .iter()
        .flat_map(|summary| summary.keep_moving_folds.iter())
        .chain(item.relation_completes.iter().flat_map(|complete| {
            complete.task_complete.keep_moving_folds.iter()
        }))
        .chain(
            item.pomodoro_close
                .iter()
                .flat_map(|summary| summary.keep_moving_folds.iter()),
        )
        .chain(
            item.pomodoro_past
                .iter()
                .flat_map(|past| past.close.keep_moving_folds.iter()),
        )
}

/// Shared `!` detail rows for reuse by relation rows: closed subtasks,
/// left-open descendants, ledger effects, unblocked dependents, and
/// still-blocked dependents. The `!` heading stays with the caller so
/// old output stays byte-identical.
pub(super) fn print_task_complete_details(
    summary: &TaskCompleteSummaryJson,
    dry_run: bool,
    styler: &Styler,
) {
    for subtask in &summary.subtasks {
        let previous =
            style_task_status_marker(styler, subtask.previous_status_symbol);
        let current = style_task_status_marker(styler, subtask.status_symbol);
        let locator =
            task_complete_locator(&subtask.note_path, &subtask.block_id);
        if subtask.text.is_empty() {
            println!("    {previous} → {current}  {locator}");
        } else {
            println!("    {previous} → {current} {}  {locator}", subtask.text);
        }
    }
    for left in &summary.subtasks_left_open {
        let marker = style_task_status_marker(styler, left.status_symbol);
        let locator = task_complete_locator(&left.note_path, &left.block_id);
        if left.text.is_empty() {
            println!("    left {} {marker}  {locator}", left.status_name);
        } else {
            println!(
                "    left {} {marker} {}  {locator}",
                left.status_name, left.text
            );
        }
    }
    if let Some(ledger) = summary.ledger.as_ref() {
        let mut parts: Vec<String> = Vec::new();
        for struck in &ledger.struck_in {
            let label = ledger_entry_label(&struck.name, struck.line);
            if struck.status == "completed" {
                parts.push(format!("Task Link struck in {label} (completed)"));
            } else {
                parts.push(format!("Task Link struck in {label}"));
            }
        }
        for item in &ledger.moved {
            parts.push(format!(
                "Task Link moved {} → {} (struck)",
                ledger_entry_label(&item.from.name, item.from.line),
                ledger_entry_label(&item.to.name, item.to.line),
            ));
        }
        for item in &ledger.dropped {
            parts.push(format!(
                "Task Link already in {}; dropped the {} copy",
                ledger_entry_label(&item.to.name, item.to.line),
                ledger_entry_label(&item.from.name, item.from.line),
            ));
        }
        for removed in &ledger.removed_placeholders {
            parts.push(format!("removed empty {}", removed.name));
        }
        parts.push(ledger.day_file.clone());
        println!("  ledger  {}", parts.join(" · "));
    }
    print_human_successor_rows(
        &summary.unblocked,
        &summary.still_blocked,
        RelationHumanOutput {
            rows: &summary.relation_successors,
            groups: &summary.relation_groups,
            held: &summary.relation_held,
            check: summary.relation_check,
        },
        dry_run,
        styler,
    );
    print_human_keep_moving_rows(
        &summary.keep_moving,
        &summary.keep_moving_folds,
        dry_run,
        styler,
    );
}

fn pulled_in_phrase(
    unblocked: &[TaskCompleteUnblockedJson],
    keep_moving: &[TaskCompleteUnblockedJson],
    relation: &[TaskCompleteUnblockedJson],
) -> String {
    let rows = || {
        unblocked
            .iter()
            .chain(keep_moving.iter())
            .chain(relation.iter())
    };
    let offer_count = rows().filter(|row| row.offer.is_some()).count();
    let picked_count = rows()
        .filter(|row| row.offer.as_ref().is_some_and(|offer| offer.selected))
        .count();
    if picked_count == 0 {
        String::new()
    } else {
        format!(" · pulled in {picked_count} of {offer_count}")
    }
}

pub(super) fn print_global_destination_summary(
    global: &GlobalDestinationSummary,
) {
    let styler = Styler::detect();
    let route_label = format!("{}.md", global.route);
    println!("{}  {}", styler.dim("global"), styler.cyan(&route_label));
}

/// Human lines for a reference item, matching the reading-queue
/// wording table: a queued headline names the display URL, an
/// unchanged headline names the library path or display URL, and one
/// dim detail line says what happens next.
pub(super) fn print_human_ref_item_success(
    result: &CaptureItemResult,
    reference: &RefItemJson,
    styler: &Styler,
    prefix: &str,
    ordinal: &str,
) {
    let queued = matches!(result.placement, Placement::Queued);
    let parent_route = reference.parent.route.as_str();
    let (verb, subject) = match reference.library.verdict {
        "in_library" => (
            "already in library",
            styler.cyan(
                reference
                    .library
                    .path
                    .as_deref()
                    .unwrap_or(&reference.display),
            ),
        ),
        "in_intake" => (
            "already queued",
            styler.cyan(
                reference
                    .library
                    .path
                    .as_deref()
                    .unwrap_or(&reference.display),
            ),
        ),
        "clipping" => ("already clipping", styler.cyan(&reference.display)),
        "duplicate" => ("duplicate", styler.cyan(&reference.display)),
        _ if queued && result.dry_run => (
            "would queue",
            styler.cyan(&format!("{} → {parent_route}", reference.display)),
        ),
        _ => (
            "queued",
            styler.cyan(&format!("{} → {parent_route}", reference.display)),
        ),
    };
    // A small “Listen” cue keeps the card and CLI wording aligned:
    // queued means the request was saved, never that narration finished.
    let subject = if reference.listen {
        format!("{subject} · Listen")
    } else {
        subject.to_string()
    };
    println!("{prefix} {verb}  {ordinal}{subject}");
    let mut detail = match reference.library.verdict {
        "in_library" => match reference.library.title.as_deref() {
            Some(title) => match reference.library.reading_state.as_deref() {
                Some(state) => format!("{title} · {state}"),
                None => title.to_string(),
            },
            None => reference
                .library
                .path
                .clone()
                .unwrap_or_else(|| reference.display.clone()),
        },
        "in_intake" => "waiting for bob ref scan".to_string(),
        "clipping" => {
            "a pending ref job has this link · bob ref jobs".to_string()
        }
        "duplicate" => reference
            .library
            .message
            .clone()
            .unwrap_or_else(|| "same link as an earlier item".to_string()),
        "legacy" => format!(
            "in your library as a legacy note ({}) · a fresh copy will be clipped",
            reference.library.path.as_deref().unwrap_or("?"),
        ),
        "unknown" => format!(
            "library check unavailable: {} · the clip still dedupes",
            reference.library.message.as_deref().unwrap_or("unknown error"),
        ),
        _ if result.dry_run => {
            if parent_route == "mac_inbox" {
                "new to your library · reading task lands in mac_inbox.md · file it later"
                    .to_string()
            } else {
                format!(
                    "new to your library · reading task lands in {parent_route}.md"
                )
            }
        }
        _ => {
            if parent_route == "mac_inbox" {
                "clipping in the background · reading task lands in mac_inbox.md · file it later"
                    .to_string()
            } else {
                format!(
                    "clipping in the background · reading task lands in {parent_route}.md"
                )
            }
        }
    };
    if reference.listen {
        if queued {
            detail.push_str(" · companion audio requested");
        } else if let Some(hint) = reference.listen_hint.as_deref() {
            detail.push_str(&format!(" · {hint}"));
        }
    }
    println!("  {}", styler.dim(&detail));
}

pub(super) fn print_human_item_success(
    result: &CaptureItemResult,
    ordinal: Option<(usize, usize)>,
    theme_refs: &BTreeMap<String, Vec<super::super::pomodoro_themes::ThemeRef>>,
) {
    let styler = Styler::detect();
    let target_label = if result.route_label.is_empty() {
        result.relative_target.as_str()
    } else {
        result.route_label.as_str()
    };
    let target_label = styler.cyan(target_label);
    let prefix = if result.dry_run {
        styler.success_prefix(true)
    } else {
        styler.green("\u{2713}")
    };
    let ordinal = ordinal
        .map(|(index, total)| format!("{index}/{total}  "))
        .unwrap_or_default();
    if let Some(reset) = result.pomodoro_reset.as_ref() {
        print_human_pomodoro_reset_success(
            result,
            reset,
            &styler,
            &prefix,
            &ordinal,
            &target_label,
        );
        print_consumed_links(result, &styler);
        return;
    }
    if let Some(past) = result.pomodoro_past.as_ref() {
        print_human_pomodoro_past_success(
            result, past, &styler, &prefix, &ordinal,
        );
        print_consumed_links(result, &styler);
        return;
    }
    if let Some(close) = result.pomodoro_close.as_ref() {
        print_human_pomodoro_close_success(
            result,
            close,
            &styler,
            &prefix,
            &ordinal,
            &target_label,
        );
        print_consumed_links(result, &styler);
        return;
    }
    if result.kind == "pomodoro_link" {
        print_human_pomodoro_link_success(
            result,
            &styler,
            &prefix,
            &ordinal,
            &target_label,
        );
        print_consumed_links(result, &styler);
        return;
    }
    if result.kind == "link_guard" {
        let id = result
            .block_id
            .as_deref()
            .map(|id| format!("^{}", id))
            .unwrap_or_else(|| "the blocked task".to_string());
        println!("{prefix} not linked  {ordinal}{target_label}");
        println!("  {}", styler.dim(&format!("{id} was not linked")));
        print_human_link_guard(result, &styler);
        if result.consumed_links.is_empty()
            && let Some(count) =
                result.removed_pomodoro_links.filter(|n| *n > 0)
        {
            println!(
                "  {}",
                styler.dim(&format!("removed {count} live Pomodoro link(s)"))
            );
        }
        print_consumed_links(result, &styler);
        return;
    }
    if result.kind == "task_toggle"
        && (result.toggle_direction.is_some() || result.link_guard.is_some())
    {
        print_human_task_toggle_success(
            result,
            &styler,
            &prefix,
            &ordinal,
            &target_label,
        );
        print_consumed_links(result, &styler);
        return;
    }
    if result.kind == "task_complete" {
        print_human_task_complete_success(
            result,
            &styler,
            &prefix,
            &ordinal,
            &target_label,
        );
        print_consumed_links(result, &styler);
        return;
    }
    // A dependency-only action is never labeled a task creation or a
    // Pomodoro link: it reports whose prerequisites changed.
    if result.kind == "task_dependency" {
        print_human_task_dependency_success(
            result,
            &styler,
            &prefix,
            &ordinal,
            &target_label,
        );
        print_consumed_links(result, &styler);
        return;
    }
    if let Some(adjust) = result.pomodoro_adjust.as_ref() {
        print_human_pomodoro_adjust_success(
            result,
            adjust,
            &styler,
            &prefix,
            &ordinal,
            &target_label,
        );
        print_consumed_links(result, &styler);
        return;
    }
    if let Some(shift) = result.pomodoro_shift.as_ref() {
        print_human_pomodoro_shift_success(
            result,
            shift,
            &styler,
            &prefix,
            &ordinal,
            &target_label,
        );
        print_consumed_links(result, &styler);
        return;
    }
    if let Some(split) = result.pomodoro_split.as_ref() {
        print_human_pomodoro_split_success(
            result,
            split,
            &styler,
            &prefix,
            &ordinal,
            &target_label,
        );
        print_consumed_links(result, &styler);
        return;
    }
    if let Some(merge) = result.pomodoro_merge.as_ref() {
        print_human_pomodoro_merge_success(
            result,
            merge,
            &styler,
            &prefix,
            &ordinal,
            &target_label,
        );
        print_consumed_links(result, &styler);
        return;
    }
    if result.kind == "pomodoro_start"
        && let Some(start) = result.pomodoro_start.as_ref()
    {
        print_human_pomodoro_start_success(
            result,
            start,
            &styler,
            &prefix,
            &ordinal,
            &target_label,
        );
        print_consumed_links(result, &styler);
        return;
    }
    if result.kind == "ref"
        && let Some(reference) = result.r#ref.as_ref()
    {
        print_human_ref_item_success(
            result, reference, &styler, &prefix, &ordinal,
        );
        print_consumed_links(result, &styler);
        return;
    }
    if matches!(result.kind, "record_note" | "record_update")
        && let Some(record) = result.record_note.as_ref()
    {
        print_human_record_note_success(
            result,
            record,
            &styler,
            &prefix,
            &ordinal,
            &target_label,
        );
        return;
    }
    if matches!(
        result.kind,
        "notebook_note" | "notebook_sub_note" | "notebook_create"
    ) && let Some(notebook) = result.notebook.as_ref()
    {
        print_human_notebook_success(
            result, notebook, &styler, &prefix, &ordinal,
        );
        return;
    }
    if result.note_pull.is_some() {
        return;
    }
    if result.kind == "task_merge" {
        print_human_transfer_merges(result);
        return;
    }
    if result.kind == "task_move"
        && !result.transfer_move_output.moves.is_empty()
    {
        print_human_transfer_moves(result, &styler);
        print_consumed_links(result, &styler);
        return;
    }
    let verb = if result.dry_run {
        "would capture"
    } else {
        "captured"
    };
    println!("{prefix} {verb}  {ordinal}{target_label}");
    if let Some(note) = result.project_note.as_ref() {
        println!("  parent  {}", styler.cyan(&note.parent_link));
    }
    if let Some(parent_text) = result.parent_text.as_deref() {
        let marker = parent_under_marker(result, &styler);
        let block_id = result
            .block_id
            .as_deref()
            .map(|id| format!("  {}", styler.cyan(&format!("^{id}"))))
            .unwrap_or_default();
        if !result.notes.is_empty() {
            // Multi-note: `under … ^bar · 4 notes`, or with the single
            // drawer that received every note.
            let count = result.notes.len();
            let note_word = if count == 1 { "note" } else { "notes" };
            if let Some(drawer) = result.drawer.as_ref() {
                let base = format!(" · {}", styler.cyan(&drawer.title));
                let created = if drawer.created {
                    "        new drawer"
                } else {
                    ""
                };
                // Glued items read `· REQUIREMENTS · 2 notes`.
                println!(
                    "  under {marker}{parent_text}{block_id}{base} · {count} {note_word}{created}"
                );
            } else {
                println!(
                    "  under {marker}{parent_text}{block_id} · {count} {note_word}"
                );
            }
        } else {
            let drawer_suffix = result
                .drawer
                .as_ref()
                .map(|drawer| {
                    let base = format!(" · {}", styler.cyan(&drawer.title));
                    if drawer.created {
                        format!("{base}        new drawer")
                    } else {
                        base
                    }
                })
                .unwrap_or_default();
            println!("  under {marker}{parent_text}{block_id}{drawer_suffix}");
        }
    }
    if !result.notes.is_empty() {
        print_human_multi_note_notes(result, &styler);
    } else if !(result.kind == "sub_bullet" && result.task_line.is_empty()) {
        println!("  {}", styler.dim(&result.task_line));
    }
    print_human_link_guard(result, &styler);
    if result.kind == "pomodoro_task"
        && let Some(destination) = result.pomodoro_link_destination.as_ref()
    {
        println!(
            "  {}",
            styler.dim(&format_styled_pomodoro_task_destination(
                destination,
                theme_refs,
                &styler,
            ))
        );
    }
    if let Some(row) = format_task_theme_row(result, &styler) {
        print!("{row}");
    }
    if let Some(row) = format_type_relocation_row(result) {
        print!("{row}");
    }
    if let Some(note) = result.project_note.as_ref() {
        let sections = if note.sections.is_empty() {
            "—".to_string()
        } else {
            note.sections.join(", ")
        };
        let task_word = if note.tasks == 1 { "task" } else { "tasks" };
        let moved = note
            .moved_tasks
            .first()
            .map(|first| {
                format!(
                    " · {} moved from {}",
                    note.moved_tasks.len(),
                    first.source
                )
            })
            .unwrap_or_default();
        println!(
            "  {}",
            styler.dim(&format!(
                "{} {task_word}{moved} · sections {sections}",
                note.tasks,
            ))
        );
        print_human_project_note_moves(note, result.dry_run, &styler);
    }
    for line in &result.sub_bullets {
        println!("  {}", styler.dim(line));
    }
    print_human_dependency_updates(result, &styler);
    print_human_relation_updates(result, &styler);
    if let Some(clip) = &result.clip {
        if let Some(history) = &clip.history {
            println!("  {}", styler.dim(&format_clip_history_summary(history)));
        }
        for line in &clip.lines {
            println!("  {}", styler.dim(line));
        }
        for (saved, reused) in clip.file_confirmations() {
            print_clip_file_confirmation(
                &styler,
                result.dry_run,
                &saved,
                reused,
            );
        }
    }
    if let Some(schedule_log) = &result.schedule_log {
        for line in &schedule_log.lines {
            println!("  {}", styler.dim(line));
        }
    }
    if let (Some(day_file), Some(block_link)) =
        (&result.day_file, &result.block_link)
    {
        let link_verb = if result.dry_run {
            "would link"
        } else {
            "linked"
        };
        println!("{prefix} {link_verb}   {}", styler.cyan(day_file));
        println!("  {}", styler.dim(&format!("- {block_link}")));
    }
    if let Some(start) = result.pomodoro_start.as_ref() {
        let name = start.pomodoro_name.as_deref().unwrap_or("next session");
        let created = if start.created_pomodoro {
            " (created)"
        } else {
            ""
        };
        let verb = if result.dry_run {
            "would start"
        } else {
            "started"
        };
        println!(
            "  {}",
            styler.dim(&format!(
                "{verb} {name} {}-{} ({}m{}){created} at line {}{}",
                start.start,
                start.end,
                start.duration_minutes,
                off_clock_label(start),
                start.pomodoro_line,
                calendar_label(start),
            ))
        );
    }
    if let Some(note) = result.project_note.as_ref()
        && !note.task_links.is_empty()
    {
        let link_verb = if result.dry_run {
            "would link"
        } else {
            "linked"
        };
        if let Some(day_file) = result.day_file.as_deref() {
            println!("{prefix} {link_verb}   {}", styler.cyan(day_file));
        }
        if let Some(name) = result.pomodoro_name.as_deref() {
            let created = if result.creates_pomodoro == Some(true) {
                " (created)"
            } else {
                ""
            };
            println!("  under {name}{created}");
        }
        for link in &note.task_links {
            println!("  {}", styler.dim(&format!("- {}", link.block_link)));
        }
    }
    if let Some(note) = result.project_note.as_ref()
        && let Some(successors) = note.successors.as_ref()
    {
        print_human_successor_rows(
            successors,
            &[],
            RelationHumanOutput::empty(),
            result.dry_run,
            &styler,
        );
    }
    if result.project_note.is_some() {
        println!(
            "  {}",
            styler.dim(
                "hint: 'bob projects sync' adds the parent's Sub-projects line; 'bob task reconcile' reconciles Blocked state"
            )
        );
    }
    print_consumed_links(result, &styler);
}

fn format_clip_history_summary(
    history: &capture_clip::ClipHistoryOutput,
) -> String {
    let provider = match history.provider.as_str() {
        "alfred" => "Alfred",
        "clipy" => "Clipy",
        other => other,
    };
    let Some(first) = history.entries.first() else {
        return format!("clip  from {provider}");
    };
    let Some(newest) = first.age_seconds else {
        return format!("clip  from {provider}");
    };
    let Some(oldest) =
        history.entries.last().and_then(|entry| entry.age_seconds)
    else {
        return format!("clip  from {provider}");
    };
    format!(
        "clip  from {provider} · newest {} · oldest {}",
        crate::native::capture_templates::command::age_unit(newest),
        crate::native::capture_templates::command::age_unit(oldest),
    )
}

fn print_human_project_note_moves(
    note: &ProjectNoteSummary,
    dry_run: bool,
    styler: &Styler,
) {
    let move_verb = if dry_run { "would move" } else { "moved" };
    for moved in &note.moved_tasks {
        let status =
            style_task_status_marker(styler, moved.previous_status_symbol);
        let text = styler.dim(&moved.text);
        let location = styler.cyan(&format!(
            "{}:{} → ^{}",
            moved.source, moved.source_line, moved.block_id
        ));
        let consequence = moved
            .detail
            .as_ref()
            .map(|detail| format!(" · {detail}"))
            .unwrap_or_default();
        println!("  {move_verb}  {status} {text}  {location}{consequence}");
    }

    if let Some(files) = note.repaired_files.as_ref() {
        let repaired_links: usize =
            files.iter().map(|file| file.link_count).sum();
        let repaired_link_notes =
            files.iter().filter(|file| file.link_count > 0).count();
        if repaired_links > 0 {
            let repair_verb = if dry_run { "would repair" } else { "repaired" };
            let link_word = if repaired_links == 1 { "link" } else { "links" };
            let note_word = if repaired_link_notes == 1 {
                "note"
            } else {
                "notes"
            };
            println!(
                "  {repair_verb}  {repaired_links} {link_word} in {repaired_link_notes} {note_word}"
            );
        }
    }

    if let Some(hint) = note.move_hint.as_ref() {
        let task_word = if hint.open_count == 1 {
            "task"
        } else {
            "tasks"
        };
        println!(
            "  {}",
            styler.dim(&format!(
                "hint: '- ^' moves one of {}'s {} open {} into this project; '- ^^' lets you choose several",
                hint.parent, hint.open_count, task_word
            ))
        );
    }
}

/// Human rows for a bare-note `>` transfer: the shared move header plus
/// one row per source in typed order and the canonical repair row.
/// Dry runs use the `Would move` / `would repair` wording.
fn print_human_transfer_merges(result: &CaptureItemResult) {
    let records = &result.relation_merge_output.merges;
    let Some(first) = records.first() else { return };
    let target = first
        .target
        .title
        .as_deref()
        .or(first.target.section.as_deref())
        .unwrap_or("destination");
    let suffix = first
        .target
        .drawer
        .as_deref()
        .or(first.target.section.as_deref())
        .map(|title| format!(" › {title}"))
        .unwrap_or_default();
    // A capture-born task target (merge-in `<` into a new task) uses the
    // dedicated new-target header; existing task/drawer/section targets
    // keep the established transfer wording.
    let is_new_task_target =
        first.target.created && first.target.kind == "task";
    if is_new_task_target {
        let action = if result.dry_run {
            "Would merge"
        } else {
            "Merged"
        };
        println!(
            "⤵ {action} {} {} into new “{target}” · {}",
            records.len(),
            if records.len() == 1 { "task" } else { "tasks" },
            display_relative(&first.target.path)
        );
    } else {
        let action = if result.dry_run {
            "would merge"
        } else {
            "merged"
        };
        println!(
            "⤵ {action} {} {} into “{target}” · {}{suffix}",
            records.len(),
            if records.len() == 1 { "task" } else { "tasks" },
            display_relative(&first.target.path)
        );
    }
    let mut printed_dependents = std::collections::BTreeSet::new();
    for record in records {
        println!("  {}", record.message);
        for consequence in &record.consequences {
            match consequence.kind.as_str() {
                "dependent" => {
                    let already_printed = !consequence.keys.is_empty()
                        && consequence
                            .keys
                            .iter()
                            .all(|key| printed_dependents.contains(key));
                    if already_printed {
                        continue;
                    }
                    printed_dependents.extend(consequence.keys.iter().cloned());
                    println!("  ⛓ {}", consequence.text);
                }
                "prerequisite_left" | "successor_orphaned" => {
                    println!("  ⚠ {}", consequence.text);
                }
                _ => {}
            }
        }
        for offer in &record.offers {
            let resolved_offer = result
                .relation_merge_output
                .offer_rows
                .iter()
                .filter_map(|row| row.offer.as_ref())
                .find(|candidate| candidate.key == offer.key)
                .or_else(|| {
                    result
                        .relation_merge_output
                        .offer_rows
                        .iter()
                        .filter_map(|row| row.offer.as_ref())
                        .find(|candidate| candidate.n == offer.n)
                });
            let verdict = resolved_offer.and_then(|candidate| {
                candidate
                    .verdict_message
                    .as_deref()
                    .map(|message| (message, &candidate.verdict_ticklers))
            });
            if let Some((message, ticklers)) = verdict {
                println!(
                    "  ◌ {} link removed · offer {}  {message}",
                    offer.entry_name, offer.n
                );
                for tickler in ticklers {
                    println!("  {tickler}");
                }
                continue;
            }
            println!(
                "  ◌ {} link removed · offer {}: link “{target}” under {}{}",
                offer.entry_name,
                offer.n,
                offer.entry_name,
                if offer.selected {
                    String::from(" · selected")
                } else {
                    format!(" (-p {})", offer.n)
                }
            );
        }
    }
}

/// Label merges (and future `<` merges on ordinary kinds) share the same
/// human merge block that `>` uses: header, source rows, consequences,
/// and numbered offers. Normal creation/relation lines are already
/// printed by `print_human_item_success`; the block prints once here.
/// `task_merge` items already returned through their dedicated path.
fn print_human_merge_block_unless_task_merge(result: &CaptureItemResult) {
    if result.kind == "task_merge" {
        return;
    }
    if result.relation_merge_output.merges.is_empty() {
        return;
    }
    print_human_transfer_merges(result);
}

fn print_human_transfer_moves(result: &CaptureItemResult, styler: &Styler) {
    let output = &result.transfer_move_output;
    let count = output.moves.len();
    let task_word = if count == 1 { "task" } else { "tasks" };
    let destination = if result.relative_target.is_empty() {
        result.route_label.clone()
    } else {
        result.relative_target.clone()
    };
    let header_verb = if result.dry_run {
        "Would move"
    } else {
        "Moved"
    };
    println!(
        "{}",
        styler.bold(&format!(
            "↪ {header_verb} {count} {task_word} into {destination} › Tasks"
        ))
    );
    for movement in &output.moves {
        let identity = match (&movement.old_block_id, &movement.new_block_id) {
            (Some(old), Some(new)) if old != new => {
                format!("^{old} → ^{new}")
            }
            (Some(id), _) => format!("^{id}"),
            _ => "(no block id)".to_string(),
        };
        println!(
            "  {} {}:{} → {}  {}",
            styler.cyan(&identity),
            movement.source_path,
            movement.source_line,
            movement.destination_path,
            movement.title
        );
    }
    let repaired_links: usize = output
        .repaired_files
        .iter()
        .map(|file| file.link_count)
        .sum();
    let repaired_notes = output
        .repaired_files
        .iter()
        .filter(|file| file.link_count > 0)
        .count();
    if repaired_links > 0 {
        let repair_verb = if result.dry_run {
            "would repair"
        } else {
            "repaired"
        };
        let link_word = if repaired_links == 1 { "link" } else { "links" };
        let note_word = if repaired_notes == 1 { "note" } else { "notes" };
        println!(
            "  ↻ {repair_verb} {repaired_links} {link_word} in {repaired_notes} {note_word}"
        );
    }
}

/// Render one close task row's typed Work Log entries, each followed by
/// its details indented two more spaces. A missing details element counts
/// as empty, so short rows still render.
fn typed_work_log_lines(
    task: &PomodoroCloseTaskJson,
    log_indent: &str,
) -> Vec<String> {
    let mut lines = Vec::new();
    for (position, entry) in task.typed_work_log.iter().enumerate() {
        lines.push(format!("{log_indent}{entry}"));
        if let Some(details) = task.typed_work_log_details.get(position) {
            for detail in details {
                lines.push(format!("{log_indent}  {detail}"));
            }
        }
    }
    lines
}

pub(super) fn print_human_pomodoro_reset_success(
    result: &CaptureItemResult,
    reset: &PomodoroResetSummaryJson,
    styler: &Styler,
    prefix: &str,
    ordinal: &str,
    _target_label: &str,
) {
    let verb = if result.dry_run {
        "would reset"
    } else {
        "reset"
    };
    let name = reset.pomodoro_name.as_deref().unwrap_or("session");
    println!(
        "{prefix} {ordinal}{verb} {name} · {} line {}",
        reset.day_relative, reset.pomodoro_line
    );
    println!(
        "  {}",
        styler.dim("now the first future Pomodoro with its contents kept")
    );
    if result.routed
        && let (Some(action), Some(dest)) = (
            result.pomodoro_link_action,
            result.pomodoro_link_destination.as_ref(),
        )
    {
        let dest_name = dest.name.as_deref().unwrap_or("session");
        println!(
            "  {}",
            styler.dim(&format!(
                "{action} into {dest_name} at line {}",
                dest.line
            ))
        );
    }
    println!("  {}", styler.dim(&reset.entry_line));
    print_human_dropped_dependencies(result, styler);
}

pub(super) fn print_human_pomodoro_close_success(
    result: &CaptureItemResult,
    close: &PomodoroCloseSummaryJson,
    styler: &Styler,
    prefix: &str,
    ordinal: &str,
    _target_label: &str,
) {
    let verb = if result.dry_run {
        "would close"
    } else {
        "closed"
    };
    let name = close
        .closed_name
        .as_deref()
        .or(close.pomodoro_name.as_deref())
        .unwrap_or("session");
    // R6: a narrowed name says where the session came from.
    let narrowed_from = match (&close.closed_name, &close.pomodoro_name) {
        (Some(closed), Some(running)) if closed != running => {
            format!(" · from {running}")
        }
        _ => String::new(),
    };
    let off_clock = crate::native::pomodoro_homes::off_clock_via_label(
        close.off_clock,
        close.clock_note.as_deref(),
    );
    let range_text = if let Some(size) = close.size.as_ref() {
        let from = &size.from.time_range;
        let closed_range =
            format!("{}-{}", close.closed.start, close.closed.end);
        let delta_text = if size.delta_minutes == 0 {
            String::new()
        } else if size.delta_minutes < 0 {
            format!(", \u{2212}{}m", size.delta_minutes.abs())
        } else {
            format!(", +{}m", size.delta_minutes)
        };
        if from == &closed_range || size.delta_minutes == 0 {
            format!(
                "{closed_range} ({}m{off_clock} · {}x)",
                close.closed.duration_minutes, size.raw,
            )
        } else {
            format!(
                "{from} → {closed_range} ({}m{off_clock} · {}x{delta_text})",
                close.closed.duration_minutes, size.raw,
            )
        }
    } else if close.decremented_minutes > 0 {
        format!(
            "{}-{} → {}-{} ({}m{}, −{}m)",
            close.planned.start,
            close.planned.end,
            close.closed.start,
            close.closed.end,
            close.closed.duration_minutes,
            off_clock,
            close.decremented_minutes
        )
    } else {
        format!(
            "{}-{} ({}m{})",
            close.closed.start,
            close.closed.end,
            close.closed.duration_minutes,
            off_clock
        )
    };
    let mut header = format!(
        "{verb} {name} {range_text}{narrowed_from} · {} line {}",
        close.day_relative, close.pomodoro_line
    );
    if close.size.is_none() && close.remaining_minutes < 0 {
        header.push_str(&format!(
            " (ran {}m over)",
            close.remaining_minutes.abs()
        ));
    }
    println!("{prefix} {ordinal}{header}");
    print_close_body(result, close, styler);
}

fn print_close_body(
    result: &CaptureItemResult,
    close: &PomodoroCloseSummaryJson,
    styler: &Styler,
) {
    // Link forms: existing link status and ledger lines.
    if result.routed
        && let (Some(action), Some(dest)) = (
            result.pomodoro_link_action,
            result.pomodoro_link_destination.as_ref(),
        )
    {
        let dest_name = dest.name.as_deref().unwrap_or("session");
        let source_text = result
            .pomodoro_link_source
            .as_ref()
            .map(|source| {
                let source_name = source.name.as_deref().unwrap_or("session");
                format!(" from {source_name}")
            })
            .unwrap_or_default();
        println!(
            "  {}",
            styler.dim(&format!(
                "{action} into {dest_name} at line {}{source_text}",
                dest.line
            ))
        );
    }
    print_human_link_guard(result, styler);
    print_human_dropped_dependencies(result, styler);
    // Numbered index column: width of the highest numbered row, plus one
    // space. Unnumbered rows get blanks so text stays aligned; with zero
    // numbered rows the output below is byte-identical to before.
    let index_width = close
        .tasks
        .iter()
        .filter_map(|task| task.index)
        .map(|index| index.to_string().len())
        .max()
        .unwrap_or(0);
    let numbered = index_width > 0;
    let outcome_of = |index: u32| -> (&'static str, &'static str) {
        close
            .task_links
            .iter()
            .find(|link| link.index == index)
            .map(|link| (link.outcome, link.source))
            .unwrap_or(("deferred", "unlisted"))
    };
    // Bold index tinted by outcome: in progress uses the `/` marker color,
    // complete uses the `x` marker color (dim), deferred is dim, and
    // unlisted rows are dim so chosen rows stand out.
    let style_index = |index: u32| -> String {
        let (outcome, source) = outcome_of(index);
        let text = index.to_string();
        if source == "unlisted" {
            styler.dim(&text)
        } else if outcome == "in_progress" || outcome == "parked" {
            // Parked gets normal In Progress work effects, so it shares the
            // `/` marker color; the caption carries the not-carried signal.
            styler.blue(&text)
        } else {
            // Complete uses the `x` marker color (dim); deferred is dim.
            // Bold (`1;`) so listed rows stand out from unlisted ones.
            styler.paint("1;2", &text)
        }
    };
    let row_prefix = |index: Option<u32>| -> String {
        if !numbered {
            return "  ".to_string();
        }
        match index {
            Some(number) => {
                let digits = number.to_string().len();
                format!(
                    "  {}{} ",
                    " ".repeat(index_width.saturating_sub(digits)),
                    style_index(number)
                )
            }
            None => format!("  {} ", " ".repeat(index_width)),
        }
    };
    let log_indent = if numbered {
        " ".repeat(4 + index_width + 1)
    } else {
        "    ".to_string()
    };
    for task in &close.tasks {
        // Dropped rows name the task number directly (`dropped 4
        // [[sase#^x]]`), with a lane caption: a dropped task keeps its
        // lane, so every dropped row says which status it stays in.
        if task.role == "dropped" && task.resolved {
            let mut line = match task.index {
                Some(index) => format!("dropped {index} {}", task.block_link),
                None => format!("dropped {}", task.block_link),
            };
            if let Some(status) = task
                .status_name
                .as_deref()
                .or(task.previous_status_name.as_deref())
            {
                line.push_str(&format!(" · stays {status}"));
            }
            println!("  {line}");
            continue;
        }
        let prefix = row_prefix(task.index);
        if !task.resolved {
            let warning =
                task.warning.as_deref().unwrap_or("unresolved target");
            println!("{prefix}{}", styler.dim(&format!("warning: {warning}")));
            continue;
        }
        let transition = match (
            task.previous_status_symbol,
            task.status_symbol,
            task.role,
        ) {
            (Some(previous), Some(current), _) if previous != current => {
                let previous_marker =
                    style_task_status_marker(styler, previous);
                let current_marker = style_task_status_marker(styler, current);
                format!("{previous_marker} → {current_marker}")
            }
            (_, _, "deferred") => {
                let marker = task
                    .previous_status_symbol
                    .or(task.status_symbol)
                    .map(|symbol| style_task_status_marker(styler, symbol))
                    .unwrap_or_else(|| "?".to_string());
                format!("{marker} deferred")
            }
            (Some(symbol), _, _) | (_, Some(symbol), _) => {
                style_task_status_marker(styler, symbol)
            }
            _ => "?".to_string(),
        };
        let text = task.text.as_deref().unwrap_or("");
        let locator = match &task.relative_target {
            Some(target) => format!("{target} ^{}", task.block_id),
            None => format!("^{}", task.block_id),
        };
        let mut line = if text.is_empty() {
            format!("{transition} {locator}")
        } else {
            format!("{transition} {text} {locator}")
        };
        // Parked rows keep their real transition and log counts with a
        // concise caption; color is never the only signal.
        if let Some(index) = task.index {
            let (outcome, source) = outcome_of(index);
            if outcome == "parked" && source == "listed" {
                line.push_str(" · Parked · not carried");
            }
        }
        if task.work_log_created {
            let count = task.work_log.len();
            if count > 0 {
                line.push_str(&format!(" +{count} Work Log"));
            }
        }
        if let Some(clock) = task
            .index
            .and_then(|index| {
                close.task_links.iter().find(|link| link.index == index)
            })
            .and_then(|link| link.clock.as_ref())
        {
            let caption = match clock.class {
                "ended_inside" => {
                    format!("ended {}", format_event_hhmm(&clock.span[5..9]))
                }
                "running" => format!(
                    "running until {}",
                    format_event_hhmm(&clock.span[5..9])
                ),
                "not_started" => {
                    format!("starts {}", format_event_hhmm(&clock.span[..4]))
                }
                "ended_before" => format!(
                    "ended {} · before this session",
                    format_event_hhmm(&clock.span[5..9])
                ),
                _ => String::new(),
            };
            if !caption.is_empty() {
                line.push_str(&styler.dim(&format!(" · {caption}")));
            }
        }
        println!("{prefix}{line}");
        // Typed entries print first, all of them, not dimmed, each
        // followed by its details indented two more spaces and not
        // dimmed; up to two other entries follow, dimmed as before.
        for line in typed_work_log_lines(task, &log_indent) {
            println!("{line}");
        }
        let mut unprinted = task.typed_work_log.clone();
        let mut others = Vec::new();
        for entry in &task.work_log {
            if let Some(position) =
                unprinted.iter().position(|typed| typed == entry)
            {
                unprinted.remove(position);
            } else {
                others.push(entry);
            }
        }
        for entry in others.iter().take(2) {
            println!("{log_indent}{}", styler.dim(entry));
        }
    }
    print_human_successor_rows(
        &close.unblocked,
        &close.still_blocked,
        RelationHumanOutput {
            rows: &close.relation_successors,
            groups: &close.relation_groups,
            held: &close.relation_held,
            check: close.relation_check,
        },
        result.dry_run,
        styler,
    );
    print_human_keep_moving_rows(
        &close.keep_moving,
        &close.keep_moving_folds,
        result.dry_run,
        styler,
    );
    let mut parked: Vec<u32> = close
        .task_links
        .iter()
        .filter(|link| link.outcome == "parked" && link.source == "listed")
        .map(|link| link.index)
        .collect();
    parked.sort_unstable();
    parked.dedup();
    if !parked.is_empty() {
        let numbers = parked
            .iter()
            .map(u32::to_string)
            .collect::<Vec<_>>()
            .join(", ");
        println!("  {}", styler.dim(&format!("Parked {numbers}")));
    }
    let mut dropped: Vec<u32> = close
        .task_links
        .iter()
        .filter(|link| link.outcome == "dropped")
        .map(|link| link.index)
        .collect();
    dropped.sort_unstable();
    if !dropped.is_empty() {
        let numbers = dropped
            .iter()
            .map(u32::to_string)
            .collect::<Vec<_>>()
            .join(", ");
        println!("  {}", styler.dim(&format!("Dropped {numbers}")));
    }
    if let Some(next) = close.next_pomodoro.as_ref() {
        let next_name = next.name.as_deref().unwrap_or("session");
        let created_text = if next.created { " (created)" } else { "" };
        let carries = close.carried.len();
        let carries_text = if next.created {
            let noun = if carries == 1 { "link" } else { "links" };
            format!(" · carries {carries} {noun}")
        } else {
            String::new()
        };
        let placed_last_text = if next.created && next.placed_last {
            " · placed last"
        } else {
            ""
        };
        let shed_text = if next.shed.is_empty() {
            String::new()
        } else {
            let shed = next
                .shed
                .iter()
                .map(|theme| theme.name.as_str())
                .collect::<Vec<_>>()
                .join(" + ");
            format!(" · shed {shed}")
        };
        let unblocked_text = if close.unblocked.is_empty() {
            String::new()
        } else {
            format!(" · {} unblocked", close.unblocked.len())
        };
        let pulled = pulled_in_phrase(
            &close.unblocked,
            &close.keep_moving,
            &close.relation_successors,
        );
        println!(
            "  {}",
            styler.dim(&format!(
                "next: {next_name}{created_text} at line {}{carries_text}{placed_last_text}{shed_text}{unblocked_text}{pulled}",
                next.line
            ))
        );
    }
}

/// Segment clause for the past human header, shared by dry-run and real
/// runs: `at the start/end of <label>` for numbered segments, `since NAME`
/// for the tail, and `before NAME` for the head. `None` when the member
/// carries no suffix (unpinned runs and non-carrier members of pinned runs).
fn past_segment_clause(past: &PomodoroPastSummaryJson) -> Option<String> {
    let segment = past.segment.as_ref()?;
    match segment.kind.as_str() {
        "pomodoro" => {
            let index = segment.index.unwrap_or(0);
            let name = segment.name.as_deref().unwrap_or("session");
            let edge = if segment.edge == "end" {
                "end"
            } else {
                "start"
            };
            Some(format!(
                "at the {edge} of ~{index} {name} {}-{}",
                segment.start, segment.end
            ))
        }
        "gap" => {
            let index = segment.index.unwrap_or(0);
            let edge = if segment.edge == "end" {
                "end"
            } else {
                "start"
            };
            Some(format!(
                "at the {edge} of ~{index} gap {}-{}",
                segment.start, segment.end
            ))
        }
        "tail" => {
            let name = segment.name.as_deref().unwrap_or("session");
            Some(format!("since {name}"))
        }
        "head" => {
            let name = segment.name.as_deref().unwrap_or("session");
            Some(format!("before {name}"))
        }
        _ => None,
    }
}

pub(super) fn print_human_pomodoro_past_success(
    result: &CaptureItemResult,
    past: &PomodoroPastSummaryJson,
    styler: &Styler,
    prefix: &str,
    ordinal: &str,
) {
    let close = &*past.close;
    let verb = if result.dry_run {
        "would record"
    } else {
        "recorded"
    };
    let name = past
        .name
        .as_deref()
        .or(close.closed_name.as_deref())
        .or(close.pomodoro_name.as_deref())
        .unwrap_or("session");
    let narrowed_from = match (&close.closed_name, &close.pomodoro_name) {
        (Some(closed), Some(running)) if closed != running => {
            format!(" · from {running}")
        }
        _ => String::new(),
    };
    let off_clock = crate::native::pomodoro_homes::off_clock_via_label(
        close.off_clock,
        close.clock_note.as_deref(),
    );
    let range_text = if let Some(cut) = past.window.cut.as_deref() {
        let requested =
            past.window.requested_minutes.unwrap_or(past.window.minutes);
        let cut_phrase = match cut {
            "now" => "at now".to_string(),
            "wall" => past
                .window
                .wall
                .as_deref()
                .map(|wall| format!("at {wall}"))
                .unwrap_or_else(|| "at wall".to_string()),
            "segment" => past
                .segment
                .as_ref()
                .map(|segment| segment.raw.clone())
                .or_else(|| past.window.fit.clone())
                .map(|raw| format!("to fit {raw}"))
                .unwrap_or_else(|| "to fit segment".to_string()),
            _ => format!("at {cut}"),
        };
        format!(
            "{}-{} ({}m{}, {requested}m cut {cut_phrase})",
            past.window.start, past.window.end, past.window.minutes, off_clock
        )
    } else {
        format!(
            "{}-{} ({}m{})",
            past.window.start, past.window.end, past.window.minutes, off_clock
        )
    };
    let mut header = format!(
        "{verb} {name} {range_text}{narrowed_from} · {} line {}",
        close.day_relative, past.line
    );
    if let Some(clause) = past_segment_clause(past) {
        header.push_str(&format!(" · {clause}"));
    }
    if past.stack.size > 1 {
        header.push_str(&format!(
            " · {} of {}",
            past.stack.index, past.stack.size
        ));
    }
    // Records never overrun and never decrement by construction.
    println!("{prefix} {ordinal}{header}");
    print_close_body(result, close, styler);
}

fn format_event_hhmm(value: &str) -> String {
    if value.len() == 4 {
        format!("{}:{}", &value[..2], &value[2..])
    } else {
        value.to_string()
    }
}

pub(super) fn print_human_pomodoro_adjust_success(
    result: &CaptureItemResult,
    adjust: &PomodoroAdjustSummary,
    styler: &Styler,
    prefix: &str,
    ordinal: &str,
    target_label: &str,
) {
    let verb = if result.dry_run {
        "would adjust"
    } else {
        "adjusted"
    };
    println!("{prefix} {verb}  {ordinal}{target_label}");
    let name = adjust.pomodoro_name.as_deref().unwrap_or("current session");
    let sign = if adjust.delta_minutes >= 0 { "+" } else { "-" };
    let effect = adjust.delta_minutes.abs();
    let requested_sign = if adjust.direction == "plus" { "+" } else { "-" };
    let mut detail = format!(
        "{} {}-{} ({}m) to {}-{} ({}m), {}{}m at line {}",
        name,
        adjust.before_start,
        adjust.before_end,
        adjust.before_duration_minutes,
        adjust.after_start,
        adjust.after_end,
        adjust.after_duration_minutes,
        sign,
        effect,
        adjust.pomodoro_line,
    );
    if adjust.clamped {
        detail.push_str(&format!(
            " (requested {}{}m in {} units clamped)",
            requested_sign,
            adjust.requested_minutes.abs(),
            adjust.requested_units,
        ));
    }
    println!("  {}", styler.dim(&detail));
    if let Some(conflict) = adjust.event_conflict.as_ref() {
        println!(
            "  {}",
            styler.yellow(&event_conflict_message(
                adjust.pomodoro_name.as_deref(),
                &adjust.after_start,
                &adjust.after_end,
                conflict,
            ))
        );
    }
    println!("  {}", styler.dim(&result.task_line));
}

pub(super) fn print_human_pomodoro_shift_success(
    result: &CaptureItemResult,
    shift: &PomodoroShiftSummary,
    styler: &Styler,
    prefix: &str,
    ordinal: &str,
    target_label: &str,
) {
    let verb = if result.dry_run {
        "would shift"
    } else {
        "shifted"
    };
    println!("{prefix} {verb}  {ordinal}{target_label}");
    let name = shift.pomodoro_name.as_deref().unwrap_or("current session");
    let direction_word = if shift.direction == "later" {
        "later"
    } else {
        "earlier"
    };
    let detail = format!(
        "{} {}-{} to {}-{} ({}m), {}m {} at line {}",
        name,
        shift.before_start,
        shift.before_end,
        shift.after_start,
        shift.after_end,
        shift.duration_minutes,
        shift.delta_minutes.abs(),
        direction_word,
        shift.pomodoro_line,
    );
    println!("  {}", styler.dim(&detail));
    if let Some(conflict) = shift.event_conflict.as_ref() {
        println!(
            "  {}",
            styler.yellow(&event_conflict_message(
                shift.pomodoro_name.as_deref(),
                &shift.after_start,
                &shift.after_end,
                conflict,
            ))
        );
    }
    println!("  {}", styler.dim(&result.task_line));
}

pub(super) fn print_human_pomodoro_split_success(
    result: &CaptureItemResult,
    split: &PomodoroSplitSummary,
    styler: &Styler,
    prefix: &str,
    ordinal: &str,
    target_label: &str,
) {
    let verb = if result.dry_run {
        "would split"
    } else {
        "split"
    };
    println!("{prefix} {verb}  {ordinal}{target_label}");
    let detail = format!(
        "{} \u{2192} {} \u{00B7} {} line {}",
        split.before, split.after, split.path, split.entry_line,
    );
    println!("  {}", styler.dim(&detail));
    for created in &split.created {
        let links = if created.moved_links == 0 {
            "no links".to_string()
        } else if created.moved_links == 1 {
            "1 link".to_string()
        } else {
            format!("{} links", created.moved_links)
        };
        println!(
            "  {}",
            styler.dim(&format!(
                "\u{2193} {}   new Pomodoro \u{00B7} {}",
                created.name, links,
            )),
        );
    }
    println!("  {}", styler.dim(&result.task_line));
}

pub(super) fn print_human_pomodoro_merge_success(
    result: &CaptureItemResult,
    merge: &PomodoroMergeSummary,
    styler: &Styler,
    prefix: &str,
    ordinal: &str,
    target_label: &str,
) {
    let verb = if result.dry_run {
        "would merge"
    } else {
        "merged"
    };
    println!("{prefix} {verb}  {ordinal}{target_label}");
    let absorbed = merge
        .absorbed
        .iter()
        .map(|item| item.name.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    let links = if merge.moved_links == 0 {
        "no links moved".to_string()
    } else if merge.moved_links == 1 {
        "1 link moved".to_string()
    } else {
        format!("{} links moved", merge.moved_links)
    };
    let detail = format!(
        "{absorbed} \u{2192} {} \u{00B7} {} line {} \u{00B7} {links}",
        merge.after, merge.path, merge.entry_line,
    );
    println!("  {}", styler.dim(&detail));
    let mut implied = Vec::new();
    if matches!(
        merge.source_role,
        PomodoroMergeSourceRole::Current | PomodoroMergeSourceRole::NextUp
    ) {
        implied.push(format!("current {}", merge.before));
    }
    implied.extend(
        merge
            .absorbed
            .iter()
            .filter(|item| item.role == PomodoroMergeAbsorbedRole::Next)
            .map(|item| format!("next {}", item.name)),
    );
    if !implied.is_empty() {
        let line = match merge.explicit.as_deref() {
            Some(token) => format!("same as {token} · {}", implied.join(", ")),
            None => implied.join(", "),
        };
        println!("  {}", styler.dim(&line));
    }
    println!("  {}", styler.dim(&result.task_line));
}

/// Link-guard trailer (§13.5): one plain line naming why the target
/// stayed unlinked and which prerequisites rose instead. Called by
/// every printer for a gesture that can suppress a link.
pub(super) fn print_human_link_guard(
    result: &CaptureItemResult,
    styler: &Styler,
) {
    let Some(guard) = result.link_guard.as_ref() else {
        return;
    };
    if let Some(detail) = guard.detail.as_deref() {
        println!("  {}", styler.dim(&format!("⛔ {detail}")));
    }
}

/// Summary for the `@route+block-id!` link-presence toggle:
/// `linked ^id under NAME · set Next`, `linked ^id under NAME · stays In
/// Progress`, or `unlinked ^id from N open Pomodoros · stays Next`.
fn print_human_link_toggle_summary(
    result: &CaptureItemResult,
    styler: &Styler,
) {
    let id = result
        .block_id
        .as_deref()
        .map(|id| format!("^{id}"))
        .unwrap_or_else(|| "^?".to_string());
    let id = styler.cyan(&id);
    if result.toggle_direction == Some("unlink") {
        let removed = result.removed_pomodoro_links.unwrap_or(0);
        let source = if removed == 1 {
            "1 open Pomodoro".to_string()
        } else {
            format!("{removed} open Pomodoros")
        };
        let status = result.status_name.as_deref().unwrap_or(
            match result.status_symbol {
                Some('/') => "In Progress",
                Some('*') => "Next",
                Some('?') => "Blocked",
                _ => "Ready",
            },
        );
        println!("  unlinked {id} from {source} · stays {status}");
        return;
    }
    if result.link_guard.is_some() {
        let status = result.status_name.as_deref().unwrap_or(
            match result.status_symbol {
                Some('/') => "In Progress",
                Some('*') => "Next",
                Some('?') => "Blocked",
                _ => "Ready",
            },
        );
        println!("  not linked {id} · stays {status}");
        print_human_link_guard(result, styler);
        return;
    }
    let destination = result
        .pomodoro_name
        .as_deref()
        .map(|name| format!("under {name}"))
        .unwrap_or_else(|| "under current/next Pomodoro".to_string());
    let verdict = match (result.previous_status_symbol, result.status_symbol) {
        (Some(previous), Some(current)) if previous != current => "set Next",
        (_, Some('/')) => "stays In Progress",
        _ => "stays Next",
    };
    println!("  linked {id} {destination} · {verdict}");
}

/// Human rendering for a dependency-only capture: whose prerequisites
/// changed, never a task creation or Pomodoro link.
pub(super) fn print_human_task_dependency_success(
    result: &CaptureItemResult,
    styler: &Styler,
    prefix: &str,
    ordinal: &str,
    target_label: &str,
) {
    let verb = if result.dry_run {
        "would update"
    } else {
        "updated"
    };
    println!("{prefix} {verb}  {ordinal}{target_label}");
    print_human_owner_status_row(result, styler);
    print_human_dependency_updates(result, styler);
    print_human_relation_updates(result, styler);
}

fn print_human_owner_status_row(result: &CaptureItemResult, styler: &Styler) {
    let previous = result
        .previous_status_symbol
        .map(|symbol| style_task_status_marker(styler, symbol))
        .unwrap_or_else(|| styler.dim("[?]"));
    let next = result
        .status_symbol
        .map(|symbol| style_task_status_marker(styler, symbol))
        .unwrap_or_else(|| styler.dim("[?]"));
    let description = if result.text.is_empty() {
        result.task_line.as_str()
    } else {
        result.text.as_str()
    };
    let block_id = result
        .block_id
        .as_deref()
        .map(|id| format!("  {}", styler.cyan(&format!("^{id}"))))
        .unwrap_or_default();
    let changed = result.status_changed.unwrap_or(false)
        || result.previous_status_symbol != result.status_symbol;
    if changed
        && result.previous_status_symbol.is_some()
        && result.status_symbol.is_some()
        && result.previous_status_symbol != result.status_symbol
    {
        println!("  {previous} → {next}  {description}{block_id}");
        return;
    }
    let status =
        result
            .status_name
            .as_deref()
            .unwrap_or(match result.status_symbol {
                Some('/') => "In Progress",
                Some('*') => "Next",
                Some('?') => "Blocked",
                Some('x') | Some('X') => "Done",
                Some('-') => "Canceled",
                _ => "Ready",
            });
    println!("  {next} stays {status}  {description}{block_id}");
}

fn print_human_multi_note_notes(result: &CaptureItemResult, styler: &Styler) {
    // Mirrors the draft order, grouping consecutive notes that share one
    // drawer under a single drawer header. Plain notes print their
    // task line plus children; drawer notes print indented under their
    // header.
    let mut index = 0usize;
    while index < result.notes.len() {
        let note = &result.notes[index];
        if let Some(drawer) = note.drawer.as_ref() {
            let key = drawer.key.clone();
            let mut end = index + 1;
            while end < result.notes.len()
                && result.notes[end]
                    .drawer
                    .as_ref()
                    .is_some_and(|d| d.key == key)
            {
                end += 1;
            }
            let header_drawer = &result.notes[index]
                .drawer
                .as_ref()
                .expect("drawer note")
                .clone();
            // `new drawer` tag on first touch; subsequent groups in the
            // same drawer (same item, later in draft order) omit it
            // because their `created` is false.
            let created_tag = if header_drawer.created {
                "        new drawer"
            } else {
                ""
            };
            println!("  {}{created_tag}", styler.cyan(&header_drawer.title));
            for grouped in &result.notes[index..end] {
                println!("    {}", styler.dim(&grouped.task_line));
                for child in &grouped.sub_bullets {
                    println!("      {}", styler.dim(child.trim_start()));
                }
            }
            index = end;
        } else {
            println!("  {}", styler.dim(&note.task_line));
            for child in &note.sub_bullets {
                println!("    {}", styler.dim(child.trim_start()));
            }
            index += 1;
        }
    }
}

fn parent_under_marker(result: &CaptureItemResult, styler: &Styler) -> String {
    let Some(current) = result.parent_status_symbol else {
        return String::new();
    };
    let current_marker = style_task_status_marker(styler, current);
    let updates: Vec<&DependencyUpdateJson> =
        if !result.dependency_updates.is_empty() {
            result.dependency_updates.iter().collect()
        } else {
            result.dependency_update.iter().collect()
        };
    let previous = updates
        .iter()
        .find_map(|update| parent_previous_status(result, update, current));
    if let Some(previous) = previous {
        let previous_marker = style_task_status_marker(styler, previous);
        format!("{previous_marker} → {current_marker} ")
    } else {
        format!("{current_marker} ")
    }
}

/// Pre-capture parent status from whichever relationship changed it:
/// the parent as `&` dependent, or as a `*` prerequisite that
/// commitment transfer raised.
fn parent_previous_status(
    result: &CaptureItemResult,
    update: &DependencyUpdateJson,
    current: char,
) -> Option<char> {
    let parent_id = result.block_id.as_deref()?;
    if update.role == DependencyUpdateRole::Dependent
        && update.dependent_block_id.as_deref() == Some(parent_id)
        && update.previous_status != current
    {
        return Some(update.previous_status);
    }
    update.prerequisites.iter().find_map(|prereq| {
        let previous = prereq.previous_status?;
        (prereq.block_id == parent_id
            && prereq.note == result.relative_target
            && previous != current)
            .then_some(previous)
    })
}

fn print_human_relation_updates(result: &CaptureItemResult, styler: &Styler) {
    for update in &result.relation_updates {
        println!("  {}", human_relation_line(result.dry_run, update));
    }
    // One bob-authored ⊘ row per cancel-label source (§17 CL6); the
    // matching `relation_cancelled_source` warning stays JSON-only so
    // the human output never repeats it.
    for cancel in &result.relation_cancels {
        println!("  {}", cancel.message);
    }
    // One bob-authored ✓ row per complete-label source (§18 CP6); the
    // matching `relation_completed_source` warning stays JSON-only so
    // the human output never repeats it. Completed sources then print
    // the shared `!` detail rows.
    for complete in &result.relation_completes {
        println!("  {}", complete.message);
        if complete.effect == RelationCompleteEffect::Completed {
            print_task_complete_details(
                &complete.task_complete,
                result.dry_run,
                styler,
            );
        }
    }
    for hide in &result.relation_label_hides {
        if hide.action != RelationLabelHideAction::Hide {
            continue;
        }
        let links = if hide.existing_count == 1 {
            "link"
        } else {
            "links"
        };
        println!(
            "  🪢 hides \"{}\" from suggestions ({} {links})",
            hide.label, hide.existing_count
        );
    }
}

fn human_relation_line(dry_run: bool, update: &RelationUpdateJson) -> String {
    let label = update
        .label
        .as_deref()
        .map(|label| format!(" · {label}"))
        .unwrap_or_default();
    let offer = update
        .offer_effect
        .map(|_| " · 🪢 offers source")
        .unwrap_or_default();
    let archived = if update.target.archived == Some(true) {
        " · archived"
    } else {
        ""
    };
    let locator = format!(
        "   {} ^{}{archived}",
        update.target.path, update.target.block_id
    );
    let quoted = format!("\"{}\"", update.target.text);

    match (update.direction, update.action, dry_run) {
        (RelationDirection::To, RelationAction::Added, false) => {
            format!("🪢 links to {quoted}{label}{offer}{locator}")
        }
        (RelationDirection::To, RelationAction::Added, true) => {
            format!("🪢 would link to {quoted}{label}{offer}{locator}")
        }
        (RelationDirection::From, RelationAction::Added, false) => {
            format!(
                "🪢 {quoted} now links here{label}{offer}   {} ^{}",
                update.source.path, update.source.block_id
            )
        }
        (RelationDirection::From, RelationAction::Added, true) => {
            format!(
                "🪢 {quoted} would now link here{label}{offer}   {} ^{}",
                update.source.path, update.source.block_id
            )
        }
        (_, RelationAction::Relabeled, false) => {
            format!("🪢 relabeled {quoted}{label}{offer}{locator}")
        }
        (_, RelationAction::Relabeled, true) => {
            format!("🪢 would relabel {quoted}{label}{offer}{locator}")
        }
        (_, RelationAction::Unchanged, _) => {
            format!("🪢 unchanged {quoted}{locator}")
        }
    }
}

fn print_human_dependency_updates(result: &CaptureItemResult, styler: &Styler) {
    let updates: Vec<&DependencyUpdateJson> =
        if !result.dependency_updates.is_empty() {
            result.dependency_updates.iter().collect()
        } else {
            result.dependency_update.iter().collect()
        };
    for update in updates {
        match update.role {
            DependencyUpdateRole::Dependent => {
                print_human_dependency_line(result, update, styler);
            }
            DependencyUpdateRole::BlockedTask => {
                print_human_blocked_task_line(result, update, styler);
            }
        }
    }
}

fn print_human_blocked_task_line(
    result: &CaptureItemResult,
    update: &DependencyUpdateJson,
    styler: &Styler,
) {
    let existing_owner =
        result.kind == "task_dependency" || result.kind == "sub_bullet";
    let already = update.added == 0 && update.already_present > 0;
    let verb = if already {
        "already blocks"
    } else {
        match (result.dry_run, existing_owner) {
            (true, true) => "would now block",
            (false, true) => "now blocks",
            (true, false) => "would block",
            (false, false) => "blocks",
        }
    };
    let block_id = update
        .dependent_block_id
        .as_deref()
        .map(|id| format!(" ^{id}"))
        .unwrap_or_default();
    let mut line = format!(
        "  🧱 {verb} \"{}\"  {}{block_id}",
        update.dependent_text, update.dependent_note,
    );
    if !already {
        let next = style_task_status_marker(styler, update.dependent_status);
        if update.previous_status != update.dependent_status {
            let previous =
                style_task_status_marker(styler, update.previous_status);
            line.push_str(&format!(
                "  {previous} → {next} {}",
                update.dependent_status_name
            ));
        } else {
            line.push_str(&format!(
                "  {next} {}",
                update.dependent_status_name
            ));
        }
    }
    if !already && update.blocked_residents > 0 {
        let noun = if update.blocked_residents == 1 {
            "task"
        } else {
            "tasks"
        };
        let resident_verb = if result.dry_run {
            "would block"
        } else {
            "blocked"
        };
        line.push_str(&format!(
            " · {resident_verb} {} project {noun}",
            update.blocked_residents
        ));
    }
    println!("{line}");
}

/// One row per prerequisite for every capture carrying a
/// `dependency_update`.
pub(super) fn print_human_dependency_line(
    result: &CaptureItemResult,
    update: &DependencyUpdateJson,
    styler: &Styler,
) {
    let existing_owner =
        result.kind == "task_dependency" || result.kind == "sub_bullet";
    let update_already = update.added == 0 && update.already_present > 0;
    for prereq in &update.prerequisites {
        let already = prereq.already_present || update_already;
        let verb = if already {
            "already depends on"
        } else {
            match (result.dry_run, existing_owner) {
                (true, true) => "would now depend on",
                (false, true) => "now depends on",
                (true, false) => "would depend on",
                (false, false) => "depends on",
            }
        };
        let mut line = format!(
            "  ⛓ {verb} \"{}\"  {} ^{}",
            prereq.text, prereq.note, prereq.block_id,
        );
        let next = style_task_status_marker(styler, prereq.status_symbol);
        if let (Some(previous), Some(_)) =
            (prereq.previous_status, prereq.previous_status_name.as_ref())
            && previous != prereq.status_symbol
        {
            let previous_marker = style_task_status_marker(styler, previous);
            line.push_str(&format!(
                "  {previous_marker} → {next} {}",
                prereq.status_name
            ));
        } else {
            line.push_str(&format!("  {next} {}", prereq.status_name));
        }
        println!("{line}");
    }
}

pub(super) fn print_human_task_toggle_success(
    result: &CaptureItemResult,
    styler: &Styler,
    prefix: &str,
    ordinal: &str,
    target_label: &str,
) {
    let ensure_next = result.toggle_behavior == Some("ensure_next");
    let verb = match (result.dry_run, ensure_next) {
        (true, true) => "would ensure",
        (false, true) => "ensured",
        (true, false) => "would toggle",
        (false, false) => "toggled",
    };
    println!("{prefix} {verb}  {ordinal}{target_label}");

    let previous_marker = result
        .previous_status_symbol
        .map(|symbol| style_task_status_marker(styler, symbol))
        .unwrap_or_else(|| styler.dim("[?]"));
    let next_marker = result
        .status_symbol
        .map(|symbol| style_task_status_marker(styler, symbol))
        .unwrap_or_else(|| styler.dim("[?]"));
    let description = result
        .toggle_task_description
        .as_deref()
        .unwrap_or(result.task_line.as_str());
    let block_id = result
        .block_id
        .as_deref()
        .map(|id| format!("  {}", styler.cyan(&format!("^{id}"))))
        .unwrap_or_default();
    if !ensure_next {
        print_human_link_toggle_summary(result, styler);
    } else if result.link_guard.is_some() {
        let status = result.status_name.as_deref().unwrap_or("Blocked");
        println!("  {next_marker} stays {status}  {description}{block_id}");
    } else if result.status_changed == Some(false)
        && result.status_symbol == Some('/')
    {
        println!("  {next_marker} stays In Progress  {description}{block_id}");
    } else if result.status_changed == Some(false) {
        println!("  {next_marker} already Next  {description}{block_id}");
    } else {
        println!(
            "  {previous_marker} → {next_marker}  {description}{block_id}"
        );
    }

    print_human_dropped_dependencies(result, styler);

    let mut chips = Vec::new();
    if result.removed_scheduled.is_some() {
        chips.push("removed future schedule".to_string());
    }
    if result.schedule_log.is_some() {
        chips.push("logged schedule change".to_string());
    }
    if !ensure_next && result.pomodoro_already_linked == Some(true) {
        chips.push("already linked".to_string());
    }
    if result.pomodoro_selector_unused == Some(true) {
        let selector = result
            .pomodoro_name
            .as_deref()
            .map(|name| format!("#{name}"))
            .unwrap_or_else(|| "#name".to_string());
        chips.push(format!("{selector} not used when clearing"));
    }
    if !chips.is_empty() {
        println!("  {}", styler.dim(&chips.join(" · ")));
    }

    if ensure_next {
        print_human_ensure_next_ledger(result, styler);
        return;
    }

    if let Some(day_file) = result.day_file.as_deref() {
        let under = result
            .pomodoro_name
            .as_deref()
            .filter(|_| result.toggle_direction == Some("link"))
            .map(|name| {
                let suffix = result
                    .pomodoro_link_destination
                    .as_ref()
                    .map_or_else(String::new, |destination| {
                        format!(
                            "{}{}",
                            format_placed_type_suffix(destination),
                            format_merged_suffix(destination)
                        )
                    });
                format!(" · under {}{}", styler.cyan(name), styler.dim(&suffix))
            })
            .unwrap_or_default();
        println!("  {}{under}", styler.cyan(day_file));
    }

    match result.toggle_direction {
        Some("link") => {
            if result.link_guard.is_none()
                && let Some(block_link) = result.block_link.as_deref()
            {
                let marker = styler.green("+");
                println!("  {marker} {block_link}");
            }
            if result.consumed_links.is_empty()
                && result.removed_pomodoro_links.unwrap_or(0) > 0
            {
                print_removed_pomodoro_links(
                    styler,
                    result.removed_pomodoro_links.unwrap_or(0),
                    "later ",
                );
            }
        }
        Some("unlink") => {
            print_removed_pomodoro_links(
                styler,
                result.removed_pomodoro_links.unwrap_or(0),
                "",
            );
        }
        _ => {}
    }
    print_human_successor_rows(
        &[],
        &[],
        RelationHumanOutput {
            rows: &result.relation_successors,
            groups: &result.relation_groups,
            held: &result.relation_held,
            check: result.relation_check,
        },
        result.dry_run,
        styler,
    );
}

pub(super) fn print_human_ensure_next_ledger(
    result: &CaptureItemResult,
    styler: &Styler,
) {
    if result.link_guard.is_some() {
        print_human_link_guard(result, styler);
        return;
    }
    match result.pomodoro_link_action {
        Some("moved") => {
            if let Some(day_file) = result.day_file.as_deref() {
                let under = result
                    .pomodoro_name
                    .as_deref()
                    .map(|name| format!(" · under {}", styler.cyan(name)))
                    .unwrap_or_default();
                println!("  {}{under}", styler.cyan(day_file));
            }
            let source = result
                .pomodoro_link_source
                .as_ref()
                .map(format_pomodoro_endpoint)
                .unwrap_or_else(|| "source".to_string());
            let destination = result
                .pomodoro_link_destination
                .as_ref()
                .map(format_pomodoro_endpoint)
                .unwrap_or_else(|| "current/next".to_string());
            let suffix = result.pomodoro_link_destination.as_ref().map_or_else(
                String::new,
                |endpoint| {
                    format!(
                        "{}{}",
                        format_placed_type_suffix(endpoint),
                        format_merged_suffix(endpoint)
                    )
                },
            );
            println!(
                "  {} moved Task Link {} → {}{}",
                styler.green("↗"),
                styler.cyan(&source),
                styler.cyan(&destination),
                styler.dim(&suffix),
            );
            if result.creates_pomodoro == Some(true) {
                println!(
                    "  {} created {}",
                    styler.green("+"),
                    styler.cyan(&destination)
                );
            }
        }
        _ => {
            let already = result
                .pomodoro_name
                .as_deref()
                .or_else(|| {
                    result
                        .pomodoro_link_destination
                        .as_ref()
                        .and_then(|endpoint| endpoint.name.as_deref())
                })
                .map(|name| format!("Task Link already in {name}; no ledger change."))
                .unwrap_or_else(|| {
                    "Task Link already in current/next Pomodoro; no ledger change.".to_string()
                });
            println!("  {}", styler.dim(&already));
        }
    }
    if let Some(row) = format_task_theme_row(result, styler) {
        print!("{row}");
    }
    if let Some(row) = format_type_relocation_row(result) {
        print!("{row}");
    }
}

pub(super) fn format_pomodoro_endpoint(
    endpoint: &PomodoroLinkEndpoint,
) -> String {
    if let Some(name) = endpoint.name.as_deref() {
        name.to_string()
    } else if let Some(range) = endpoint.time_range.as_deref() {
        range.to_string()
    } else {
        format!("line {}", endpoint.line)
    }
}

pub(super) fn print_human_dropped_dependencies(
    result: &CaptureItemResult,
    styler: &Styler,
) {
    if result.dropped_dependencies.is_empty() {
        return;
    }
    let links = result
        .dropped_dependencies
        .iter()
        .map(|dependency| dependency.link.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    let action = if result.dry_run {
        "would drop"
    } else {
        "dropped"
    };
    let line = if result.dropped_dependencies.len() == 1 {
        format!("⛓️ {action} dependency on {links}")
    } else {
        format!(
            "⛓️ {action} {} dependencies: {links}",
            result.dropped_dependencies.len()
        )
    };
    println!("  {}", styler.dim(&line));
}

pub(super) fn print_human_pomodoro_link_success(
    result: &CaptureItemResult,
    styler: &Styler,
    prefix: &str,
    ordinal: &str,
    target_label: &str,
) {
    let has_start = result.pomodoro_start.is_some();
    let verb = match (result.dry_run, has_start) {
        (true, true) => "would start",
        (false, true) => "start",
        (true, false) => "would link",
        (false, false) => "link",
    };
    println!("{prefix} {verb}  {ordinal}{target_label}");
    print!("{}", format_human_pomodoro_link_details(result, styler));
}

/// Status transition, ledger line, and dependency-drop / schedule chips
/// for a Pomodoro link. Shared by `bob capture` and `bob ref today`.
pub(crate) fn format_human_pomodoro_link_details(
    result: &CaptureItemResult,
    styler: &Styler,
) -> String {
    let mut out = String::new();
    let previous_marker = result
        .previous_status_symbol
        .map(|symbol| style_task_status_marker(styler, symbol))
        .unwrap_or_else(|| styler.dim("[?]"));
    let next_marker = result
        .status_symbol
        .map(|symbol| style_task_status_marker(styler, symbol))
        .unwrap_or_else(|| styler.dim("[?]"));
    let description = result
        .toggle_task_description
        .as_deref()
        .unwrap_or(result.task_line.as_str());
    let block_id = result
        .block_id
        .as_deref()
        .map(|id| format!("  {}", styler.cyan(&format!("^{id}"))))
        .unwrap_or_default();
    let previous_symbol = result.previous_status_symbol.unwrap_or(' ');
    let current_symbol = result.status_symbol.unwrap_or(' ');
    if previous_symbol == '*' && current_symbol == '*' {
        out.push_str(&format!(
            "  {next_marker} already Next  {description}{block_id}\n"
        ));
    } else if previous_symbol == '/' && current_symbol == '/' {
        out.push_str(&format!(
            "  {next_marker} stays In Progress  {description}{block_id}\n"
        ));
    } else {
        out.push_str(&format!(
            "  {previous_marker} → {next_marker}  {description}{block_id}\n"
        ));
    }

    if !result.dropped_dependencies.is_empty() {
        let links = result
            .dropped_dependencies
            .iter()
            .map(|dependency| dependency.link.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        let action = if result.dry_run {
            "would drop"
        } else {
            "dropped"
        };
        let line = if result.dropped_dependencies.len() == 1 {
            format!("⛓️ {action} dependency on {links}")
        } else {
            format!(
                "⛓️ {action} {} dependencies: {links}",
                result.dropped_dependencies.len()
            )
        };
        out.push_str(&format!("  {}\n", styler.dim(&line)));
    }

    let mut chips = Vec::new();
    if result.removed_scheduled.is_some() {
        chips.push("removed future schedule".to_string());
    }
    if result.schedule_log.is_some() {
        chips.push("logged schedule change".to_string());
    }
    if !chips.is_empty() {
        out.push_str(&format!("  {}\n", styler.dim(&chips.join(" · "))));
    }

    if let Some(guard) = result.link_guard.as_ref() {
        if let Some(detail) = guard.detail.as_deref() {
            out.push_str(&format!(
                "  {}\n",
                styler.dim(&format!("⛔ {detail}"))
            ));
        }
    } else {
        let placed_suffix = result
            .pomodoro_link_destination
            .as_ref()
            .map(format_placed_type_suffix)
            .unwrap_or_default();
        let merged_suffix = result
            .pomodoro_link_destination
            .as_ref()
            .map(format_merged_suffix)
            .unwrap_or_default();
        let suffix = format!("{placed_suffix}{merged_suffix}");
        match result.pomodoro_link_action {
            Some("moved") => {
                let source = result
                    .pomodoro_link_source
                    .as_ref()
                    .map(format_pomodoro_endpoint)
                    .unwrap_or_else(|| "source".to_string());
                let destination = result
                    .pomodoro_link_destination
                    .as_ref()
                    .map(format_pomodoro_endpoint)
                    .unwrap_or_else(|| "current/next".to_string());
                let created = if result.creates_pomodoro == Some(true) {
                    format!(" (created {destination})")
                } else {
                    String::new()
                };
                out.push_str(&format!(
                    "  {}\n",
                    styler.dim(&format!(
                        "Moved Task Link {source} → \
                        {destination}{suffix}{created}"
                    ))
                ));
            }
            Some("linked") => {
                let destination = result
                    .pomodoro_link_destination
                    .as_ref()
                    .map(format_pomodoro_endpoint)
                    .unwrap_or_else(|| "current/next".to_string());
                out.push_str(&format!(
                    "  {}\n",
                    styler.dim(&format!("Linked under {destination}{suffix}"))
                ));
                if result.creates_pomodoro == Some(true) {
                    out.push_str(&format!(
                        "  {}\n",
                        styler.dim(&format!("created {destination}"))
                    ));
                }
            }
            _ => {
                let already = result
                    .pomodoro_name
                    .as_deref()
                    .or_else(|| {
                        result
                            .pomodoro_link_destination
                            .as_ref()
                            .and_then(|endpoint| endpoint.name.as_deref())
                    })
                    .map(|name| {
                        format!(
                            "Task Link already in {name}; no ledger change."
                        )
                    })
                    .unwrap_or_else(|| {
                        "Task Link already in current/next Pomodoro; no ledger change."
                            .to_string()
                    });
                out.push_str(&format!("  {}\n", styler.dim(&already)));
            }
        }
    }

    if let Some(row) = format_task_theme_row(result, styler) {
        out.push_str(&row);
    }
    if let Some(row) = format_type_relocation_row(result) {
        out.push_str(&row);
    }
    if let Some(start) = result.pomodoro_start.as_ref() {
        let name = start.pomodoro_name.as_deref().unwrap_or("next session");
        let created = if start.created_pomodoro {
            " (created)"
        } else {
            ""
        };
        let verb = if result.dry_run {
            "would start"
        } else {
            "started"
        };
        out.push_str(&format!(
            "  {}\n",
            styler.dim(&format!(
                "{verb} {name} {}-{} ({}m){created} at line {}",
                start.start,
                start.end,
                start.duration_minutes,
                start.pomodoro_line,
            ))
        ));
    }
    out
}

fn format_type_relocation_row(result: &CaptureItemResult) -> Option<String> {
    let relocation = result.type_relocation.as_ref()?;
    let row = match relocation.outcome {
        "rename_entry" => {
            let from = relocation.from.as_deref().unwrap_or("entry");
            format!("{} → {} · running entry renamed", from, relocation.to)
        }
        "append_component" => {
            let from = relocation.from.as_deref().unwrap_or("entry");
            format!("{} → {} · running entry shared", from, relocation.to)
        }
        "move_to" => format!("moved under {}", relocation.to),
        "rename_placeholder" => {
            let from = relocation.from.as_deref().unwrap_or("entry");
            format!("{} → {} · planned entry renamed", from, relocation.to)
        }
        "create_below" => format!("moved under new {}", relocation.to),
        _ => return None,
    };
    Some(format!("  ledger  {row}\n"))
}

fn format_task_theme_row(
    result: &CaptureItemResult,
    styler: &Styler,
) -> Option<String> {
    let change = result.task_theme.as_ref()?;
    let inherited = change
        .inherited
        .as_ref()
        .map(|suggestion| match suggestion.source {
            super::super::pomodoro_themes::suggest::SuggestionSource::Task => {
                format!("this task suggests {}", suggestion.theme.name)
            }
            super::super::pomodoro_themes::suggest::SuggestionSource::Reading => {
                format!("reading tasks suggest {}", suggestion.theme.name)
            }
            super::super::pomodoro_themes::suggest::SuggestionSource::Note => {
                let stem = suggestion
                    .label
                    .strip_prefix("from ")
                    .unwrap_or(&suggestion.label);
                format!("{stem} suggests {}", suggestion.theme.name)
            }
            super::super::pomodoro_themes::suggest::SuggestionSource::Default => {
                format!("default type suggests {}", suggestion.theme.name)
            }
        })
        .unwrap_or_else(|| "no default here".to_string());
    let row = match change.action {
        "set" => {
            let theme = change.theme.as_ref()?;
            let name = super::super::pomodoro_themes::swatch_name(
                std::slice::from_ref(theme),
                "",
                styler,
            )
            .unwrap_or_else(|| theme.name.clone());
            if result.dry_run {
                format!("would set type → {name} · {inherited}")
            } else {
                format!("→ {name} · {inherited}")
            }
        }
        "cleared" => {
            if result.dry_run {
                format!("would clear type · {inherited}")
            } else {
                format!("cleared · {inherited}")
            }
        }
        "kept" => {
            let theme = change.theme.as_ref()?;
            let name = super::super::pomodoro_themes::swatch_name(
                std::slice::from_ref(theme),
                "",
                styler,
            )
            .unwrap_or_else(|| theme.name.clone());
            // Short source label: `this task`, `from <stem>`,
            // `default type`, or `reading task`.
            let field_is_selected_type = change
                .previous
                .as_ref()
                .zip(change.theme.as_ref())
                .is_some_and(|(previous, selected)| {
                    previous.key == selected.key
                });
            let label = if field_is_selected_type {
                "this task".to_string()
            } else {
                change
                    .inherited
                    .as_ref()
                    .map(|suggestion| match suggestion.source {
                    super::super::pomodoro_themes::suggest::SuggestionSource::Task => {
                        "this task".to_string()
                    }
                    super::super::pomodoro_themes::suggest::SuggestionSource::Reading => {
                        "reading task".to_string()
                    }
                    super::super::pomodoro_themes::suggest::SuggestionSource::Note => {
                        suggestion.label.clone()
                    }
                    super::super::pomodoro_themes::suggest::SuggestionSource::Default => {
                        "default type".to_string()
                    }
                    })
                    .unwrap_or_else(|| "no default here".to_string())
            };
            format!("{name} · {label}")
        }
        _ => return None,
    };
    Some(format!("  type  {row}\n"))
}

pub(super) fn print_removed_pomodoro_links(
    styler: &Styler,
    count: usize,
    qualifier: &str,
) {
    let marker = styler.red("−");
    let plural = if count == 1 { "" } else { "s" };
    println!(
        "  {marker} removed {count} {qualifier}Pomodoro task link{plural}"
    );
}

pub(super) fn style_task_status_marker(
    styler: &Styler,
    symbol: char,
) -> String {
    let marker = format!("[{symbol}]");
    match symbol {
        '/' => styler.blue(&marker),
        '*' => styler.yellow(&marker),
        '?' => styler.red(&marker),
        'x' | 'X' => styler.green(&marker),
        _ => styler.dim(&marker),
    }
}

/// Human rendering for a whole-item `!` completion: the status
/// transition, closed subtasks, left-open descendants, ledger effects,
/// and unblocked dependents.
pub(super) fn print_human_task_complete_success(
    result: &CaptureItemResult,
    styler: &Styler,
    prefix: &str,
    ordinal: &str,
    target_label: &str,
) {
    let Some(summary) = result.task_complete.as_ref() else {
        let verb = if result.dry_run {
            "would complete"
        } else {
            "completed"
        };
        println!("{prefix} {verb}  {ordinal}{target_label}");
        return;
    };
    let block_label = styler.cyan(&format!("^{}", summary.block_id));
    if summary.action == "already_done" {
        let marker = result
            .status_symbol
            .map(|symbol| style_task_status_marker(styler, symbol))
            .unwrap_or_else(|| styler.dim("[x]"));
        let text = summary.text.as_str();
        let body = if text.is_empty() {
            marker.clone()
        } else {
            format!("{marker} {text}")
        };
        println!(
            "{prefix} already done {body}  {ordinal}{target_label} {block_label} · nothing to change"
        );
        return;
    }
    let verb = if result.dry_run {
        "would complete"
    } else {
        "completed"
    };
    let previous_marker = result
        .previous_status_symbol
        .map(|symbol| style_task_status_marker(styler, symbol))
        .unwrap_or_else(|| "?".to_string());
    let current_marker = result
        .status_symbol
        .map(|symbol| style_task_status_marker(styler, symbol))
        .unwrap_or_else(|| "?".to_string());
    let text = summary.text.as_str();
    let head = if text.is_empty() {
        format!("{previous_marker} → {current_marker}")
    } else {
        format!("{previous_marker} → {current_marker} {text}")
    };
    let pulled = pulled_in_phrase(
        &summary.unblocked,
        &summary.keep_moving,
        &summary.relation_successors,
    );
    println!(
        "{prefix} {verb} {head}  {ordinal}{target_label} {block_label}{pulled}"
    );
    print_task_complete_details(summary, result.dry_run, styler);
}

/// Relation rows, groups, and held metadata for one human successor
/// block. Empty slices keep existing unblocked-only callers unchanged.
pub(super) struct RelationHumanOutput<'a> {
    rows: &'a [TaskCompleteUnblockedJson],
    groups: &'a [crate::native::task_complete::RelationOfferGroup],
    held: &'a [RelationHeldJson],
    check: Option<&'static str>,
}

impl RelationHumanOutput<'static> {
    fn empty() -> Self {
        Self {
            rows: &[],
            groups: &[],
            held: &[],
            check: None,
        }
    }
}

/// Successor rows shared by `!` closes, Pomodoro closes, and Pending
/// unlinks (`docs/task-dependencies.md` §12.6 human output): linked
/// rows with their destination capsule, recovered rows with their
/// reason, and muted still-blocked rows. Relation groups print in
/// engine order between hand-offs and project groups.
pub(super) fn print_human_successor_rows(
    unblocked: &[TaskCompleteUnblockedJson],
    still_blocked: &[TaskCompleteStillBlockedJson],
    relation: RelationHumanOutput<'_>,
    dry_run: bool,
    styler: &Styler,
) {
    let mut offered: Vec<&TaskCompleteUnblockedJson> =
        unblocked.iter().filter(|row| row.offer.is_some()).collect();
    offered.sort_by_key(|row| {
        row.offer.as_ref().map(|offer| offer.n).unwrap_or(0)
    });
    print_unblocked_offer_groups(&offered, &["hand_off"], dry_run, styler);
    print_human_relation_rows(
        relation.rows,
        relation.groups,
        relation.held,
        relation.check,
        dry_run,
        styler,
    );
    print_unblocked_offer_groups(
        &offered,
        &["project", "project_finished"],
        dry_run,
        styler,
    );
    for row in unblocked.iter().filter(|row| row.offer.is_none()) {
        print_human_unblocked_row(row, dry_run, styler, None);
    }
    for blocked in still_blocked {
        let locator =
            task_complete_locator(&blocked.note_path, &blocked.block_id);
        let detail = match blocked.reason {
            "scheduled" => blocked
                .scheduled
                .as_deref()
                .map(|date| format!("until {date}"))
                .unwrap_or_else(|| "scheduled".to_string()),
            "project" => blocked
                .project
                .as_deref()
                .map(|path| format!("project {path}"))
                .unwrap_or_else(|| "project".to_string()),
            _ => format!("waits on {} more", blocked.waits_on),
        };
        let project_label = if blocked.trigger == "project_empty" {
            " · project has no open tasks"
        } else {
            ""
        };
        if blocked.text.is_empty() {
            println!("  still blocked  {locator}{project_label} · {detail}");
        } else {
            println!(
                "  still blocked  {}  {locator}{project_label} · {detail}",
                blocked.text
            );
        }
    }
}

fn print_unblocked_offer_groups(
    offered: &[&TaskCompleteUnblockedJson],
    groups: &[&str],
    dry_run: bool,
    styler: &Styler,
) {
    let mut last_header: Option<String> = None;
    for row in offered {
        let offer = row.offer.as_ref().expect("offered row");
        if !groups.contains(&offer.group) {
            continue;
        }
        let header = successor_offer_header(row, offer);
        if last_header.as_ref() != Some(&header) {
            println!("  {header}");
            last_header = Some(header);
        }
        print_human_unblocked_row(row, dry_run, styler, Some(offer.n));
    }
}

fn print_human_relation_rows(
    rows: &[TaskCompleteUnblockedJson],
    groups: &[crate::native::task_complete::RelationOfferGroup],
    held: &[RelationHeldJson],
    check: Option<&str>,
    dry_run: bool,
    styler: &Styler,
) {
    use crate::native::task_complete::relation_successors::copy;

    let mut offered: Vec<&TaskCompleteUnblockedJson> =
        rows.iter().filter(|row| row.offer.is_some()).collect();
    offered.sort_by_key(|row| {
        row.offer.as_ref().map(|offer| offer.n).unwrap_or(0)
    });
    let mut order: Vec<(String, String)> = Vec::new();
    for row in &offered {
        let key = relation_row_group_key(row);
        if !order.contains(&key) {
            order.push(key);
        }
    }
    for (after, label_key) in &order {
        let group_rows: Vec<&TaskCompleteUnblockedJson> = offered
            .iter()
            .copied()
            .filter(|row| {
                let key = relation_row_group_key(row);
                key.0 == *after && key.1 == *label_key
            })
            .collect();
        let Some(first) = group_rows.first() else {
            continue;
        };
        println!("  {}", relation_cli_header(first, groups, after, label_key));
        for row in &group_rows {
            print_human_relation_row(row, dry_run);
        }
        if let Some(fold) = relation_cli_fold(groups, after, label_key, held) {
            println!("  {}", styler.dim(&fold));
        }
    }
    if !offered.is_empty() {
        return;
    }
    if check == Some("unavailable") {
        let mut labels: Vec<String> = Vec::new();
        for row in held {
            if !labels.iter().any(|label| label == &row.relation.label) {
                labels.push(row.relation.label.clone());
            }
        }
        let notice = if labels.is_empty() {
            Some(
                "🪢 Couldn't check relation links · nothing offered"
                    .to_string(),
            )
        } else {
            copy::uncertainty_notice(&labels)
        };
        if let Some(line) = notice {
            println!("  {}", styler.dim(&line));
        }
        return;
    }
    for row in held {
        if row.reason == "already_planned" {
            continue;
        }
        let target = row
            .relation
            .after
            .first()
            .map(|target| target.text.as_str())
            .unwrap_or_default();
        let Some(reason) = held_copy_reason(row) else {
            continue;
        };
        if let Some(line) = copy::held_notice(copy::HeldNoticeCopy {
            source: &row.text,
            label: &row.relation.label,
            target,
            reason: &reason,
        }) {
            println!("  {}", styler.dim(&line));
        }
    }
}

fn relation_row_group_key(row: &TaskCompleteUnblockedJson) -> (String, String) {
    let after = row
        .offer
        .as_ref()
        .and_then(|offer| offer.after.clone())
        .unwrap_or_default();
    let label_key = row
        .relation
        .as_ref()
        .map(|info| info.key.clone())
        .unwrap_or_default();
    (after, label_key)
}

fn relation_cli_header(
    row: &TaskCompleteUnblockedJson,
    groups: &[crate::native::task_complete::RelationOfferGroup],
    after: &str,
    label_key: &str,
) -> String {
    use crate::native::task_complete::relation_successors::copy::{
        self, GroupCopy,
    };
    if let Some(group) = groups.iter().find(|group| {
        group.label_key == label_key
            && format!(
                "{}#^{}",
                display_relative(&group.target.note_path),
                group.target.block_id
            ) == after
    }) {
        return copy::cli_header(
            GroupCopy {
                label: &group.label,
                target: &group.target.text,
            },
            group.event,
        );
    }
    let info = row.relation.as_ref();
    let label = info.map(|info| info.label.as_str()).unwrap_or(label_key);
    let target = info
        .and_then(|info| info.after.first())
        .map(|target| target.text.as_str())
        .unwrap_or_default();
    let event = match info.map(|info| info.event) {
        Some("done") => copy::EventKind::Done,
        _ => copy::EventKind::Parked,
    };
    copy::cli_header(GroupCopy { label, target }, event)
}

fn relation_cli_fold(
    groups: &[crate::native::task_complete::RelationOfferGroup],
    after: &str,
    label_key: &str,
    held: &[RelationHeldJson],
) -> Option<String> {
    if let Some(fold) = groups.iter().find_map(|group| {
        (group.label_key == label_key
            && format!(
                "{}#^{}",
                display_relative(&group.target.note_path),
                group.target.block_id
            ) == after)
            .then(|| group.fold.clone())
            .flatten()
    }) {
        return Some(fold.replacen("not offered · ", "not offered  ", 1));
    }
    let _ = held;
    None
}

fn print_human_relation_row(row: &TaskCompleteUnblockedJson, dry_run: bool) {
    use crate::native::task_complete::relation_successors::copy::{
        self, GroupCopy,
    };
    let Some(offer) = row.offer.as_ref() else {
        return;
    };
    if let Some(message) = offer.verdict_message.as_deref() {
        println!("  offer {}  {message}", offer.n);
        for tickler in &offer.verdict_ticklers {
            println!("  {tickler}");
        }
        return;
    }
    let locator = task_complete_locator(&row.note_path, &row.block_id);
    let text = if row.text.is_empty() {
        String::new()
    } else {
        format!(" {}", row.text)
    };
    let suffix = relation_row_suffix(row);
    if let Some(link) = row.link.as_ref() {
        let verb = if dry_run { "would link" } else { "linked" };
        let previous = keep_moving_lane_name(row.previous_status_symbol);
        let current = keep_moving_lane_name(row.status_symbol);
        let minted = link
            .block_link
            .rsplit_once('^')
            .filter(|_| link.block_id_created)
            .map(|(_, id)| format!(" · added ^{}", id.trim_end_matches("]]")))
            .unwrap_or_default();
        let linked_suffix = row
            .relation
            .as_ref()
            .map(|info| {
                let target = info
                    .after
                    .first()
                    .map(|target| target.text.as_str())
                    .unwrap_or_default();
                copy::secondary_reason(GroupCopy {
                    label: &info.label,
                    target,
                })
            })
            .unwrap_or_default();
        println!(
            "  offer {}  {verb} {previous} \u{2192} {current}{text}  {locator} \u{2192} {} {linked_suffix}{suffix}{minted}",
            offer.n,
            successor_destination(link),
        );
    } else {
        let lane = keep_moving_lane_name(row.status_symbol);
        let reason = row
            .not_linked
            .as_deref()
            .filter(|_| !dry_run)
            .map(|reason| format!(" \u{b7} {}", not_linked_reason(reason)))
            .unwrap_or_default();
        println!(
            "  offer {}  {lane}{text}  {locator} \u{2192} {}{suffix}{reason}",
            offer.n,
            offer_destination_label(&offer.destination),
        );
    }
}

fn relation_row_suffix(row: &TaskCompleteUnblockedJson) -> String {
    use crate::native::task_complete::relation_successors::copy;
    let Some(info) = row.relation.as_ref() else {
        return String::new();
    };
    if info.after.len() < 2 {
        return String::new();
    }
    let texts: Vec<String> = info
        .after
        .iter()
        .map(|target| target.text.clone())
        .collect();
    copy::trigger_suffix(&texts)
        .map(|suffix| format!(" {suffix}"))
        .unwrap_or_default()
}

fn held_copy_reason(
    row: &RelationHeldJson,
) -> Option<crate::native::task_complete::relation_successors::copy::HeldReason>
{
    use crate::native::task_complete::relation_successors::copy::HeldReason;
    Some(match row.reason {
        "already_started" => HeldReason::AlreadyStarted,
        "hidden" => HeldReason::Hidden,
        "project" => HeldReason::Project,
        "waits_for" => HeldReason::WaitsFor(
            row.waits_for
                .as_ref()
                .map(|targets| {
                    targets.iter().map(|target| target.text.clone()).collect()
                })
                .unwrap_or_default(),
        ),
        "blocked" => {
            if let Some(date) = row.scheduled.as_ref() {
                HeldReason::Scheduled(date.clone())
            } else if row.project.is_some() {
                HeldReason::Project
            } else {
                HeldReason::Blocked {
                    waits_on: row.waits_on.as_ref().map(Vec::len).unwrap_or(0),
                }
            }
        }
        "already_planned" => return None,
        _ => return None,
    })
}

fn is_keep_moving_row(row: &TaskCompleteUnblockedJson) -> bool {
    row.offer
        .as_ref()
        .is_some_and(|offer| offer.group == "keep_moving")
}

fn is_relation_row(row: &TaskCompleteUnblockedJson) -> bool {
    row.trigger == "relation"
        || row
            .offer
            .as_ref()
            .is_some_and(|offer| offer.group == "relation")
}

/// Lane name for keep-moving human rows (`docs/task-dependencies.md`
/// §12.15): Pending, Next, Ready, Blocked. Never a raw status symbol.
fn keep_moving_lane_name(symbol: char) -> &'static str {
    match symbol {
        '/' => "Pending",
        '*' => "Next",
        ' ' => "Ready",
        '?' => "Blocked",
        _ => "Ready",
    }
}

/// Human keep-moving groups (`docs/task-dependencies.md` §12.15): one
/// header per leaf project that leads with why and says the rows are not
/// dependents, the offered rows in the shared offer numbering, and the
/// folded-count row. Never labeled "unblocked" and never the unlock glyph;
/// the header takes the Next colour because picks land in Next.
pub(super) fn print_human_keep_moving_rows(
    rows: &[TaskCompleteUnblockedJson],
    folds: &[crate::native::task_complete::KeepMovingFold],
    dry_run: bool,
    styler: &Styler,
) {
    let mut offered: Vec<&TaskCompleteUnblockedJson> =
        rows.iter().filter(|row| row.offer.is_some()).collect();
    offered.sort_by_key(|row| {
        row.offer.as_ref().map(|offer| offer.n).unwrap_or(0)
    });
    let mut projects: Vec<String> = Vec::new();
    for row in &offered {
        let project = super::keep_moving_net::row_project(row);
        if !projects.contains(&project) {
            projects.push(project);
        }
    }
    for project in projects {
        let group: Vec<&&TaskCompleteUnblockedJson> = offered
            .iter()
            .filter(|row| super::keep_moving_net::row_project(row) == project)
            .collect();
        let Some(first) = group.first() else {
            continue;
        };
        let stem = keep_moving_project_stem(&project);
        println!(
            "  {}",
            styler.yellow(&keep_moving_header(&stem, &first.unblocked_by))
        );
        for row in &group {
            print_human_keep_moving_row(row, &stem, dry_run, styler);
        }
        if let Some(fold) = folds
            .iter()
            .find(|fold| display_relative(&fold.project) == project)
            && let Some(parts) = keep_moving_fold_text(fold)
        {
            println!("  {}", styler.dim(&format!("not offered  {parts}")));
        }
    }
}

/// N's note stem: the project path without directories or extension.
fn keep_moving_project_stem(project: &str) -> String {
    Path::new(project)
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_else(|| project.to_string())
}

/// Group header: why the group appeared and that its rows are not
/// dependents (§12.15). ‹T› is the predecessor's cleaned text cut to 48
/// characters; several predecessors say "and k−1 more".
fn keep_moving_header(
    stem: &str,
    predecessors: &[TaskCompleteUnblockedCauseJson],
) -> String {
    let first = predecessors
        .first()
        .map(|cause| super::super::style::truncate(cause.text.trim(), 48))
        .filter(|text| !text.is_empty())
        .unwrap_or_else(|| "A task".to_string());
    let why = match predecessors.len() {
        0 | 1 => format!("{first} was its last task planned today"),
        count => format!(
            "{first} and {} more were its last tasks planned today",
            count - 1
        ),
    };
    format!("Keep {stem} moving \u{b7} {why} (ready tasks, not dependents)")
}

/// `2 blocked · 1 scheduled later · 1 hidden`, omitting zero parts; `None`
/// when nothing folded.
fn keep_moving_fold_text(
    fold: &crate::native::task_complete::KeepMovingFold,
) -> Option<String> {
    let mut parts = Vec::new();
    if fold.blocked > 0 {
        parts.push(format!("{} blocked", fold.blocked));
    }
    if fold.scheduled > 0 {
        parts.push(format!("{} scheduled later", fold.scheduled));
    }
    if fold.hidden > 0 {
        parts.push(format!("{} hidden", fold.hidden));
    }
    (!parts.is_empty()).then(|| parts.join(" \u{b7} "))
}

/// Destination of an unpicked offer: the entry the row would link under.
fn offer_destination_label(
    destination: &TaskCompleteOfferDestinationJson,
) -> String {
    match (destination.entry_role, destination.entry_name.as_str()) {
        ("created", "") => "new session".to_string(),
        ("created", name) => format!("new {name} session"),
        (_, "") => "session".to_string(),
        (_, name) => name.to_string(),
    }
}

fn print_human_keep_moving_row(
    row: &TaskCompleteUnblockedJson,
    stem: &str,
    dry_run: bool,
    _styler: &Styler,
) {
    let Some(offer) = row.offer.as_ref() else {
        return;
    };
    if let Some(message) = offer.verdict_message.as_deref() {
        println!("  offer {}  {message}", offer.n);
        for tickler in &offer.verdict_ticklers {
            println!("  {tickler}");
        }
        return;
    }
    let locator = task_complete_locator(&row.note_path, &row.block_id);
    let text = if row.text.is_empty() {
        String::new()
    } else {
        format!(" {}", row.text)
    };
    if let Some(link) = row.link.as_ref() {
        let verb = if dry_run { "would link" } else { "linked" };
        let previous = keep_moving_lane_name(row.previous_status_symbol);
        let current = keep_moving_lane_name(row.status_symbol);
        let minted = link
            .block_link
            .rsplit_once('^')
            .filter(|_| link.block_id_created)
            .map(|(_, id)| {
                format!(" \u{b7} added ^{}", id.trim_end_matches("]]"))
            })
            .unwrap_or_default();
        println!(
            "  offer {}  {verb} {previous} \u{2192} {current}{text}  {locator} \u{2192} {} \u{b7} keeps {stem} moving{minted}",
            offer.n,
            successor_destination(link),
        );
    } else {
        let lane = keep_moving_lane_name(row.status_symbol);
        // A real run says an unpicked row was not pulled in; a dry run
        // shows the `-p` hint instead.
        let reason = row
            .not_linked
            .as_deref()
            .filter(|_| !dry_run)
            .map(|reason| format!(" \u{b7} {}", not_linked_reason(reason)))
            .unwrap_or_default();
        println!(
            "  offer {}  {lane}{text}  {locator} \u{2192} {}{reason}",
            offer.n,
            offer_destination_label(&offer.destination),
        );
    }
}

fn successor_offer_header(
    row: &TaskCompleteUnblockedJson,
    offer: &TaskCompleteOfferJson,
) -> String {
    match offer.group {
        "hand_off" => "Hand-offs".to_string(),
        "project"
            if row.trigger
                == crate::native::task_complete::PROJECT_CREATED_TRIGGER =>
        {
            format!(
                "New project {}",
                offer.project.as_deref().unwrap_or_default()
            )
        }
        "project" => {
            format!("Project {}", offer.project.as_deref().unwrap_or_default())
        }
        "project_finished" => "Project finished".to_string(),
        other => other.to_string(),
    }
}

fn print_human_unblocked_row(
    unblocked: &TaskCompleteUnblockedJson,
    dry_run: bool,
    styler: &Styler,
    offer_n: Option<usize>,
) {
    if let Some(offer) = unblocked.offer.as_ref()
        && let Some(message) = offer.verdict_message.as_deref()
    {
        let prefix = match offer_n {
            Some(n) => format!("  offer {n}  "),
            None => "  ".to_string(),
        };
        println!("{prefix}{message}");
        for tickler in &offer.verdict_ticklers {
            println!("  {tickler}");
        }
        return;
    }
    let previous =
        style_task_status_marker(styler, unblocked.previous_status_symbol);
    let current = style_task_status_marker(styler, unblocked.status_symbol);
    let locator =
        task_complete_locator(&unblocked.note_path, &unblocked.block_id);
    let project_empty = unblocked.trigger == "project_empty";
    let project_label = if project_empty {
        " · project has no open tasks"
    } else {
        ""
    };
    let head = if unblocked.text.is_empty() {
        format!("{previous} → {current}")
    } else {
        format!("{previous} → {current} {}", unblocked.text)
    };
    let prefix = match offer_n {
        Some(n) => format!("  offer {n}  "),
        None => "  ".to_string(),
    };
    let relation_suffix = unblocked_relation_suffix(unblocked);
    if let Some(link) = unblocked.link.as_ref() {
        let verb = if dry_run { "would link" } else { "linked" };
        let destination = successor_destination(link);
        let minted = link
            .block_link
            .rsplit_once('^')
            .filter(|_| link.block_id_created)
            .map(|(_, id)| format!(" · added ^{}", id.trim_end_matches("]]")))
            .unwrap_or_default();
        println!(
            "{prefix}{verb} {head}  {locator} → {destination}{minted}{project_label}{relation_suffix}"
        );
    } else if project_empty
        && unblocked.hide_removed
        && unblocked.previous_status_symbol == unblocked.status_symbol
    {
        let verb = if dry_run { "would surface" } else { "surfaced" };
        println!(
            "{prefix}{verb} {}  {locator}{project_label}{relation_suffix}",
            unblocked.text
        );
    } else {
        let reason = unblocked
            .not_linked
            .as_deref()
            .map(not_linked_reason)
            .unwrap_or_default();
        if reason.is_empty() {
            println!(
                "{prefix}unblocked {head}  {locator}{project_label}{relation_suffix}"
            );
        } else {
            println!(
                "{prefix}unblocked {head}  {locator}{project_label} · {reason}{relation_suffix}"
            );
        }
    }
}

fn unblocked_relation_suffix(row: &TaskCompleteUnblockedJson) -> String {
    use crate::native::task_complete::relation_successors::copy::{
        self, GroupCopy,
    };
    row.relation
        .as_ref()
        .map(|info| {
            let target = info
                .after
                .first()
                .map(|target| target.text.as_str())
                .unwrap_or_default();
            format!(
                " {}",
                copy::secondary_reason(GroupCopy {
                    label: &info.label,
                    target,
                })
            )
        })
        .unwrap_or_default()
}

/// Destination capsule for a linked successor row
/// (`docs/task-dependencies.md` §12.6).
fn successor_destination(link: &TaskCompleteSuccessorLinkJson) -> String {
    let mut destination = if link.entry_created {
        if link.entry_name.is_empty() {
            "new session".to_string()
        } else {
            format!("new {} session", link.entry_name)
        }
    } else if link.entry_name.is_empty() {
        format!("line {}", link.entry_line)
    } else {
        link.entry_name.clone()
    };
    if link.next_up {
        destination.push_str(" (next up)");
    }
    destination
}

/// Human suffix for a recovered (not linked) successor row
/// (`docs/task-dependencies.md` §12.6).
fn not_linked_reason(reason: &str) -> &str {
    match reason {
        "already_planned" => "already planned",
        "project_task" => "project task",
        "project_member" => "project member",
        "hidden" => "hidden",
        "disabled" => "linking off",
        "not_selected" => "not pulled in",
        "resolved_in_card" => "resolved in card",
        "no_destination" => "no open Pomodoro",
        _ => reason,
    }
}

/// Ledger entry label for human output: the short name, or `line N`
/// when the entry has no name.
fn ledger_entry_label(name: &str, line: usize) -> String {
    if name.is_empty() {
        format!("line {line}")
    } else {
        name.to_string()
    }
}

/// Human locator for a subtask, left-open descendant, or recovered
/// dependent: the note path plus ` ^block-id`, or just the note path
/// when the row has no block ID.
fn task_complete_locator(note_path: &str, block_id: &str) -> String {
    if block_id.is_empty() {
        note_path.to_string()
    } else {
        format!("{note_path} ^{block_id}")
    }
}

/// Clean display text for a completed task line: the body after the
/// status box with the configured global filter, inline fields, and
/// trailing block ID removed.
pub(super) fn task_complete_display_text(
    task_line: &str,
    global_filter: &str,
    block_id: Option<&str>,
) -> String {
    let body = task_line
        .find("] ")
        .map(|index| task_line[index + 2..].to_string())
        .unwrap_or_else(|| task_line.to_string());
    note_tasks::clean_description(&body, global_filter, block_id)
}

pub(super) fn print_clip_file_confirmation(
    styler: &Styler,
    dry_run: bool,
    saved: &str,
    reused: bool,
) {
    let prefix = if dry_run {
        styler.success_prefix(true)
    } else {
        styler.green("\u{2713}")
    };
    let verb = if dry_run {
        "would save"
    } else if reused {
        "reused"
    } else {
        "saved"
    };
    let note = if reused && dry_run { " (reused)" } else { "" };
    println!("{prefix} {verb:<10}{}{note}", styler.cyan(saved));
}

pub(super) fn success_json(result: &CaptureResult) -> String {
    if result.template.is_none() {
        return serde_json::to_string(result)
            .expect("serialize capture result");
    }
    let mut value =
        serde_json::to_value(result).expect("serialize capture result");
    if let Some(template) = value
        .get_mut("template")
        .and_then(|value| value.as_object_mut())
    {
        if let Some(saved) = result.template_saved.as_ref() {
            template.insert("saved".to_string(), json!(saved));
        }
        if let Some(reason) = result.template_not_saved.as_deref() {
            template.insert("not_saved".to_string(), json!(reason));
        }
    }
    if let Some(row) = result.template_rows.first().and_then(Option::as_ref)
        && let serde_json::Value::Object(object) = &mut value
    {
        object.insert("template_row".to_string(), json!(row));
    }
    if let Some(captures) = value
        .get_mut("captures")
        .and_then(serde_json::Value::as_array_mut)
    {
        for (capture, row) in captures.iter_mut().zip(&result.template_rows) {
            if let (serde_json::Value::Object(object), Some(row)) =
                (capture, row)
            {
                object.insert("template_row".to_string(), json!(row));
            }
        }
    }
    serde_json::to_string(&value).expect("serialize capture result JSON")
}

fn print_human_record_note_success(
    result: &CaptureItemResult,
    record: &RecordNoteSummary,
    styler: &Styler,
    prefix: &str,
    ordinal: &str,
    target_label: &str,
) {
    let emoji = record
        .emoji
        .as_deref()
        .map(|emoji| format!("{emoji} "))
        .unwrap_or_default();
    let kind = record.label.to_lowercase();
    let updating = record.action == "update";
    // Album records name the album and owner instead of the bare path,
    // e.g. `Restaurants · body`. Config records keep the path label.
    let destination;
    let target_label = match record.album.as_ref() {
        Some(album) => {
            destination = match album.owner.as_deref() {
                Some(owner) => format!("{} · {owner}", album.title),
                None => album.title.clone(),
            };
            destination.as_str()
        }
        None => target_label,
    };
    if result.dry_run {
        if updating {
            println!(
                "{prefix} would update  {ordinal}{emoji}{kind}  {target_label}"
            );
        } else {
            println!(
                "{prefix} would create  {ordinal}{emoji}new {kind}  {target_label}"
            );
        }
        print!("{}", record.markdown);
        if !record.markdown.ends_with('\n') {
            println!();
        }
        return;
    }
    if updating {
        println!("{prefix} {emoji}updated {kind}  {ordinal}{target_label}");
    } else {
        println!("{prefix} {emoji}new {kind}  {ordinal}{target_label}");
    }
    for property in &record.properties {
        let Some(display) = property.display.as_deref() else {
            continue;
        };
        if updating
            && property.before_display.as_deref() == Some(display)
            && property.source.is_none()
        {
            continue;
        }
        if let Some(before) = property.before_display.as_deref()
            && before != display
        {
            println!("  {}  {before} → {display}", styler.dim(&property.label));
            continue;
        }
        println!("  {}  {}", styler.dim(&property.label), display);
    }
    if record.notes > 0 {
        let word = if record.notes == 1 { "note" } else { "notes" };
        println!("  {}", styler.dim(&format!("{} {word}", record.notes)));
    }
}

fn print_human_notebook_success(
    result: &CaptureItemResult,
    notebook: &super::notebook_note::NotebookSummary,
    styler: &Styler,
    prefix: &str,
    ordinal: &str,
) {
    if result.kind == "notebook_create" {
        let parent = notebook
            .breadcrumb
            .first()
            .map(String::as_str)
            .unwrap_or("");
        println!(
            "{prefix} 📓 New notebook {} · in {parent}",
            result.relative_target
        );
        if !result.text.is_empty() {
            println!("  {}", styler.dim(&result.text));
        }
        return;
    }
    let detail = if result.kind == "notebook_sub_note" {
        notebook
            .parent_block_id
            .as_deref()
            .map(|id| format!("{} › ^{id}", notebook.title))
            .unwrap_or_else(|| notebook.title.clone())
    } else if notebook.section_action == Some("create") {
        let section = notebook.section.as_deref().unwrap_or("");
        format!("{} › new section {section}", notebook.title)
    } else if let Some(section) = notebook.section.as_deref() {
        if let Some(block_id) = notebook.block_id.as_deref() {
            format!("{} › {section} › ^{block_id}", notebook.title)
        } else {
            format!("{} › {section}", notebook.title)
        }
    } else if let Some(block_id) = notebook.block_id.as_deref() {
        format!("{} › ^{block_id}", notebook.title)
    } else {
        notebook.title.clone()
    };
    println!("{prefix} 📓 Note → {ordinal}{detail}");
    println!("  {}", styler.dim(&result.task_line));
    for line in &result.sub_bullets {
        println!("  {}", styler.dim(line));
    }
    if let Some(clip) = &result.clip {
        if let Some(history) = &clip.history {
            println!("  {}", styler.dim(&format_clip_history_summary(history)));
        }
        for line in &clip.lines {
            println!("  {}", styler.dim(line));
        }
    }
}

pub(super) fn print_capture_error(
    error: CaptureError,
    output_format: OutputFormat,
) -> i32 {
    match output_format {
        OutputFormat::Human => {
            eprintln!("{COMMAND_NAME}: {}", error.message);
            if let Some(extra) = &error.extra {
                if let Some(draft) =
                    extra.get("draft").and_then(|value| value.as_str())
                {
                    eprintln!("{draft}");
                }
                if let Some(invalid) =
                    extra.get("invalid").and_then(|value| value.as_array())
                {
                    for diagnostic in invalid {
                        if let Some(message) = diagnostic
                            .get("message")
                            .and_then(|value| value.as_str())
                        {
                            eprintln!("  error: {message}");
                        }
                    }
                }
            }
            if !error.human_unblocked.is_empty()
                || !error.human_note_pull_rows.is_empty()
            {
                let styler = Styler::detect();
                // Keep-moving offers render as their own group, never
                // under the dependency wording.
                let (moving, rest): (Vec<_>, Vec<_>) = error
                    .human_unblocked
                    .iter()
                    .cloned()
                    .partition(is_keep_moving_row);
                let (relation, dependents): (Vec<_>, Vec<_>) =
                    rest.into_iter().partition(is_relation_row);
                print_human_successor_rows(
                    &dependents,
                    &[],
                    RelationHumanOutput {
                        rows: &relation,
                        groups: &error.human_relation_groups,
                        held: &error.human_relation_held,
                        check: error.human_relation_check,
                    },
                    false,
                    &styler,
                );
                print_human_keep_moving_rows(
                    &moving,
                    &error.human_keep_moving_folds,
                    false,
                    &styler,
                );
                print_human_note_pull_groups(
                    &error.human_note_pull_groups,
                    &error.human_note_pull_rows.iter().collect::<Vec<_>>(),
                    false,
                    &styler,
                );
            }
        }
        OutputFormat::Json => {
            let mut value = serde_json::Map::new();
            value.insert("ok".to_string(), json!(false));
            value.insert("error".to_string(), json!(error.message));
            if let Some(code) = error.code {
                value.insert("code".to_string(), json!(code));
            }
            if let Some(extra) = error.extra {
                for (key, extra_value) in extra {
                    value.insert(key, extra_value);
                }
            }
            println!("{}", serde_json::Value::Object(value));
        }
    }
    error.kind.exit_code()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CaptureError {
    pub(super) kind: CaptureErrorKind,
    pub(crate) message: String,
    /// Machine-readable failure code for template errors and capture
    /// choice refusals.
    pub(super) code: Option<String>,
    /// Extra JSON object fields merged into the error value (offers and
    /// `pull_in` on `pull_in_choice_required`).
    pub(super) extra: Option<serde_json::Map<String, serde_json::Value>>,
    /// Numbered offer rows printed on human `pull_in_choice_required`.
    pub(super) human_unblocked: Vec<TaskCompleteUnblockedJson>,
    /// Folded counts for the keep-moving groups in `human_unblocked`.
    pub(super) human_keep_moving_folds:
        Vec<crate::native::task_complete::KeepMovingFold>,
    /// Surviving relation groups for human `pull_in_choice_required`.
    pub(super) human_relation_groups:
        Vec<crate::native::task_complete::RelationOfferGroup>,
    /// Relation-held rows for human `pull_in_choice_required`.
    pub(super) human_relation_held: Vec<RelationHeldJson>,
    /// `"checked"` / `"unavailable"` for human `pull_in_choice_required`.
    pub(super) human_relation_check: Option<&'static str>,
    /// Numbered note-pull offers shown on human `pull_in_choice_required`.
    pub(super) human_note_pull_rows: Vec<TaskCompleteUnblockedJson>,
    /// Note-pull groups, including folded counts, shown on human refusals.
    pub(super) human_note_pull_groups: Vec<PullInGroupJson>,
    /// Typed failure kind for `bob ref today`. Capture JSON ignores this
    /// so `bob capture` stays byte-identical.
    pub(crate) typed: Option<CaptureFailureKind>,
}

/// Stable classification for capture failures `bob ref today` maps to
/// error codes. Never string-matched from messages.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CaptureFailureKind {
    NoDailyNote,
    NoPomodorosSection,
    NoOpenPomodoro,
    MultipleRunning,
    InvalidPomodoroName,
    StalePomodoro,
    PomodoroClosed,
    LedgerInvariant,
    NoteChanged,
}

impl CaptureError {
    pub(super) fn usage(message: impl Into<String>) -> Self {
        Self {
            kind: CaptureErrorKind::Usage,
            message: message.into(),
            code: None,
            extra: None,
            human_unblocked: Vec::new(),
            human_keep_moving_folds: Vec::new(),
            human_relation_groups: Vec::new(),
            human_relation_held: Vec::new(),
            human_relation_check: None,
            human_note_pull_rows: Vec::new(),
            human_note_pull_groups: Vec::new(),
            typed: None,
        }
    }

    pub(super) fn usage_code(
        message: impl Into<String>,
        code: impl Into<String>,
    ) -> Self {
        Self {
            kind: CaptureErrorKind::Usage,
            message: message.into(),
            code: Some(code.into()),
            extra: None,
            human_unblocked: Vec::new(),
            human_keep_moving_folds: Vec::new(),
            human_relation_groups: Vec::new(),
            human_relation_held: Vec::new(),
            human_relation_check: None,
            human_note_pull_rows: Vec::new(),
            human_note_pull_groups: Vec::new(),
            typed: None,
        }
    }

    pub(super) fn io(message: impl Into<String>) -> Self {
        Self {
            kind: CaptureErrorKind::Io,
            message: message.into(),
            code: None,
            extra: None,
            human_unblocked: Vec::new(),
            human_keep_moving_folds: Vec::new(),
            human_relation_groups: Vec::new(),
            human_relation_held: Vec::new(),
            human_relation_check: None,
            human_note_pull_rows: Vec::new(),
            human_note_pull_groups: Vec::new(),
            typed: None,
        }
    }

    pub(super) fn io_code(
        message: impl Into<String>,
        code: impl Into<String>,
    ) -> Self {
        Self {
            kind: CaptureErrorKind::Io,
            message: message.into(),
            code: Some(code.into()),
            extra: None,
            human_unblocked: Vec::new(),
            human_keep_moving_folds: Vec::new(),
            human_relation_groups: Vec::new(),
            human_relation_held: Vec::new(),
            human_relation_check: None,
            human_note_pull_rows: Vec::new(),
            human_note_pull_groups: Vec::new(),
            typed: None,
        }
    }

    pub(super) fn pull_in_choice_required(
        extra: serde_json::Map<String, serde_json::Value>,
        human_unblocked: Vec<TaskCompleteUnblockedJson>,
        human_keep_moving_folds: Vec<
            crate::native::task_complete::KeepMovingFold,
        >,
        human_relation_groups: Vec<
            crate::native::task_complete::RelationOfferGroup,
        >,
        human_relation_held: Vec<RelationHeldJson>,
        human_relation_check: Option<&'static str>,
        human_note_pull_rows: Vec<TaskCompleteUnblockedJson>,
        human_note_pull_groups: Vec<PullInGroupJson>,
    ) -> Self {
        Self {
            kind: CaptureErrorKind::Io,
            message: "this capture has successor offers; pass --pull-in"
                .to_string(),
            code: Some("pull_in_choice_required".to_string()),
            extra: Some(extra),
            human_unblocked,
            human_keep_moving_folds,
            human_relation_groups,
            human_relation_held,
            human_relation_check,
            human_note_pull_rows,
            human_note_pull_groups,
            typed: None,
        }
    }

    pub(super) fn inbox_route_choice_required(
        report: &InboxRouteReport,
        move_choice: &super::move_choice::MoveChoiceReport,
    ) -> Self {
        let count = report.candidate_count;
        let task_word = if count == 1 { "task" } else { "tasks" };
        let mut extra = serde_json::Map::new();
        extra.insert(
            "inbox_route".to_string(),
            serde_json::to_value(report).expect("serialize inbox_route"),
        );
        extra.insert(
            "move_choice".to_string(),
            serde_json::to_value(move_choice).expect("serialize move_choice"),
        );
        Self {
            kind: CaptureErrorKind::Io,
            message: format!(
                "this capture changes {count} inbox {task_word}; pass --inbox-route"
            ),
            code: Some("inbox_route_choice_required".to_string()),
            extra: Some(extra),
            human_unblocked: Vec::new(),
            human_keep_moving_folds: Vec::new(),
            human_relation_groups: Vec::new(),
            human_relation_held: Vec::new(),
            human_relation_check: None,
            human_note_pull_rows: Vec::new(),
            human_note_pull_groups: Vec::new(),
            typed: None,
        }
    }

    /// Stale route answers: a usage error (exit 2) listing the unknown
    /// picks and the current candidates, exactly as `--pull-in` does
    /// for stale picks. Writes nothing.
    pub(super) fn inbox_route_stale(
        report: &InboxRouteReport,
        bad: &[String],
    ) -> Self {
        let mut lines = vec![format!(
            "stale, unknown, or out-of-range --inbox-route pick: {}",
            bad.join(", ")
        )];
        for candidate in &report.candidates {
            lines.push(format!(
                "{} {} {}",
                candidate.n, candidate.key, candidate.text
            ));
        }
        let mut extra = serde_json::Map::new();
        extra.insert(
            "inbox_route".to_string(),
            serde_json::to_value(report).expect("serialize inbox_route"),
        );
        Self {
            kind: CaptureErrorKind::Usage,
            message: lines.join("\n"),
            code: Some("inbox_route_stale".to_string()),
            extra: Some(extra),
            human_unblocked: Vec::new(),
            human_keep_moving_folds: Vec::new(),
            human_relation_groups: Vec::new(),
            human_relation_held: Vec::new(),
            human_relation_check: None,
            human_note_pull_rows: Vec::new(),
            human_note_pull_groups: Vec::new(),
            typed: None,
        }
    }

    /// Refused route answer: an I/O-class error (exit 1) naming the
    /// refused candidate, the requested destination, and the move
    /// engine's reason. Writes nothing.
    pub(super) fn inbox_route_refused(
        report: &InboxRouteReport,
        candidate: &InboxRouteCandidate,
        destination: String,
        message: String,
    ) -> Self {
        let mut extra = serde_json::Map::new();
        extra.insert(
            "inbox_route".to_string(),
            serde_json::to_value(report).expect("serialize inbox_route"),
        );
        extra.insert(
            "refused".to_string(),
            serde_json::json!({
                "key": candidate.key,
                "n": candidate.n,
                "destination": destination,
                "message": message,
            }),
        );
        Self {
            kind: CaptureErrorKind::Io,
            message,
            code: Some("inbox_route_refused".to_string()),
            extra: Some(extra),
            human_unblocked: Vec::new(),
            human_keep_moving_folds: Vec::new(),
            human_relation_groups: Vec::new(),
            human_relation_held: Vec::new(),
            human_relation_check: None,
            human_note_pull_rows: Vec::new(),
            human_note_pull_groups: Vec::new(),
            typed: None,
        }
    }

    /// Conflicting card-verdict answers (the same row in two of `-p`,
    /// `-x`, and `-X`; `-X` twice for one row; or an `-i` answer on a
    /// card-resolved task): a usage error (exit 2) that writes nothing
    /// and carries `pull_in` plus the flat offers list with each row's
    /// close descriptor (`docs/task-dependencies.md` §12.14).
    pub(super) fn offer_verdict_conflict(
        pull_in: &PullInJson,
        offers: &[TaskCompleteUnblockedJson],
        message: String,
    ) -> Self {
        let mut lines = vec![message];
        lines.extend(offer_list_lines(offers));
        let mut extra = serde_json::Map::new();
        extra.insert(
            "pull_in".to_string(),
            serde_json::to_value(pull_in).expect("serialize pull_in"),
        );
        extra.insert(
            "offers".to_string(),
            serde_json::to_value(offers).expect("serialize offers"),
        );
        Self {
            kind: CaptureErrorKind::Usage,
            message: lines.join("\n"),
            code: Some("offer_verdict_conflict".to_string()),
            extra: Some(extra),
            human_unblocked: Vec::new(),
            human_keep_moving_folds: Vec::new(),
            human_relation_groups: Vec::new(),
            human_relation_held: Vec::new(),
            human_relation_check: None,
            human_note_pull_rows: Vec::new(),
            human_note_pull_groups: Vec::new(),
            typed: None,
        }
    }

    /// Unknown, stale, or out-of-range `-x`/`-X` selector: a usage error
    /// (exit 2) that writes nothing and lists the current offers with
    /// their close descriptors, as `inbox_route_stale` does.
    pub(super) fn offer_close_stale(
        pull_in: &PullInJson,
        offers: &[TaskCompleteUnblockedJson],
        bad: &[String],
    ) -> Self {
        let mut lines = vec![format!(
            "stale, unknown, or out-of-range offer verdict pick: {}",
            bad.join(", ")
        )];
        lines.extend(offer_list_lines(offers));
        let mut extra = serde_json::Map::new();
        extra.insert(
            "pull_in".to_string(),
            serde_json::to_value(pull_in).expect("serialize pull_in"),
        );
        extra.insert(
            "offers".to_string(),
            serde_json::to_value(offers).expect("serialize offers"),
        );
        Self {
            kind: CaptureErrorKind::Usage,
            message: lines.join("\n"),
            code: Some("offer_close_stale".to_string()),
            extra: Some(extra),
            human_unblocked: Vec::new(),
            human_keep_moving_folds: Vec::new(),
            human_relation_groups: Vec::new(),
            human_relation_held: Vec::new(),
            human_relation_check: None,
            human_note_pull_rows: Vec::new(),
            human_note_pull_groups: Vec::new(),
            typed: None,
        }
    }

    /// A verdict the row's descriptor refuses: an I/O-class error
    /// (exit 1) naming the row, the outcome, and the engine's `why`.
    /// Writes nothing and carries the current offers for a reopen.
    pub(super) fn offer_close_refused(
        pull_in: &PullInJson,
        offers: &[TaskCompleteUnblockedJson],
        key: &str,
        n: usize,
        outcome: &str,
        why: &str,
        message: String,
    ) -> Self {
        let mut lines = vec![message];
        lines.extend(offer_list_lines(offers));
        let mut extra = serde_json::Map::new();
        extra.insert(
            "pull_in".to_string(),
            serde_json::to_value(pull_in).expect("serialize pull_in"),
        );
        extra.insert(
            "offers".to_string(),
            serde_json::to_value(offers).expect("serialize offers"),
        );
        extra.insert(
            "refused".to_string(),
            serde_json::json!({
                "key": key,
                "n": n,
                "outcome": outcome,
                "why": why,
            }),
        );
        Self {
            kind: CaptureErrorKind::Io,
            message: lines.join("\n"),
            code: Some("offer_close_refused".to_string()),
            extra: Some(extra),
            human_unblocked: Vec::new(),
            human_keep_moving_folds: Vec::new(),
            human_relation_groups: Vec::new(),
            human_relation_held: Vec::new(),
            human_relation_check: None,
            human_note_pull_rows: Vec::new(),
            human_note_pull_groups: Vec::new(),
            typed: None,
        }
    }

    /// Hidden `--refuse-override` guard: an I/O-class error (exit 1)
    /// carrying `pomodoro_override_refused` for subprocess callers.
    pub(super) fn override_refused(message: impl Into<String>) -> Self {
        Self {
            kind: CaptureErrorKind::Io,
            message: message.into(),
            code: Some("pomodoro_override_refused".to_string()),
            extra: None,
            human_unblocked: Vec::new(),
            human_keep_moving_folds: Vec::new(),
            human_relation_groups: Vec::new(),
            human_relation_held: Vec::new(),
            human_relation_check: None,
            human_note_pull_rows: Vec::new(),
            human_note_pull_groups: Vec::new(),
            typed: None,
        }
    }
}

/// Current offer rows for verdict refusal messages: `n key text`
/// in offer order, as `--pull-in` stale picks list them.
fn offer_list_lines(offers: &[TaskCompleteUnblockedJson]) -> Vec<String> {
    let mut rows: Vec<(usize, String, String)> = Vec::new();
    for row in offers {
        if let Some(offer) = row.offer.as_ref() {
            rows.push((offer.n, offer.key.clone(), row.text.clone()));
        }
    }
    rows.sort_by_key(|(n, _, _)| *n);
    rows.into_iter()
        .map(|(n, key, text)| format!("{n} {key} {text}"))
        .collect()
}

impl CaptureError {
    /// Hidden `--expect-past-windows` guard: an I/O-class error (exit 1)
    /// carrying `pomodoro_past_windows_changed` for subprocess callers.
    pub(super) fn past_windows_changed(message: impl Into<String>) -> Self {
        Self {
            kind: CaptureErrorKind::Io,
            message: message.into(),
            code: Some("pomodoro_past_windows_changed".to_string()),
            extra: None,
            human_unblocked: Vec::new(),
            human_keep_moving_folds: Vec::new(),
            human_relation_groups: Vec::new(),
            human_relation_held: Vec::new(),
            human_relation_check: None,
            human_note_pull_rows: Vec::new(),
            human_note_pull_groups: Vec::new(),
            typed: None,
        }
    }

    pub(crate) fn with_typed(mut self, typed: CaptureFailureKind) -> Self {
        self.typed = Some(typed);
        self
    }

    /// Attach a one-based capture item and an absolute half-open UTF-8 byte
    /// range to a refusal that points at a token in the original draft.
    pub(super) fn with_item_span(
        mut self,
        item: usize,
        span: (usize, usize),
    ) -> Self {
        let mut extra = self.extra.take().unwrap_or_default();
        extra.insert("item".to_string(), json!(item));
        extra.insert(
            "span".to_string(),
            json!({"start": span.0, "end": span.1}),
        );
        self.extra = Some(extra);
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum CaptureErrorKind {
    Usage,
    Io,
}

impl CaptureErrorKind {
    pub(super) fn exit_code(self) -> i32 {
        match self {
            Self::Usage => 2,
            Self::Io => 1,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, to_value};

    #[test]
    fn capture_destination_swatches_respect_no_color() {
        let destination = PomodoroLinkEndpoint {
            line: 3,
            name: Some("BOB".to_string()),
            time_range: None,
            role: Some("created"),
            placed_type: None,
            merged: None,
        };
        let refs = BTreeMap::from([(
            "bob".to_string(),
            vec![crate::native::pomodoro_themes::ThemeRef {
                key: "bob".to_string(),
                name: "BOB".to_string(),
                hue: Some(crate::native::pomodoro_homes::Hue::Magenta),
                hue_source: crate::native::pomodoro_themes::HueSource::Hash,
                known: true,
                configured: None,
                start: None,
                end: None,
            }],
        )]);
        let plain = format_pomodoro_task_destination(&destination);
        assert_eq!(
            format_styled_pomodoro_task_destination(
                &destination,
                &refs,
                &Styler::plain(),
            ),
            plain
        );
        assert!(format_styled_pomodoro_task_destination(
            &destination,
            &refs,
            &Styler::colored(),
        )
        .contains("38;5;211m■"));
    }

    fn task_row(
        typed_work_log: Vec<String>,
        typed_work_log_details: Vec<Vec<String>>,
    ) -> PomodoroCloseTaskJson {
        PomodoroCloseTaskJson {
            role: "worked",
            block_link: "[[bob#^capture-stop]]".to_string(),
            ledger_line: 6,
            index: Some(1),
            resolved: true,
            relative_target: Some("bob.md".to_string()),
            block_id: "capture-stop".to_string(),
            text: Some("Add support for `=x` syntax!".to_string()),
            previous_status_symbol: Some('*'),
            previous_status_name: Some("Next".to_string()),
            status_symbol: Some('/'),
            status_name: Some("In Progress".to_string()),
            status_changed: true,
            carried: true,
            work_log: Vec::new(),
            work_log_created: true,
            typed_work_log,
            typed_work_log_details,
            warning: None,
        }
    }

    #[test]
    fn typed_work_log_details_omitted_when_no_entry_has_details() {
        let row = task_row(
            vec!["*2026-09-28* — wired the lexer".to_string()],
            vec![Vec::new()],
        );
        let value = to_value(&row).expect("serialize");
        assert_eq!(
            value.get("typed_work_log"),
            Some(&json!(["*2026-09-28* — wired the lexer"]))
        );
        assert!(value.get("typed_work_log_details").is_none());

        let bare = task_row(Vec::new(), Vec::new());
        assert!(to_value(&bare)
            .expect("serialize")
            .get("typed_work_log_details")
            .is_none());
    }

    #[test]
    fn typed_work_log_details_align_with_entries_when_present() {
        let row = task_row(
            vec![
                "*2026-09-28* — wired the lexer".to_string(),
                "*2026-09-28* — sketched the parser".to_string(),
            ],
            vec![vec!["chose a hand-rolled lexer".to_string()], Vec::new()],
        );
        let value = to_value(&row).expect("serialize");
        assert_eq!(
            value.get("typed_work_log_details"),
            Some(&json!([["chose a hand-rolled lexer"], []]))
        );
    }

    #[test]
    fn log_entry_details_omitted_when_empty() {
        let plain = PomodoroCloseLogEntryJson {
            index: 2,
            text: "wired the lexer".to_string(),
            details: Vec::new(),
            all: false,
        };
        let value = to_value(&plain).expect("serialize");
        assert_eq!(value, json!({ "index": 2, "text": "wired the lexer" }));
        let detailed = PomodoroCloseLogEntryJson {
            index: 2,
            text: "wired the lexer".to_string(),
            details: vec!["chose a hand-rolled lexer".to_string()],
            all: false,
        };
        assert_eq!(
            to_value(&detailed).expect("serialize"),
            json!({
                "index": 2,
                "text": "wired the lexer",
                "details": ["chose a hand-rolled lexer"],
            })
        );
        let starred = PomodoroCloseLogEntryJson {
            index: 3,
            text: "code review".to_string(),
            details: Vec::new(),
            all: true,
        };
        assert_eq!(
            to_value(&starred).expect("serialize"),
            json!({ "index": 3, "text": "code review", "all": true })
        );
    }

    #[test]
    fn typed_entry_details_print_two_spaces_under_their_entry() {
        let row = task_row(
            vec![
                "*2026-09-28* — wired the lexer".to_string(),
                "*2026-09-28* — sketched the parser".to_string(),
            ],
            vec![vec!["chose a hand-rolled lexer".to_string()], Vec::new()],
        );
        assert_eq!(
            typed_work_log_lines(&row, "      "),
            vec![
                "      *2026-09-28* — wired the lexer".to_string(),
                "        chose a hand-rolled lexer".to_string(),
                "      *2026-09-28* — sketched the parser".to_string(),
            ]
        );
        // A short details array renders without panicking.
        let short = task_row(
            vec!["*2026-09-28* — wired the lexer".to_string()],
            Vec::new(),
        );
        assert_eq!(
            typed_work_log_lines(&short, "    "),
            vec!["    *2026-09-28* — wired the lexer".to_string()]
        );
    }

    fn minimal_summary(text: &str) -> TaskCompleteSummaryJson {
        TaskCompleteSummaryJson {
            raw: "!a^1".to_string(),
            note: "a".to_string(),
            note_path: "a.md".to_string(),
            block_id: "1".to_string(),
            action: "completed",
            completion_date: Some("2026-10-28".to_string()),
            text: text.to_string(),
            subtasks: Vec::new(),
            subtasks_left_open: Vec::new(),
            ledger: None,
            unblocked: Vec::new(),
            keep_moving: Vec::new(),
            relation_successors: Vec::new(),
            relation_held: Vec::new(),
            relation_check: None,
            relation_groups: Vec::new(),
            still_blocked: Vec::new(),
            unblocked_check: "checked",
            project_folds: Vec::new(),
            keep_moving_folds: Vec::new(),
        }
    }

    fn unblocked_row(
        text: &str,
        key: &str,
        n: usize,
    ) -> TaskCompleteUnblockedJson {
        TaskCompleteUnblockedJson {
            note_path: "b.md".to_string(),
            block_id: format!("{key}-id"),
            line: 1,
            text: text.to_string(),
            previous_status_symbol: ' ',
            previous_status_name: "Ready".to_string(),
            status_symbol: '?',
            status_name: "Blocked".to_string(),
            inbox: false,
            unblocked_by: Vec::new(),
            link: None,
            not_linked: Some("not_selected".to_string()),
            trigger: "recovery",
            hide_removed: false,
            offer: Some(TaskCompleteOfferJson {
                key: key.to_string(),
                n,
                group: "hand_off",
                selected: false,
                verdict: None,
                verdict_message: None,
                verdict_ticklers: Vec::new(),
                block_id_needed: false,
                project: None,
                destination: TaskCompleteOfferDestinationJson {
                    day_file: "today.md".to_string(),
                    entry_name: "work".to_string(),
                    entry_role: "slot",
                    fallback: false,
                },
                after: None,
                moved_from: None,
                merged_from: None,
                note: None,
                close: None,
                order_key: (0, (0, 0), 0, std::path::PathBuf::from("b.md"), 0),
                settled_ordinal: None,
            }),
            relation: None,
        }
    }

    fn minimal_result() -> CaptureItemResult {
        CaptureItemResult {
            note_pull: None,
            record_note: None,
            notebook: None,
            task_theme: None,
            type_relocation: None,
            ok: true,
            dry_run: false,
            routed: true,
            route: None,
            route_label: "a.md".to_string(),
            relative_target: "a.md".to_string(),
            target: "/tmp/bob/a.md".to_string(),
            text: String::new(),
            task_line: "- [x] done".to_string(),
            kind: "task_complete",
            fanout: None,
            created: "2026-10-28".to_string(),
            scheduled: None,
            priority: None,
            priority_label: None,
            placement: Placement::Completed,
            sub_bullets: Vec::new(),
            notes: Vec::new(),
            drawers: Vec::new(),
            clip: None,
            schedule_log: None,
            block_id: Some("1".to_string()),
            day_file: None,
            block_link: None,
            pomodoro_link_placement: None,
            parent_line: None,
            parent_text: None,
            drawer: None,
            parent_status_symbol: None,
            parent_status_name: None,
            toggle_direction: None,
            previous_task_line: None,
            status_symbol: None,
            status_name: None,
            previous_status_symbol: None,
            previous_status_name: None,
            pomodoro_name: None,
            creates_pomodoro: None,
            pomodoro_already_linked: None,
            removed_pomodoro_links: None,
            removed_scheduled: None,
            pomodoro_selector_unused: None,
            toggle_behavior: None,
            status_changed: None,
            pomodoro_link_action: None,
            pomodoro_link_source: None,
            pomodoro_link_destination: None,
            project_note: None,
            pomodoro_start: None,
            pomodoro_adjust: None,
            pomodoro_shift: None,
            pomodoro_split: None,
            pomodoro_merge: None,
            pomodoro_close: None,
            pomodoro_past: None,
            pomodoro_reset: None,
            dependency_update: None,
            consumed_links: Vec::new(),
            task_complete: None,
            r#ref: None,
            gmail: Vec::new(),
            toggle_task_description: None,
            link_guard: None,
            dropped_dependencies: Vec::new(),
            blocks_line_updates: Vec::new(),
            minted_dependent_block_id: None,
            dependency_updates: Vec::new(),
            minted_blocker_block_id: None,
            relation_updates: Vec::new(),
            relation_cancels: Vec::new(),
            relation_completes: Vec::new(),
            relation_merge_output: Default::default(),
            transfer_move_output: Default::default(),
            linked_from_line_updates: Vec::new(),
            minted_relation_block_ids: Vec::new(),
            mirror_skipped: Vec::new(),
            list: None,
            relation_label_hides: Vec::new(),
            relation_successors: Vec::new(),
            relation_held: Vec::new(),
            relation_check: None,
            relation_groups: Vec::new(),
        }
    }

    #[test]
    fn relation_completes_omitted_when_empty() {
        let result = minimal_result();
        let value = to_value(&result).expect("serialize");
        assert!(value.get("relation_completes").is_none(), "{value}");
    }

    #[test]
    fn all_unblocked_covers_nested_relation_summaries_in_order() {
        let mut whole = minimal_summary("whole");
        whole
            .unblocked
            .push(unblocked_row("whole-row", "whole-key", 7));
        let mut nested = minimal_summary("nested");
        nested
            .unblocked
            .push(unblocked_row("nested-row", "nested-key", 7));
        let mut result = minimal_result();
        result.task_complete = Some(whole);
        result.relation_completes.push(RelationCompleteJson {
            effect: RelationCompleteEffect::Completed,
            source: RelationCompleteSourceJson {
                path: "a.md".to_string(),
                line: 1,
                block_id: "1".to_string(),
                text: "source".to_string(),
            },
            target: RelationCompleteTargetJson {
                path: "b.md".to_string(),
                block_id: "2".to_string(),
                text: "target".to_string(),
            },
            labels: vec!["implemented by".to_string()],
            action: RelationAction::Added,
            previous_status: ' ',
            previous_status_name: "Ready".to_string(),
            project_open_tasks: None,
            task_complete: nested,
            message: "✓ completed".to_string(),
        });
        let texts: Vec<String> =
            all_unblocked(&result).map(|row| row.text.clone()).collect();
        assert_eq!(
            texts,
            vec!["whole-row".to_string(), "nested-row".to_string()]
        );
        let value = to_value(&result).expect("serialize");
        assert_eq!(
            value["relation_completes"].as_array().unwrap().len(),
            1,
            "{value}"
        );
        assert_eq!(
            value["relation_completes"][0]["labels"],
            json!(["implemented by"]),
            "{value}"
        );
    }

    #[test]
    fn unblocked_vecs_mut_reaches_nested_rows() {
        let mut whole = minimal_summary("whole");
        whole
            .unblocked
            .push(unblocked_row("whole-row", "whole-key", 1));
        let mut nested = minimal_summary("nested");
        nested
            .unblocked
            .push(unblocked_row("nested-row", "nested-key", 1));
        let mut result = minimal_result();
        result.task_complete = Some(whole);
        result.relation_completes.push(RelationCompleteJson {
            effect: RelationCompleteEffect::Completed,
            source: RelationCompleteSourceJson {
                path: "a.md".to_string(),
                line: 1,
                block_id: "1".to_string(),
                text: "source".to_string(),
            },
            target: RelationCompleteTargetJson {
                path: "b.md".to_string(),
                block_id: "2".to_string(),
                text: "target".to_string(),
            },
            labels: Vec::new(),
            action: RelationAction::Added,
            previous_status: ' ',
            previous_status_name: "Ready".to_string(),
            project_open_tasks: None,
            task_complete: nested,
            message: String::new(),
        });
        for vec in unblocked_vecs_mut(&mut result) {
            for row in vec.iter_mut() {
                if let Some(offer) = row.offer.as_mut() {
                    offer.selected = true;
                }
            }
        }
        assert!(all_unblocked(&result)
            .all(|row| row.offer.as_ref().is_some_and(|offer| offer.selected)));
    }

    #[test]
    fn all_unblocked_includes_project_note_successors() {
        let mut result = minimal_result();
        result.kind = "project_note";
        result.project_note = Some(ProjectNoteSummary {
            basename: "cash_goog_exit.md".to_string(),
            parent_route: "cash".to_string(),
            parent_link: "[[cash]]".to_string(),
            tasks: 2,
            sections: Vec::new(),
            task_links: Vec::new(),
            moved_tasks: Vec::new(),
            move_summary: None,
            repaired_files: None,
            move_hint: None,
            form: Some("offer"),
            successors: Some(vec![unblocked_row(
                "Draft the memo",
                "cash_goog_exit.md@abcd1234",
                1,
            )]),
            project_folds: vec![crate::native::task_complete::ProjectFold {
                project: std::path::PathBuf::from("cash_goog_exit.md"),
                hidden: 1,
                already_planned: 1,
                sub_projects: 0,
            }],
        });
        let texts: Vec<String> =
            all_unblocked(&result).map(|row| row.text.clone()).collect();
        assert_eq!(texts, vec!["Draft the memo".to_string()]);
        let folds: Vec<_> = all_project_folds(&result).collect();
        assert_eq!(folds.len(), 1);
        assert_eq!(folds[0].hidden, 1);
        assert_eq!(folds[0].already_planned, 1);
        assert_eq!(folds[0].sub_projects, 0);
        let value = to_value(&result.project_note).expect("serialize");
        assert_eq!(value["form"], json!("offer"));
        assert_eq!(value["successors"].as_array().unwrap().len(), 1);
        assert!(value.get("project_folds").is_none(), "{value}");
    }

    #[test]
    fn create_project_note_omits_form_and_successors() {
        let summary = ProjectNoteSummary {
            basename: "cash_goog_exit.md".to_string(),
            parent_route: "cash".to_string(),
            parent_link: "[[cash]]".to_string(),
            tasks: 1,
            sections: Vec::new(),
            task_links: Vec::new(),
            moved_tasks: Vec::new(),
            move_summary: None,
            repaired_files: None,
            move_hint: None,
            form: None,
            successors: None,
            project_folds: Vec::new(),
        };
        let value = to_value(&summary).expect("serialize");
        assert!(value.get("form").is_none(), "{value}");
        assert!(value.get("successors").is_none(), "{value}");
    }

    #[test]
    fn project_created_offer_header_reads_new_project() {
        let mut row = unblocked_row("Draft the memo", "k", 1);
        row.trigger = crate::native::task_complete::PROJECT_CREATED_TRIGGER;
        row.offer.as_mut().expect("offer").group = "project";
        row.offer.as_mut().expect("offer").project =
            Some("cash_goog_exit.md".to_string());
        let offer = row.offer.as_ref().expect("offer");
        assert_eq!(
            successor_offer_header(&row, offer),
            "New project cash_goog_exit.md"
        );
        row.trigger = "project_empty";
        let offer = row.offer.as_ref().expect("offer");
        assert_eq!(
            successor_offer_header(&row, offer),
            "Project cash_goog_exit.md"
        );
    }

    #[test]
    fn complete_effect_omitted_when_none() {
        let update = RelationUpdateJson {
            direction: RelationDirection::To,
            source: RelationEndpointJson {
                path: "a.md".to_string(),
                line: 1,
                block_id: "1".to_string(),
                text: "s".to_string(),
                archived: None,
            },
            target: RelationEndpointJson {
                path: "b.md".to_string(),
                line: 2,
                block_id: "2".to_string(),
                text: "t".to_string(),
                archived: None,
            },
            label: Some("implemented by".to_string()),
            previous_label: None,
            action: RelationAction::Added,
            cancel_effect: None,
            complete_effect: None,
            merge_effect: None,
            offer_effect: None,
        };
        let value = to_value(&update).expect("serialize");
        assert!(value.get("complete_effect").is_none(), "{value}");
        let completed = RelationUpdateJson {
            complete_effect: Some(RelationCompleteEffect::Completed),
            ..update
        };
        let value = to_value(&completed).expect("serialize");
        assert_eq!(value["complete_effect"], json!("completed"), "{value}");
    }

    #[test]
    fn relation_completes_omits_already_closed_rows() {
        let mut result = minimal_result();
        result.relation_completes.push(RelationCompleteJson {
            effect: RelationCompleteEffect::AlreadyClosed,
            source: RelationCompleteSourceJson {
                path: "a.md".to_string(),
                line: 1,
                block_id: "1".to_string(),
                text: "source".to_string(),
            },
            target: RelationCompleteTargetJson {
                path: "b.md".to_string(),
                block_id: "2".to_string(),
                text: "target".to_string(),
            },
            labels: vec!["implemented by".to_string()],
            action: RelationAction::Added,
            previous_status: 'x',
            previous_status_name: "Done".to_string(),
            project_open_tasks: None,
            task_complete: minimal_summary("already"),
            message: "Already Done · link only".to_string(),
        });
        let value = to_value(&result).expect("serialize");
        assert!(value.get("relation_completes").is_none(), "{value}");
    }

    #[test]
    fn clipboard_history_summary_uses_provider_and_known_endpoint_ages() {
        let history = capture_clip::ClipHistoryOutput {
            provider: "alfred".to_string(),
            entries: vec![
                capture_clip::ClipHistoryEntryOutput {
                    age_seconds: Some(3),
                    app: Some("Safari".to_string()),
                },
                capture_clip::ClipHistoryEntryOutput {
                    age_seconds: Some(240),
                    app: None,
                },
            ],
        };
        assert_eq!(
            format_clip_history_summary(&history),
            "clip  from Alfred · newest 3s · oldest 4m"
        );
        let command = capture_clip::ClipHistoryOutput {
            provider: "command".to_string(),
            entries: vec![capture_clip::ClipHistoryEntryOutput {
                age_seconds: None,
                app: None,
            }],
        };
        assert_eq!(format_clip_history_summary(&command), "clip  from command");
    }
}
