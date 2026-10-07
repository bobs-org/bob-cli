//! Capture result/error types and human/JSON output.
use super::*;

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
/// URL, its offline library verdict, the staged job (real runs that
/// queued only), and the inbox fallback (queued items only).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct RefItemJson {
    pub(super) url: String,
    pub(super) cleaned_url: String,
    pub(super) dedupe_key: String,
    pub(super) display: String,
    pub(super) route_hint: &'static str,
    pub(super) library: RefLibraryJson,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) job: Option<RefJobJson>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) fallback: Option<RefFallbackJson>,
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
}

fn is_false(value: &bool) -> bool {
    !value
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct CaptureResult {
    #[serde(flatten)]
    pub(super) item: CaptureItemResult,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(super) captures: Vec<CaptureItemResult>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) global_destination: Option<GlobalDestinationSummary>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(super) warnings: Vec<String>,
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
        warnings: Vec<String>,
        plan_budget: Option<CapturePlanBudget>,
        pomodoro_blocks: Vec<PomodoroBlockJson>,
        task_blocks: Vec<TaskBlockJson>,
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
            warnings,
            plan_budget,
            pomodoro_blocks,
            task_blocks,
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
    pub(super) dependency_update: Option<DependencyUpdateJson>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) task_complete: Option<TaskCompleteSummaryJson>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) r#ref: Option<RefItemJson>,
    #[serde(skip)]
    pub(super) toggle_task_description: Option<String>,
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

/// One dependent recovered by a `task_complete` item.
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
}

/// Summary JSON for one item's dependency effects, reported as
/// `dependency_update` on the capture result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct DependencyUpdateJson {
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
    for warning in &result.warnings {
        eprintln!("{COMMAND_NAME}: {}: {warning}", styler.warning_prefix());
    }
}

/// Plan-budget meter line (stdout) plus one stderr warning per fired
/// cap. Budget warnings stay out of the `warnings` vector: they
/// describe the ledger, not the captured item.
pub(super) fn print_plan_budget(result: &CaptureResult) {
    let Some(budget) = result.plan_budget.as_ref() else {
        return;
    };
    let styler = Styler::detect();
    let themes =
        format!("{}/{} themes", budget.themes.count, budget.themes.cap);
    let links = format!("{}/{} links", budget.links.count, budget.links.cap);
    let themes = if budget.themes.over {
        styler.red(&themes)
    } else {
        styler.green(&themes)
    };
    let links = if budget.links.over {
        styler.red(&links)
    } else {
        styler.green(&links)
    };
    let mut line = format!("plan {themes} · {links}");
    if !budget.added_themes.is_empty() {
        let count = budget.added_themes.len();
        let noun = if count == 1 { "theme" } else { "themes" };
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

pub(super) fn print_human_success(result: &CaptureResult) {
    if let Some(global) = &result.global_destination {
        print_global_destination_summary(global);
    }
    if result.captures.is_empty() {
        print_human_item_success(result, None);
        return;
    }

    let total = result.captures.len();
    for (index, item) in result.captures.iter().enumerate() {
        if index > 0 {
            println!();
        }
        print_human_item_success(item, Some((index + 1, total)));
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
            styler.cyan(&format!("{} → reading queue", reference.display)),
        ),
        _ => (
            "queued",
            styler.cyan(&format!("{} → reading queue", reference.display)),
        ),
    };
    println!("{prefix} {verb}  {ordinal}{subject}");
    let detail = match reference.library.verdict {
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
            "new to your library · clips in the background".to_string()
        }
        _ => "clipping in the background · bob ref jobs".to_string(),
    };
    println!("  {}", styler.dim(&detail));
}

