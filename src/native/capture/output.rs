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
}

/// Additive `ref` object on a reference item result: the classified
/// URL, its offline library verdict, the canonical resolved parent, the
/// staged job (real runs that queued only), and the parent fallback
/// (queued items only). `listen` is true when the standalone `%` marker (or
/// compatible `@` alias) requested companion audio; `listen_hint` explains an unchanged marked
/// item's no-new-listen outcome with the explicit retry command.
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
pub(super) struct PomodoroCloseTaskLinkJson {
    pub(super) index: u32,
    pub(super) ledger_line: usize,
    pub(super) block_link: String,
    pub(super) block_id: String,
    pub(super) marker: &'static str,
    pub(super) outcome: &'static str,
    pub(super) source: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct PomodoroCloseLogEntryJson {
    pub(super) index: u32,
    pub(super) text: String,
    /// Nested detail lines under the entry, omitted when empty.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(super) details: Vec<String>,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) name: Option<String>,
    pub(super) time_range: Option<String>,
    pub(super) created: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct PomodoroCloseSummaryJson {
    pub(super) raw: String,
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
    // Vault-relative day file, so human output and clients name the day
    // file even on link forms (whose `relative_target` is the route note).
    pub(super) day_relative: String,
    pub(super) entry_line: String,
    pub(super) planned: PomodoroCloseTimingJson,
    pub(super) closed: PomodoroCloseTimingJson,
    pub(super) closed_at: String,
    pub(super) remaining_minutes: i64,
    pub(super) decremented_minutes: u64,
    pub(super) tasks: Vec<PomodoroCloseTaskJson>,
    pub(super) carried: Vec<PomodoroCloseCarriedJson>,
    pub(super) notes: Vec<String>,
    pub(super) next_pomodoro: Option<PomodoroCloseNextJson>,
    /// Dependents this close fully unblocked and linked into the
    /// continuation, or recovered with the reason they were not linked
    /// (`docs/task-dependencies.md` §12.5). Empty when the close
    /// completed nothing with an `[id::]` identity.
    pub(super) unblocked: Vec<TaskCompleteUnblockedJson>,
    /// Open dependents of the completed tasks that stay blocked.
    pub(super) still_blocked: Vec<TaskCompleteStillBlockedJson>,
    /// `"checked"` when the dependents lookup ran or the `[id::]` gate
    /// proved it unnecessary; `"unavailable"` when the snapshot could
    /// not be built and the close succeeded without linking.
    pub(super) unblocked_check: &'static str,
    /// Project-group folded counts from this close's successor plan.
    /// Never serialized.
    #[serde(skip)]
    pub(super) project_folds: Vec<crate::native::task_complete::ProjectFold>,
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
pub(super) enum CaptureWarning {
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
    pub(super) fn message(&self) -> Cow<'_, str> {
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
pub(super) struct StructuredCaptureWarning {
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
    /// Notes Control-Return should open. Always present (empty when
    /// nothing opens). Never per item.
    pub(super) open_notes: Vec<OpenNoteJson>,
    /// Draft-level successor-offer summary. Present on every success.
    pub(super) pull_in: PullInJson,
    /// Inbox tasks whose checkbox status changed during this batch.
    pub(super) inbox_route: InboxRouteReport,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct GlobalDestinationSummary {
    pub(super) mode: &'static str,
    pub(super) route: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) block_id: Option<String>,
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
        open_notes: Vec<OpenNoteJson>,
        pull_in: PullInJson,
        inbox_route: InboxRouteReport,
    ) -> Self {
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
            open_notes,
            pull_in,
            inbox_route,
        }
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
pub(super) struct ProjectNoteSummary {
    pub(super) basename: String,
    pub(super) parent_route: String,
    pub(super) parent_link: String,
    pub(super) tasks: usize,
    pub(super) sections: Vec<String>,
    pub(super) task_links: Vec<ProjectTaskLinkJson>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct CaptureItemResult {
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
    pub(super) kind: &'static str,
    pub(super) created: String,
    pub(super) scheduled: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) priority: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) priority_label: Option<String>,
    pub(super) placement: Placement,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(super) sub_bullets: Vec<String>,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) parent_section: Option<String>,
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
    pub(super) pomodoro_close: Option<PomodoroCloseSummaryJson>,
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
    pub(super) still_blocked: Vec<TaskCompleteStillBlockedJson>,
    pub(super) unblocked_check: &'static str,
    /// Project-group folded counts from this item's successor plan.
    /// Never serialized.
    #[serde(skip)]
    pub(super) project_folds: Vec<crate::native::task_complete::ProjectFold>,
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
    pub(super) block_id_needed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) project: Option<String>,
    pub(super) destination: TaskCompleteOfferDestinationJson,
    /// Crate-private draft order; never a JSON field.
    #[serde(skip)]
    pub(super) order_key: crate::native::task_complete::OfferOrderKey,
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
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(super) groups: Vec<PullInGroupJson>,
}

impl PullInJson {
    #[cfg(test)]
    pub(super) fn empty() -> Self {
        Self {
            supported: true,
            offer_count: 0,
            choice: None,
            picked_count: 0,
            groups: Vec::new(),
        }
    }
}

/// One project group in `pull_in.groups`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct PullInGroupJson {
    pub(super) group: &'static str,
    pub(super) project: String,
    pub(super) folded: PullInFoldedJson,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct PullInFoldedJson {
    pub(super) hidden: usize,
    pub(super) already_planned: usize,
    pub(super) sub_projects: usize,
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
    let types = format!("{}/{} types", budget.themes.count, budget.themes.cap);
    let links = format!("{}/{} links", budget.links.count, budget.links.cap);
    let types = if budget.themes.over {
        styler.red(&types)
    } else {
        styler.green(&types)
    };
    let links = if budget.links.over {
        styler.red(&links)
    } else {
        styler.green(&links)
    };
    let mut line = format!("plan {types} · {links}");
    if !budget.added_themes.is_empty() {
        let count = budget.added_themes.len();
        let noun = if count == 1 { "type" } else { "types" };
        line.push_str(&format!(
            "  (+{count} {noun}: {})",
            budget.added_themes.join(", ")
        ));
    }
    println!("{line}");
    for warning in &budget.warnings {
        eprintln!(
            "{COMMAND_NAME}: {}: {}",
            styler.warning_prefix(),
            warning.message
        );
    }
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
    match destination.role {
        Some("created") => format!("→ new Pomodoro {target}"),
        Some("current") => match destination.time_range.as_deref() {
            Some(range) => format!("→ into running {target} ({range})"),
            None => format!("→ into running {target}"),
        },
        Some("named") => format!("→ under {target} (named)"),
        _ => format!("→ under {target} (next up)"),
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
        print_human_item_success(result, None, &result.theme_refs);
        print_pull_in_hint(result);
        print_inbox_route_hint(result);
        return;
    }

    let total = result.captures.len();
    for (index, item) in result.captures.iter().enumerate() {
        if index > 0 {
            println!();
        }
        print_human_item_success(
            item,
            Some((index + 1, total)),
            &result.theme_refs,
        );
    }
    print_pull_in_hint(result);
    print_inbox_route_hint(result);
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
    if result.pull_in.choice.is_some() {
        return;
    }
    let styler = Styler::detect();
    if result.dry_run {
        let mut numbers: Vec<usize> = Vec::new();
        let items = if result.captures.is_empty() {
            std::slice::from_ref(&result.item)
        } else {
            result.captures.as_slice()
        };
        for item in items {
            for row in all_unblocked(item) {
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
        println!("  {}", styler.dim(&format!("-p {list} · -p all")));
    } else {
        println!(
            "  {}",
            styler.dim(
                "offers were not pulled in; link later in Obsidian, Bob Mac Capture, or Ctrl+Shift+Enter"
            )
        );
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
    vecs
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
        dry_run,
        styler,
    );
}

fn pulled_in_phrase(unblocked: &[TaskCompleteUnblockedJson]) -> String {
    let offer_count =
        unblocked.iter().filter(|row| row.offer.is_some()).count();
    let picked_count = unblocked
        .iter()
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
    match global.block_id.as_deref() {
        Some(block_id) => println!(
            "{}  {} · under {}",
            styler.dim("global"),
            styler.cyan(&route_label),
            styler.cyan(&format!("^{block_id}")),
        ),
        None => {
            println!("{}  {}", styler.dim("global"), styler.cyan(&route_label),)
        }
    }
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
        let parent_section = result
            .parent_section
            .as_deref()
            .map(|title| format!(" · {}", styler.cyan(title)))
            .unwrap_or_default();
        println!("  under {marker}{parent_text}{block_id}{parent_section}");
    }
    println!("  {}", styler.dim(&result.task_line));
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
    if let Some(note) = result.project_note.as_ref() {
        let sections = if note.sections.is_empty() {
            "—".to_string()
        } else {
            note.sections.join(", ")
        };
        let task_word = if note.tasks == 1 { "task" } else { "tasks" };
        println!(
            "  {}",
            styler.dim(&format!(
                "{} {task_word} · sections {sections}",
                note.tasks
            ))
        );
    }
    for line in &result.sub_bullets {
        println!("  {}", styler.dim(line));
    }
    print_human_dependency_updates(result, &styler);
    print_human_relation_updates(result, &styler);
    if let Some(clip) = &result.clip {
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
                "{verb} {name} {}-{} ({}m){created} at line {}",
                start.start,
                start.end,
                start.duration_minutes,
                start.pomodoro_line,
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
    let name = close.pomodoro_name.as_deref().unwrap_or("session");
    let decremented = close.decremented_minutes;
    let range_text = if decremented > 0 {
        format!(
            "{}-{} → {}-{} ({}m, −{}m)",
            close.planned.start,
            close.planned.end,
            close.closed.start,
            close.closed.end,
            close.closed.duration_minutes,
            decremented
        )
    } else {
        format!(
            "{}-{} ({}m)",
            close.closed.start, close.closed.end, close.closed.duration_minutes
        )
    };
    let mut header = format!(
        "{verb} {name} {range_text} · {} line {}",
        close.day_relative, close.pomodoro_line
    );
    if close.remaining_minutes < 0 {
        header.push_str(&format!(
            " (ran {}m over)",
            close.remaining_minutes.abs()
        ));
    }
    println!("{prefix} {ordinal}{header}");
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
        let unblocked_text = if close.unblocked.is_empty() {
            String::new()
        } else {
            format!(" · {} unblocked", close.unblocked.len())
        };
        let pulled = pulled_in_phrase(&close.unblocked);
        println!(
            "  {}",
            styler.dim(&format!(
                "next: {next_name}{created_text} at line {}{carries_text}{unblocked_text}{pulled}",
                next.line
            ))
        );
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
            format!("🪢 links to {quoted}{label}{locator}")
        }
        (RelationDirection::To, RelationAction::Added, true) => {
            format!("🪢 would link to {quoted}{label}{locator}")
        }
        (RelationDirection::From, RelationAction::Added, false) => {
            format!(
                "🪢 {quoted} now links here{label}   {} ^{}",
                update.source.path, update.source.block_id
            )
        }
        (RelationDirection::From, RelationAction::Added, true) => {
            format!(
                "🪢 {quoted} would now link here{label}   {} ^{}",
                update.source.path, update.source.block_id
            )
        }
        (_, RelationAction::Relabeled, false) => {
            format!("🪢 relabeled {quoted}{label}{locator}")
        }
        (_, RelationAction::Relabeled, true) => {
            format!("🪢 would relabel {quoted}{label}{locator}")
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
            .map(|name| format!(" · under {}", styler.cyan(name)))
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
            println!(
                "  {} moved Task Link {} → {}",
                styler.green("↗"),
                styler.cyan(&source),
                styler.cyan(&destination),
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
                        "Moved Task Link {source} → {destination}{created}"
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
                    styler.dim(&format!("Linked under {destination}"))
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
    let pulled = pulled_in_phrase(&summary.unblocked);
    println!(
        "{prefix} {verb} {head}  {ordinal}{target_label} {block_label}{pulled}"
    );
    print_task_complete_details(summary, result.dry_run, styler);
}

/// Successor rows shared by `!` closes and Pomodoro closes
/// (`docs/task-dependencies.md` §12.6 human output): linked rows with
/// their destination capsule, recovered rows with their reason, and
/// muted still-blocked rows.
pub(super) fn print_human_successor_rows(
    unblocked: &[TaskCompleteUnblockedJson],
    still_blocked: &[TaskCompleteStillBlockedJson],
    dry_run: bool,
    styler: &Styler,
) {
    let mut offered: Vec<&TaskCompleteUnblockedJson> =
        unblocked.iter().filter(|row| row.offer.is_some()).collect();
    offered.sort_by_key(|row| {
        row.offer.as_ref().map(|offer| offer.n).unwrap_or(0)
    });
    let mut last_header: Option<String> = None;
    for row in offered {
        let offer = row.offer.as_ref().expect("offered row");
        let header = match offer.group {
            "hand_off" => "Hand-offs".to_string(),
            "project" => format!(
                "Project {}",
                offer.project.as_deref().unwrap_or_default()
            ),
            "project_finished" => "Project finished".to_string(),
            other => other.to_string(),
        };
        if last_header.as_ref() != Some(&header) {
            println!("  {header}");
            last_header = Some(header);
        }
        print_human_unblocked_row(row, dry_run, styler, Some(offer.n));
    }
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

fn print_human_unblocked_row(
    unblocked: &TaskCompleteUnblockedJson,
    dry_run: bool,
    styler: &Styler,
    offer_n: Option<usize>,
) {
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
    if let Some(link) = unblocked.link.as_ref() {
        let verb = if dry_run { "would link" } else { "linked" };
        let destination = successor_destination(link);
        let minted = link
            .block_link
            .rsplit_once('^')
            .filter(|_| link.block_id_created)
            .map(|(_, id)| format!(" · added ^{id}"))
            .unwrap_or_default();
        println!(
            "{prefix}{verb} {head}  {locator} → {destination}{minted}{project_label}"
        );
    } else if project_empty
        && unblocked.hide_removed
        && unblocked.previous_status_symbol == unblocked.status_symbol
    {
        let verb = if dry_run { "would surface" } else { "surfaced" };
        println!(
            "{prefix}{verb} {}  {locator}{project_label}",
            unblocked.text
        );
    } else {
        let reason = unblocked
            .not_linked
            .as_deref()
            .map(not_linked_reason)
            .unwrap_or_default();
        if reason.is_empty() {
            println!("{prefix}unblocked {head}  {locator}{project_label}");
        } else {
            println!(
                "{prefix}unblocked {head}  {locator}{project_label} · {reason}"
            );
        }
    }
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

pub(super) fn print_capture_error(
    error: CaptureError,
    output_format: OutputFormat,
) -> i32 {
    match output_format {
        OutputFormat::Human => {
            eprintln!("{COMMAND_NAME}: {}", error.message);
            if !error.human_unblocked.is_empty() {
                let styler = Styler::detect();
                print_human_successor_rows(
                    &error.human_unblocked,
                    &[],
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
    /// Machine-readable failure code for template errors, strict plan
    /// budgets, and capture choice refusals.
    pub(super) code: Option<String>,
    /// Extra JSON object fields merged into the error value (offers and
    /// `pull_in` on `pull_in_choice_required`).
    pub(super) extra: Option<serde_json::Map<String, serde_json::Value>>,
    /// Numbered offer rows printed on human `pull_in_choice_required`.
    pub(super) human_unblocked: Vec<TaskCompleteUnblockedJson>,
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
            typed: None,
        }
    }

    /// Strict-mode refusal: an I/O-class error (exit 1) carrying the
    /// `plan_theme_cap_exceeded` code for machine callers.
    pub(super) fn strict(message: impl Into<String>) -> Self {
        Self {
            kind: CaptureErrorKind::Io,
            message: message.into(),
            code: Some(plan_budget::LINT_THEME_CAP.to_string()),
            extra: None,
            human_unblocked: Vec::new(),
            typed: None,
        }
    }

    pub(super) fn pull_in_choice_required(
        extra: serde_json::Map<String, serde_json::Value>,
        human_unblocked: Vec<TaskCompleteUnblockedJson>,
    ) -> Self {
        Self {
            kind: CaptureErrorKind::Io,
            message: "this capture has successor offers; pass --pull-in"
                .to_string(),
            code: Some("pull_in_choice_required".to_string()),
            extra: Some(extra),
            human_unblocked,
            typed: None,
        }
    }

    pub(super) fn inbox_route_choice_required(
        report: &InboxRouteReport,
    ) -> Self {
        let count = report.candidate_count;
        let task_word = if count == 1 { "task" } else { "tasks" };
        let mut extra = serde_json::Map::new();
        extra.insert(
            "inbox_route".to_string(),
            serde_json::to_value(report).expect("serialize inbox_route"),
        );
        Self {
            kind: CaptureErrorKind::Io,
            message: format!(
                "this capture changes {count} inbox {task_word}; pass --inbox-route"
            ),
            code: Some("inbox_route_choice_required".to_string()),
            extra: Some(extra),
            human_unblocked: Vec::new(),
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
            typed: None,
        }
    }

    pub(crate) fn with_typed(mut self, typed: CaptureFailureKind) -> Self {
        self.typed = Some(typed);
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
        };
        let value = to_value(&plain).expect("serialize");
        assert_eq!(value, json!({ "index": 2, "text": "wired the lexer" }));
        let detailed = PomodoroCloseLogEntryJson {
            index: 2,
            text: "wired the lexer".to_string(),
            details: vec!["chose a hand-rolled lexer".to_string()],
        };
        assert_eq!(
            to_value(&detailed).expect("serialize"),
            json!({
                "index": 2,
                "text": "wired the lexer",
                "details": ["chose a hand-rolled lexer"],
            })
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
            still_blocked: Vec::new(),
            unblocked_check: "checked",
            project_folds: Vec::new(),
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
                block_id_needed: false,
                project: None,
                destination: TaskCompleteOfferDestinationJson {
                    day_file: "today.md".to_string(),
                    entry_name: "work".to_string(),
                    entry_role: "slot",
                    fallback: false,
                },
                order_key: (0, (0, 0), std::path::PathBuf::from("b.md"), 0),
            }),
        }
    }

    fn minimal_result() -> CaptureItemResult {
        CaptureItemResult {
            task_theme: None,
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
            created: "2026-10-28".to_string(),
            scheduled: None,
            priority: None,
            priority_label: None,
            placement: Placement::Completed,
            sub_bullets: Vec::new(),
            clip: None,
            schedule_log: None,
            block_id: Some("1".to_string()),
            day_file: None,
            block_link: None,
            pomodoro_link_placement: None,
            parent_line: None,
            parent_text: None,
            parent_section: None,
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
            pomodoro_close: None,
            pomodoro_reset: None,
            dependency_update: None,
            consumed_links: Vec::new(),
            task_complete: None,
            r#ref: None,
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
            linked_from_line_updates: Vec::new(),
            minted_relation_block_ids: Vec::new(),
            mirror_skipped: Vec::new(),
            relation_label_hides: Vec::new(),
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
}