pub(super) fn print_human_item_success(
    result: &CaptureItemResult,
    ordinal: Option<(usize, usize)>,
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
    if let Some(close) = result.pomodoro_close.as_ref() {
        print_human_pomodoro_close_success(
            result,
            close,
            &styler,
            &prefix,
            &ordinal,
            &target_label,
        );
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
        return;
    }
    if result.toggle_direction.is_some() {
        print_human_task_toggle_success(
            result,
            &styler,
            &prefix,
            &ordinal,
            &target_label,
        );
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
        return;
    }
    if result.kind == "pomodoro_start" {
        if let Some(start) = result.pomodoro_start.as_ref() {
            print_human_pomodoro_start_success(
                result,
                start,
                &styler,
                &prefix,
                &ordinal,
                &target_label,
            );
            return;
        }
    }
    if result.kind == "ref"
        && let Some(reference) = result.r#ref.as_ref()
    {
        print_human_ref_item_success(
            result, reference, &styler, &prefix, &ordinal,
        );
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
        let marker = result
            .parent_status_symbol
            .map(|symbol| {
                format!("{} ", style_task_status_marker(&styler, symbol))
            })
            .unwrap_or_default();
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
    if let Some(update) = result.dependency_update.as_ref() {
        print_human_dependency_line(update, &styler);
    }
    if result.kind == "pomodoro_task"
        && let Some(destination) = result.pomodoro_link_destination.as_ref()
    {
        println!(
            "  {}",
            styler.dim(&format_pomodoro_task_destination(destination))
        );
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
        println!(
            "  {}",
            styler.dim(&format!(
                "next: {next_name}{created_text} at line {}{carries_text}",
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

/// Second line for the `@route+block-id!` link-presence toggle:
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
    let Some(update) = result.dependency_update.as_ref() else {
        println!("{prefix} updated  {ordinal}{target_label}");
        println!("  {}", styler.dim(&result.task_line));
        return;
    };
    let verb = if result.dry_run {
        "would add dependency to"
    } else {
        "Add dependency to"
    };
    println!(
        "{prefix} {verb} \"{}\"  {ordinal}{target_label}",
        update.dependent_text
    );
    println!("  {}", styler.dim(&result.task_line));
    print_human_dependency_line(update, styler);
}

/// One shared `⛓ depends on …` trailer for every capture carrying a
/// `dependency_update`: added and already-present counts, the waiting
/// count, and the resulting Blocked distinction.
pub(super) fn print_human_dependency_line(
    update: &DependencyUpdateJson,
    styler: &Styler,
) {
    let links = update
        .prerequisites
        .iter()
        .map(|prereq| prereq.link.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    let head = if update.new_task {
        "New task depends on"
    } else if update.added == 0 {
        "Already depends on"
    } else {
        "depends on"
    };
    let mut chips = vec![format!("{} added", update.added)];
    if update.already_present > 0 {
        chips.push(format!("{} already", update.already_present));
    }
    if update.open_prerequisites > 0 {
        chips.push(format!("waiting on {}", update.open_prerequisites));
    }
    if update.dependent_status == '?' {
        chips.push("Blocked until finished".to_string());
    }
    println!("  ⛓ {head} {links} ({})", styler.dim(&chips.join(" · ")),);
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
            if let Some(block_link) = result.block_link.as_deref() {
                let marker = styler.green("+");
                println!("  {marker} {block_link}");
            }
            if result.removed_pomodoro_links.unwrap_or(0) > 0 {
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
        println!("  {next_marker} already Next  {description}{block_id}");
    } else if previous_symbol == '/' && current_symbol == '/' {
        println!("  {next_marker} stays In Progress  {description}{block_id}");
    } else {
        println!(
            "  {previous_marker} → {next_marker}  {description}{block_id}"
        );
    }

    let mut chips = Vec::new();
    if result.removed_scheduled.is_some() {
        chips.push("removed future schedule".to_string());
    }
    if result.schedule_log.is_some() {
        chips.push("logged schedule change".to_string());
    }
    if !chips.is_empty() {
        println!("  {}", styler.dim(&chips.join(" · ")));
    }

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
            println!(
                "  {}",
                styler.dim(&format!(
                    "Moved Task Link {source} → {destination}{created}"
                ))
            );
        }
        Some("linked") => {
            let destination = result
                .pomodoro_link_destination
                .as_ref()
                .map(format_pomodoro_endpoint)
                .unwrap_or_else(|| "current/next".to_string());
            println!(
                "  {}",
                styler.dim(&format!("Linked under {destination}"))
            );
            if result.creates_pomodoro == Some(true) {
                println!("  {}", styler.dim(&format!("created {destination}")));
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
    println!("{prefix} {verb} {head}  {ordinal}{target_label} {block_label}");
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
    for unblocked in &summary.unblocked {
        let previous =
            style_task_status_marker(styler, unblocked.previous_status_symbol);
        let current = style_task_status_marker(styler, unblocked.status_symbol);
        let locator =
            task_complete_locator(&unblocked.note_path, &unblocked.block_id);
        if unblocked.text.is_empty() {
            println!("  unblocked {previous} → {current}  {locator}");
        } else {
            println!(
                "  unblocked {previous} → {current} {}  {locator}",
                unblocked.text
            );
        }
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
    serde_json::to_string(result).expect("serialize capture result")
}

pub(super) fn print_capture_error(
    error: CaptureError,
    output_format: OutputFormat,
) -> i32 {
    match output_format {
        OutputFormat::Human => eprintln!("{COMMAND_NAME}: {}", error.message),
        OutputFormat::Json => {
            let mut value = serde_json::Map::new();
            value.insert("ok".to_string(), json!(false));
            value.insert("error".to_string(), json!(error.message));
            if let Some(code) = error.code {
                value.insert("code".to_string(), json!(code));
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
    /// Machine-readable failure code, serialized only for the strict
    /// plan-budget refusal (`plan_theme_cap_exceeded`).
    pub(super) code: Option<String>,
}

impl CaptureError {
    pub(super) fn usage(message: impl Into<String>) -> Self {
        Self {
            kind: CaptureErrorKind::Usage,
            message: message.into(),
            code: None,
        }
    }

    pub(super) fn io(message: impl Into<String>) -> Self {
        Self {
            kind: CaptureErrorKind::Io,
            message: message.into(),
            code: None,
        }
    }

    /// Strict-mode refusal: an I/O-class error (exit 1) carrying the
    /// `plan_theme_cap_exceeded` code for machine callers.
    pub(super) fn strict(message: impl Into<String>) -> Self {
        Self {
            kind: CaptureErrorKind::Io,
            message: message.into(),
            code: Some(plan_budget::LINT_THEME_CAP.to_string()),
        }
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
}
