//! Pure `bob gcal sync` planner: snapshot plus journal state in,
//! `SyncPlan{contents, records, report}` out.
//!
//! The plan inserts new rows in time order (never reordering existing
//! lines), adopts crash-recovery rows by their `📅` URL, tombstones
//! journaled rows whose line is gone, and files each created Event Task
//! under a past, running, or planned MEET Pomodoro in the same write. It
//! keeps a blank line under the `## Events` heading, then writes or
//! refreshes the Bob-owned Events stamp as its last step. A run at a
//! later minute rewrites only that stamp. When the computed contents
//! equal the input, the plan reports no change and carries no records.

use std::{
    collections::{BTreeMap, HashSet},
    ops::Range,
};

use chrono::{NaiveDate, NaiveDateTime};

use crate::native::{
    capture::line_spans,
    capture_pomodoros::{self, PomodoroEntry, PomodoroState},
    capture_task_toggle::{
        child_block_end_line, insert_line_after, insert_line_before,
        insert_link_into_entry, insert_named_placeholder_after_last_open,
    },
    event_task, markdown,
    pomodoro::pomodoros_section_range,
};

use super::{
    journal::{occurrence_key, OccurrenceState},
    lifecycle::{
        cancel_task_row, line_root, remove_childless_untimed_meet,
        remove_meet_link, row_untouched, split_row_spans, status_char,
        AbsentVerdict, KeptNotice, LearnedSeries, LifecycleItem,
    },
    render::{render_context_row, render_task_row, ContextRowInput},
    stamp::{self, Stamp},
};

/// One included occurrence with its decided rendering.
#[derive(Debug, Clone)]
pub(crate) struct PlannedOccurrence {
    pub calendar_id: String,
    pub event_id: String,
    pub ical_uid: Option<String>,
    pub series_id: Option<String>,
    /// Bare title for block-id minting.
    pub id_hint: String,
    pub task: bool,
    pub time_label: String,
    pub title: String,
    pub html_link: Option<String>,
    pub join_link: Option<String>,
    pub location: Option<String>,
    pub root: Option<String>,
    pub source: Option<String>,
    /// Context-row qualifier (`maybe`, `free`, `day 2 of 2`); tasks ignore it.
    pub qualifier: Option<String>,
    /// Short SHA-256 hashes of each managed span, for the journal.
    pub spans: BTreeMap<String, String>,
    /// Write `[type:: MEET]` (the PT13 suggestion is not MEET).
    pub needs_type: bool,
    /// Phase at sync time; `None` for context rows.
    pub link_phase: Option<LinkPhase>,
    pub all_day: bool,
    pub start_min: i32,
    /// End in minutes from the note day's midnight (may exceed 1440).
    pub end_min: i32,
}

/// How a created Event Task is filed under a MEET Pomodoro.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LinkPhase {
    Past,
    Current,
    Future,
}

/// Ledger state of one MEET Pomodoro this run touched.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MeetEntryState {
    Closed,
    Running,
    Planned,
}

impl MeetEntryState {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Closed => "closed",
            Self::Running => "running",
            Self::Planned => "planned",
        }
    }

    pub(crate) fn is_open(self) -> bool {
        matches!(self, Self::Running | Self::Planned)
    }
}

/// One MEET Pomodoro the plan created or appended links into.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MeetPomodoroEntry {
    pub state: MeetEntryState,
    /// 1-based line in the final contents.
    pub line: usize,
    /// `HHMM-HHMM` for timed entries.
    pub time: Option<String>,
    pub created: bool,
    pub links: usize,
}

/// Pure planner input.
pub(crate) struct SyncPlanInput<'a> {
    pub contents: &'a str,
    pub is_today: bool,
    pub occurrences: Vec<PlannedOccurrence>,
    pub journal: &'a BTreeMap<(String, String), OccurrenceState>,
    pub day_string: String,
    /// Cancel reasons and similar lifecycle notices land under this date.
    pub today: NaiveDate,
    /// Journaled occurrences with no included snapshot row, each already
    /// resolved to a verdict (declined, deleted, moved, left alone).
    pub lifecycle: Vec<LifecycleItem>,
    /// Vault-learned series roots: the planner learns only what the
    /// journal does not already teach.
    pub series_roots:
        &'a BTreeMap<(String, String), super::journal::LearnedRoot>,
    /// Resolved Google account for this run (`snapshot.account`).
    pub account: &'a str,
    /// Clock for the stamp; truncated to the minute when written.
    pub now: NaiveDateTime,
}

/// One row the plan inserts, for the journal and the report.
#[derive(Debug, Clone)]
pub(crate) struct PlannedRow {
    pub calendar_id: String,
    pub event_id: String,
    pub ical_uid: Option<String>,
    pub series_id: Option<String>,
    pub block_id: String,
    pub url: Option<String>,
    pub row_kind: String,
    pub spans: BTreeMap<String, String>,
    pub root: Option<String>,
    pub source: Option<String>,
    pub time_label: String,
    pub title: String,
}

/// One adopted crash-recovery row, for the journal and the report.
#[derive(Debug, Clone)]
pub(crate) struct AdoptedRow {
    pub calendar_id: String,
    pub event_id: String,
    pub ical_uid: Option<String>,
    pub series_id: Option<String>,
    pub block_id: String,
    pub url: Option<String>,
    pub row_kind: String,
    pub spans: BTreeMap<String, String>,
    pub root: Option<String>,
    pub source: Option<String>,
    pub link_adopted: bool,
    pub time_label: String,
    pub title: String,
}

/// One tombstoned journal row, for the journal and the report.
#[derive(Debug, Clone)]
pub(crate) struct GoneRow {
    pub calendar_id: String,
    pub event_id: String,
    pub block_id: String,
    /// Why the row stays gone: `line_removed` for vault edits, or the
    /// Google-side cancel reason for lifecycle removals.
    pub reason: String,
}

/// One cancelled Event Task, for the report.
#[derive(Debug, Clone)]
pub(crate) struct CancelledRow {
    pub time_label: String,
    pub title: String,
    pub reason: String,
}

/// One MEET link the plan appends.
#[derive(Debug, Clone)]
pub(crate) struct PlannedLink {
    pub calendar_id: String,
    pub event_id: String,
    pub block_id: String,
}

/// The pure plan outcome.
#[derive(Debug, Clone, Default)]
pub(crate) struct SyncPlan {
    pub contents: String,
    pub changed: bool,
    pub rows_added: Vec<PlannedRow>,
    pub rows_adopted: Vec<AdoptedRow>,
    pub rows_gone: Vec<GoneRow>,
    pub links_added: Vec<PlannedLink>,
    /// Google-owned spans replaced in place on still-included rows.
    pub rows_updated: Vec<PlannedRow>,
    /// Untouched tasks cancelled for declined, deleted, or moved events.
    pub rows_cancelled: Vec<CancelledRow>,
    /// Untouched context rows removed for declined or deleted events.
    pub rows_removed: Vec<GoneRow>,
    /// Context rows upgraded in place to Event Tasks (Maybe went Yes).
    pub rows_upgraded: Vec<PlannedRow>,
    /// Lifecycle notices reported once (kept edits, worked tasks, …).
    pub kept: Vec<KeptNotice>,
    /// Vault edits that teach a recurring series its root.
    pub learned: Vec<LearnedSeries>,
    /// Upgrade rows that earn a MEET link, as (calendar, event, block).
    pub extra_link_wants: Vec<(String, String, String)>,
    /// Untimed MEET entries left childless by cancels, removed.
    pub entries_removed: usize,
    /// MEET Pomodoros this run created or appended links into, ledger order.
    pub pomodoro_entries: Vec<MeetPomodoroEntry>,
    /// Warnings from MEET filing (running-conflict routing).
    pub warnings: Vec<String>,
    /// Stamp the note carries after the plan, when an Events section
    /// exists.
    pub stamp: Option<Stamp>,
    /// True when this plan wrote the stamp, or would write it under `-d`.
    pub stamp_refreshed: bool,
}

impl SyncPlan {
    /// 1-based line of the first open MEET this run touched.
    #[cfg(test)]
    pub(crate) fn pomodoro_entry_line(&self) -> Option<usize> {
        self.pomodoro_entries
            .iter()
            .find(|entry| entry.state.is_open())
            .map(|entry| entry.line)
    }

    /// Whether the first open MEET this run touched was created here.
    #[cfg(test)]
    pub(crate) fn pomodoro_created(&self) -> bool {
        self.pomodoro_entries
            .iter()
            .find(|entry| entry.state.is_open())
            .is_some_and(|entry| entry.created)
    }
}

pub(crate) fn plan_sync(input: &SyncPlanInput<'_>) -> SyncPlan {
    let mut working = WorkingNote::new(input.contents);
    let mut plan = SyncPlan {
        contents: input.contents.to_string(),
        ..SyncPlan::default()
    };
    let mut used: HashSet<String> = collect_block_ids(&working.lines);

    for occurrence in &input.occurrences {
        let key = occurrence_key(&occurrence.calendar_id, &occurrence.event_id);
        let journaled = input
            .journal
            .get(&(key, input.day_string.clone()))
            .and_then(|state| state.row.as_ref());
        // Context rows without a 📅 URL match by their full rendered
        // text, so URL-less events stay idempotent.
        let text_match = if !occurrence.task && occurrence.url().is_none() {
            Some(render_occurrence(occurrence, ""))
        } else {
            None
        };
        match journaled {
            Some(journaled) => {
                let line_index = find_row_line(
                    &working.lines,
                    working.events_range(),
                    &journaled.block_id,
                    occurrence.url(),
                    occurrence.task,
                    text_match.as_deref(),
                );
                let Some(line_index) = line_index else {
                    plan.rows_gone.push(GoneRow {
                        calendar_id: occurrence.calendar_id.clone(),
                        event_id: occurrence.event_id.clone(),
                        block_id: journaled.block_id.clone(),
                        reason: "line_removed".to_string(),
                    });
                    continue;
                };
                let journaled_kind_is_task = journaled.row_kind == "task";
                if journaled_kind_is_task == occurrence.task {
                    // Learn before span updates: a time change re-slots
                    // the row, invalidating `line_index`.
                    learn_series_root(
                        &working,
                        input,
                        &input.day_string,
                        &mut plan,
                        occurrence,
                        journaled,
                        line_index,
                    );
                    apply_span_updates(
                        input,
                        &mut working,
                        &mut plan,
                        occurrence,
                        journaled,
                        line_index,
                    );
                } else if occurrence.task {
                    // A Maybe context row went Yes: upgrade it in place.
                    upgrade_context_row(
                        input,
                        &mut working,
                        &mut plan,
                        &mut used,
                        occurrence,
                        journaled,
                        line_index,
                    );
                } else {
                    // A Yes task row went Maybe: leave it and report once.
                    let key = occurrence_key(
                        &occurrence.calendar_id,
                        &occurrence.event_id,
                    );
                    let flagged = input
                        .journal
                        .get(&(key, input.day_string.clone()))
                        .is_some_and(|state| state.flags.contains("now_maybe"));
                    if !flagged {
                        plan.kept.push(KeptNotice {
                            calendar_id: occurrence.calendar_id.clone(),
                            event_id: occurrence.event_id.clone(),
                            title: occurrence.title.clone(),
                            detail: "now Maybe; leaving your task row alone"
                                .to_string(),
                            flag: "now_maybe".to_string(),
                        });
                    }
                }
            }
            None => {
                if let Some(line_index) = find_row_line(
                    &working.lines,
                    working.events_range(),
                    "",
                    occurrence.url(),
                    occurrence.task,
                    text_match.as_deref(),
                ) {
                    // Crash recovery: adopt the unjournaled line.
                    let block_id =
                        trailing_block_id(&working.lines[line_index]);
                    let link_adopted = !block_id.is_empty()
                        && pomodoro_link_present(&working.lines, &block_id);
                    if !block_id.is_empty() {
                        used.insert(block_id.clone());
                    }
                    plan.rows_adopted.push(AdoptedRow {
                        calendar_id: occurrence.calendar_id.clone(),
                        event_id: occurrence.event_id.clone(),
                        ical_uid: occurrence.ical_uid.clone(),
                        series_id: occurrence.series_id.clone(),
                        block_id,
                        url: occurrence.html_link.clone(),
                        row_kind: row_kind(occurrence.task),
                        spans: occurrence.spans.clone(),
                        root: occurrence.root.clone(),
                        source: occurrence.source.clone(),
                        link_adopted,
                        time_label: occurrence.time_label.clone(),
                        title: occurrence.title.clone(),
                    });
                    continue;
                }
                let block_id = if occurrence.task {
                    let minted =
                        crate::native::capture_block_ids::mint_block_id(
                            &occurrence.id_hint,
                            &used.iter().map(String::as_str).collect(),
                        );
                    used.insert(minted.clone());
                    minted
                } else {
                    String::new()
                };
                let text = render_occurrence(occurrence, &block_id);
                working.ensure_events_section();
                let slot = insert_slot(
                    &working,
                    occurrence.all_day,
                    occurrence.start_min,
                );
                working.insert_row(slot, &text);
                plan.rows_added.push(PlannedRow {
                    calendar_id: occurrence.calendar_id.clone(),
                    event_id: occurrence.event_id.clone(),
                    ical_uid: occurrence.ical_uid.clone(),
                    series_id: occurrence.series_id.clone(),
                    block_id,
                    url: occurrence.html_link.clone(),
                    row_kind: row_kind(occurrence.task),
                    spans: occurrence.spans.clone(),
                    root: occurrence.root.clone(),
                    source: occurrence.source.clone(),
                    time_label: occurrence.time_label.clone(),
                    title: occurrence.title.clone(),
                });
            }
        }
    }

    if !input.lifecycle.is_empty() {
        apply_lifecycle(input, &mut working, &mut plan);
    }

    working.ensure_events_heading_gap();

    plan_meet_pomodoros(input, &mut working, &mut plan);

    plan_stamp(input, &mut working, &mut plan);

    plan.contents = working.finish();
    // A no-op plan writes nothing and appends nothing: identical
    // contents and no records. A tombstone-only run leaves the note
    // alone but still records `gcal.gone`, so it is not a no-op; kept
    // notices and series learning likewise append journal records.
    let records_empty = plan.rows_added.is_empty()
        && plan.rows_adopted.is_empty()
        && plan.rows_gone.is_empty()
        && plan.rows_removed.is_empty()
        && plan.links_added.is_empty()
        && plan.rows_updated.is_empty()
        && plan.rows_cancelled.is_empty()
        && plan.rows_upgraded.is_empty()
        && plan.kept.is_empty()
        && plan.learned.is_empty();
    plan.changed = plan.contents != input.contents || !records_empty;
    if !plan.changed {
        plan.rows_added.clear();
        plan.rows_adopted.clear();
        plan.rows_gone.clear();
        plan.rows_removed.clear();
        plan.links_added.clear();
        plan.rows_updated.clear();
        plan.rows_cancelled.clear();
        plan.rows_upgraded.clear();
        plan.kept.clear();
        plan.learned.clear();
        plan.extra_link_wants.clear();
        plan.entries_removed = 0;
        plan.pomodoro_entries.clear();
        plan.warnings.clear();
        // Keep `stamp` / `stamp_refreshed` so an unchanged run still
        // reports the line the note already carries.
    }
    plan
}

/// Three-way update of one still-included row: re-render the time,
/// title, and badge spans and replace each span whose current text
/// still matches the journal hash. A span that differs from both the
/// journal and the fresh render is Bryan's edit: it is kept and
/// reported once. The checkbox, `[root::]`, `[type::]`, and block ID
/// are never rewritten.
#[allow(clippy::too_many_arguments)]
fn apply_span_updates(
    input: &SyncPlanInput<'_>,
    working: &mut WorkingNote,
    plan: &mut SyncPlan,
    occurrence: &PlannedOccurrence,
    journaled: &super::journal::RowRecord,
    line_index: usize,
) {
    let current_line =
        working.lines.get(line_index).cloned().unwrap_or_default();
    let Some(mut spans) = split_row_spans(&current_line) else {
        return;
    };
    if spans.task != occurrence.task {
        return;
    }
    let current = spans.hashes();
    let fresh_badges = super::render::badge_text(
        occurrence.html_link.as_deref(),
        occurrence.join_link.as_deref(),
        occurrence.location.as_deref(),
    );
    let fresh = [
        ("time", occurrence.time_label.clone()),
        ("title", occurrence.title.clone()),
        ("badges", fresh_badges),
    ];
    let mut edited: Vec<&str> = Vec::new();
    let mut replaced_time = false;
    let mut hashes_changed = false;
    let mut updated = journaled.spans.clone();
    for (name, fresh_text) in fresh {
        let fresh_hash = super::journal::span_hash(&fresh_text);
        let current_hash = current.get(name).cloned().unwrap_or_default();
        let recorded = journaled.spans.get(name);
        if current_hash == fresh_hash {
            if recorded != Some(&fresh_hash) {
                updated.insert(name.to_string(), fresh_hash);
                hashes_changed = true;
            }
        } else if recorded.is_some_and(|hash| *hash == current_hash) {
            if recorded != Some(&fresh_hash) {
                match name {
                    "time" => {
                        spans.time = fresh_text;
                        replaced_time = true;
                    }
                    "title" => spans.title = fresh_text,
                    _ => spans.badges = fresh_text,
                }
                updated.insert(name.to_string(), fresh_hash);
                hashes_changed = true;
            }
        } else {
            edited.push(name);
        }
    }
    // Context qualifiers (`maybe`, `free`, `day N of M`) track the fresh
    // inclusion while the row is otherwise clean.
    if !occurrence.task
        && spans.qualifier != occurrence.qualifier
        && edited.is_empty()
    {
        spans.qualifier.clone_from(&occurrence.qualifier);
    }
    if !edited.is_empty() {
        let key = occurrence_key(&occurrence.calendar_id, &occurrence.event_id);
        let flagged = input
            .journal
            .get(&(key, input.day_string.clone()))
            .is_some_and(|state| state.flags.contains("kept_edit"));
        if !flagged {
            plan.kept.push(KeptNotice {
                calendar_id: occurrence.calendar_id.clone(),
                event_id: occurrence.event_id.clone(),
                title: occurrence.title.clone(),
                detail: format!("kept your edit to {}", edited.join(" and ")),
                flag: "kept_edit".to_string(),
            });
        }
        return;
    }
    if !hashes_changed
        && (occurrence.task || spans.qualifier == occurrence.qualifier)
    {
        return;
    }
    working.lines[line_index] = spans.rebuild();
    if replaced_time {
        reslot_block(
            working,
            line_index,
            occurrence.all_day,
            occurrence.start_min,
        );
    }
    plan.rows_updated.push(PlannedRow {
        calendar_id: occurrence.calendar_id.clone(),
        event_id: occurrence.event_id.clone(),
        ical_uid: occurrence.ical_uid.clone(),
        series_id: occurrence.series_id.clone(),
        block_id: journaled.block_id.clone(),
        url: occurrence.html_link.clone(),
        row_kind: journaled.row_kind.clone(),
        spans: updated,
        root: journaled.root.clone(),
        source: journaled.source.clone(),
        time_label: occurrence.time_label.clone(),
        title: occurrence.title.clone(),
    });
}

/// Re-slot one row together with its child block at its time-ordered
/// slot after a time change. Never reorders existing lines.
fn reslot_block(
    working: &mut WorkingNote,
    line_index: usize,
    all_day: bool,
    start_min: i32,
) {
    if line_index >= working.lines.len() {
        return;
    }
    let text = working.as_string();
    let spans = line_spans(&text);
    let end = child_block_end_line(&spans, line_index)
        .min(working.lines.len().saturating_sub(1));
    if end < line_index {
        return;
    }
    let block: Vec<String> = working.lines.drain(line_index..=end).collect();
    let slot = insert_slot(working, all_day, start_min);
    working.insert_block(slot, &block);
}

/// Learn a series root from a vault edit: when a journaled task's
/// `[root::]` differs from the root Bob recorded and the event recurs,
/// new rows in that series use the learned root (still validated).
/// A later edit, including a change back, re-teaches the series.
#[allow(clippy::too_many_arguments)]
fn learn_series_root(
    working: &WorkingNote,
    input: &SyncPlanInput<'_>,
    day_string: &str,
    plan: &mut SyncPlan,
    occurrence: &PlannedOccurrence,
    journaled: &super::journal::RowRecord,
    line_index: usize,
) {
    if journaled.row_kind != "task" {
        return;
    }
    let Some(series) = occurrence
        .series_id
        .as_deref()
        .filter(|series| !series.is_empty())
    else {
        return;
    };
    let current_line =
        working.lines.get(line_index).cloned().unwrap_or_default();
    let current_root = line_root(&current_line).unwrap_or_default();
    if current_root.is_empty()
        || journaled.root.as_deref() == Some(current_root.as_str())
    {
        return;
    }
    // The journal already teaches this root: nothing new to learn.
    if input
        .series_roots
        .get(&(occurrence.calendar_id.clone(), series.to_string()))
        .is_some_and(|learned| learned.root == current_root)
    {
        return;
    }
    if plan.learned.iter().any(|learned| {
        learned.calendar_id == occurrence.calendar_id
            && learned.series == series
    }) {
        return;
    }
    plan.learned.push(LearnedSeries {
        calendar_id: occurrence.calendar_id.clone(),
        series: series.to_string(),
        root: current_root,
        title: occurrence.title.clone(),
        day: day_string.to_string(),
        block_id: journaled.block_id.clone(),
    });
}

/// Upgrade one untouched Maybe context row in place to an Event Task
/// row. An edited row is kept and reported once instead.
#[allow(clippy::too_many_arguments)]
fn upgrade_context_row(
    input: &SyncPlanInput<'_>,
    working: &mut WorkingNote,
    plan: &mut SyncPlan,
    used: &mut HashSet<String>,
    occurrence: &PlannedOccurrence,
    journaled: &super::journal::RowRecord,
    line_index: usize,
) {
    let current_line =
        working.lines.get(line_index).cloned().unwrap_or_default();
    let untouched = split_row_spans(&current_line)
        .is_some_and(|spans| !spans.task && spans.hashes() == journaled.spans);
    if !untouched {
        let key = occurrence_key(&occurrence.calendar_id, &occurrence.event_id);
        let flagged = input
            .journal
            .get(&(key, input.day_string.clone()))
            .is_some_and(|state| state.flags.contains("kept_edit"));
        if !flagged {
            plan.kept.push(KeptNotice {
                calendar_id: occurrence.calendar_id.clone(),
                event_id: occurrence.event_id.clone(),
                title: occurrence.title.clone(),
                detail: "kept your edit while Maybe went Yes".to_string(),
                flag: "kept_edit".to_string(),
            });
        }
        return;
    }
    let block_id = crate::native::capture_block_ids::mint_block_id(
        &occurrence.id_hint,
        &used.iter().map(String::as_str).collect(),
    );
    used.insert(block_id.clone());
    working.lines[line_index] = render_occurrence(occurrence, &block_id);
    plan.rows_upgraded.push(PlannedRow {
        calendar_id: occurrence.calendar_id.clone(),
        event_id: occurrence.event_id.clone(),
        ical_uid: occurrence.ical_uid.clone(),
        series_id: occurrence.series_id.clone(),
        block_id: block_id.clone(),
        url: occurrence.html_link.clone(),
        row_kind: "task".to_string(),
        spans: occurrence.spans.clone(),
        root: occurrence.root.clone(),
        source: occurrence.source.clone(),
        time_label: occurrence.time_label.clone(),
        title: occurrence.title.clone(),
    });
    plan.extra_link_wants.push((
        occurrence.calendar_id.clone(),
        occurrence.event_id.clone(),
        block_id,
    ));
}

/// Reconcile journaled occurrences that have no included snapshot row:
/// cancel untouched tasks for declined, deleted, or moved events (with
/// a created CANCEL LOG), remove untouched context rows, and report
/// everything else once. A worked task is never cancelled.
fn apply_lifecycle(
    input: &SyncPlanInput<'_>,
    working: &mut WorkingNote,
    plan: &mut SyncPlan,
) {
    let mut links_removed = false;
    for item in &input.lifecycle {
        let key = occurrence_key(&item.calendar_id, &item.event_id);
        let state = input.journal.get(&(key, input.day_string.clone()));
        let Some(journaled) = state.and_then(|state| state.row.as_ref()) else {
            continue;
        };
        let is_task = journaled.row_kind == "task";
        let line_index = find_row_line(
            &working.lines,
            working.events_range(),
            &journaled.block_id,
            journaled.url.as_deref(),
            is_task,
            None,
        );
        let Some(line_index) = line_index else {
            // Already gone from the note: tombstone it so the event
            // never comes back, recording why it stays gone.
            plan.rows_gone.push(GoneRow {
                calendar_id: item.calendar_id.clone(),
                event_id: item.event_id.clone(),
                block_id: journaled.block_id.clone(),
                reason: item
                    .verdict
                    .cancel_reason()
                    .unwrap_or_else(|| "line_removed".to_string()),
            });
            continue;
        };
        let current_line =
            working.lines.get(line_index).cloned().unwrap_or_default();
        match &item.verdict {
            AbsentVerdict::Deleted
            | AbsentVerdict::Declined
            | AbsentVerdict::Moved(_) => {
                let reason =
                    item.verdict.cancel_reason().unwrap_or_else(|| {
                        "Deleted in Google Calendar".to_string()
                    });
                if is_task {
                    cancel_lifecycle_task(
                        input,
                        working,
                        plan,
                        &item.calendar_id,
                        &item.event_id,
                        journaled,
                        line_index,
                        &current_line,
                        &reason,
                        &mut links_removed,
                    );
                } else {
                    remove_lifecycle_context(
                        input,
                        working,
                        plan,
                        &item.calendar_id,
                        &item.event_id,
                        journaled,
                        line_index,
                        &current_line,
                        &reason,
                    );
                }
            }
            AbsentVerdict::LeftAlone(detail) => {
                let flagged =
                    state.is_some_and(|state| state.flags.contains(detail));
                if !flagged {
                    plan.kept.push(KeptNotice {
                        calendar_id: item.calendar_id.clone(),
                        event_id: item.event_id.clone(),
                        title: journaled_title(&current_line),
                        detail: detail.clone(),
                        flag: detail.clone(),
                    });
                }
            }
        }
    }
    if links_removed {
        let (contents, removed) =
            remove_childless_untimed_meet(&working.as_string());
        if removed > 0 {
            working.replace(&contents);
            plan.entries_removed = removed;
        }
    }
}

/// Cancel one untouched lifecycle task in place: mark it cancelled
/// with today's date, create the CANCEL LOG when the task has none,
/// and drop its MEET link. A worked task is left alone and reported
/// once instead.
#[allow(clippy::too_many_arguments)]
fn cancel_lifecycle_task(
    input: &SyncPlanInput<'_>,
    working: &mut WorkingNote,
    plan: &mut SyncPlan,
    calendar_id: &str,
    event_id: &str,
    journaled: &super::journal::RowRecord,
    line_index: usize,
    current_line: &str,
    reason: &str,
    links_removed: &mut bool,
) {
    if status_char(current_line) == Some('-') {
        return;
    }
    if !row_untouched(&working.as_string(), line_index, &journaled.block_id) {
        let key = occurrence_key(calendar_id, event_id);
        let flagged = input
            .journal
            .get(&(key, input.day_string.clone()))
            .is_some_and(|state| state.flags.contains("worked_kept"));
        if !flagged {
            plan.kept.push(KeptNotice {
                calendar_id: calendar_id.to_string(),
                event_id: event_id.to_string(),
                title: journaled_title(current_line),
                detail: format!("left your worked task alone ({reason})"),
                flag: "worked_kept".to_string(),
            });
        }
        return;
    }
    let Some(cancelled) =
        cancel_task_row(&working.as_string(), line_index, input.today, reason)
    else {
        return;
    };
    working.replace(&cancelled);
    let (contents, removed) =
        remove_meet_link(&working.as_string(), &journaled.block_id);
    if removed {
        working.replace(&contents);
        *links_removed = true;
    }
    let spans = split_row_spans(
        working
            .lines
            .get(line_index)
            .map(String::as_str)
            .unwrap_or_default(),
    );
    plan.rows_cancelled.push(CancelledRow {
        time_label: spans
            .as_ref()
            .map(|spans| spans.time.clone())
            .unwrap_or_default(),
        title: journaled_title(current_line),
        reason: reason.to_string(),
    });
}

/// Remove one untouched lifecycle context row for a declined, deleted,
/// or moved event. A row with child bullets (Bryan's notes) or with
/// edited spans is kept and reported once instead.
#[allow(clippy::too_many_arguments)]
fn remove_lifecycle_context(
    input: &SyncPlanInput<'_>,
    working: &mut WorkingNote,
    plan: &mut SyncPlan,
    calendar_id: &str,
    event_id: &str,
    journaled: &super::journal::RowRecord,
    line_index: usize,
    current_line: &str,
    reason: &str,
) {
    let spans = split_row_spans(current_line);
    let untouched = spans
        .as_ref()
        .is_some_and(|spans| spans.hashes() == journaled.spans);
    if !untouched {
        push_kept_once(
            input,
            plan,
            calendar_id,
            event_id,
            journaled_title(current_line),
            format!("kept your edited context row ({reason})"),
            "kept_edit",
        );
        return;
    }
    let text = working.as_string();
    let line_spans = line_spans(&text);
    if child_block_end_line(&line_spans, line_index) != line_index {
        push_kept_once(
            input,
            plan,
            calendar_id,
            event_id,
            journaled_title(current_line),
            "kept a context row with your notes".to_string(),
            "kept_edit",
        );
        return;
    }
    working.lines.remove(line_index);
    plan.rows_removed.push(GoneRow {
        calendar_id: calendar_id.to_string(),
        event_id: event_id.to_string(),
        block_id: journaled.block_id.clone(),
        reason: reason.to_string(),
    });
}

/// Report one lifecycle notice once per (occurrence, day).
fn push_kept_once(
    input: &SyncPlanInput<'_>,
    plan: &mut SyncPlan,
    calendar_id: &str,
    event_id: &str,
    title: String,
    detail: String,
    flag: &str,
) {
    let key = occurrence_key(calendar_id, event_id);
    let flagged = input
        .journal
        .get(&(key, input.day_string.clone()))
        .is_some_and(|state| state.flags.contains(flag));
    if !flagged {
        plan.kept.push(KeptNotice {
            calendar_id: calendar_id.to_string(),
            event_id: event_id.to_string(),
            title,
            detail,
            flag: flag.to_string(),
        });
    }
}

/// A row's display title: the span title, or the raw line when the row
/// no longer parses.
pub(crate) fn journaled_title(line: &str) -> String {
    split_row_spans(line)
        .map(|spans| spans.title.clone())
        .filter(|title| !title.is_empty())
        .unwrap_or_else(|| line.trim().to_string())
}

impl PlannedOccurrence {
    fn url(&self) -> Option<&str> {
        self.html_link.as_deref()
    }
}

fn row_kind(task: bool) -> String {
    if task {
        "task".to_string()
    } else {
        "context".to_string()
    }
}

fn render_occurrence(occurrence: &PlannedOccurrence, block_id: &str) -> String {
    if occurrence.task {
        render_task_row(
            &super::render::TaskRowInput {
                time_label: &occurrence.time_label,
                title: &occurrence.title,
                html_link: occurrence.html_link.as_deref(),
                join_link: occurrence.join_link.as_deref(),
                location: occurrence.location.as_deref(),
                root: occurrence.root.as_deref(),
                needs_type: occurrence.needs_type,
                status: status_for_phase(occurrence.link_phase),
            },
            block_id,
        )
    } else {
        render_context_row(&ContextRowInput {
            time_label: &occurrence.time_label,
            title: &occurrence.title,
            html_link: occurrence.html_link.as_deref(),
            qualifier: occurrence.qualifier.as_deref(),
        })
    }
}

/// Owned working copy of the note: owned lines plus the input's
/// trailing-newline state, so an unchanged plan is byte-identical.
struct WorkingNote {
    lines: Vec<String>,
    trailing_newline: bool,
}

impl WorkingNote {
    fn new(contents: &str) -> Self {
        Self {
            lines: contents.lines().map(str::to_string).collect(),
            trailing_newline: contents.ends_with('\n'),
        }
    }

    fn replace(&mut self, contents: &str) {
        self.lines = contents.lines().map(str::to_string).collect();
        self.trailing_newline = contents.ends_with('\n');
    }

    fn as_string(&self) -> String {
        self.finish()
    }

    fn finish(&self) -> String {
        let mut out = self.lines.join("\n");
        if self.trailing_newline && !self.lines.is_empty() {
            out.push('\n');
        }
        out
    }

    fn line_refs(&self) -> Vec<&str> {
        self.lines.iter().map(String::as_str).collect()
    }

    fn events_range(&self) -> Option<Range<usize>> {
        let refs = self.line_refs();
        event_task::events_section_range(&refs)
    }

    /// Create `## Events` immediately above `## Pomodoros`, with one
    /// blank line under the heading. Callers guarantee the Pomodoros
    /// section exists. (Section ranges start at the body, so the heading
    /// sits one line above `pomodoros.start`.)
    fn ensure_events_section(&mut self) {
        if self.events_range().is_some() {
            return;
        }
        let refs = self.line_refs();
        let pomodoros = pomodoros_section_range(&refs).expect("Pomodoros");
        let mut insert_at = pomodoros.start.saturating_sub(1);
        while insert_at > 0 && self.lines[insert_at - 1].trim().is_empty() {
            insert_at -= 1;
        }
        let mut updated: Vec<String> = self.lines[..insert_at].to_vec();
        updated.push("## Events".to_string());
        updated.push(String::new());
        let mut tail: Vec<String> = self.lines[insert_at..].to_vec();
        while tail.first().is_some_and(|line| line.trim().is_empty()) {
            tail.remove(0);
        }
        updated.append(&mut tail);
        self.lines = updated;
    }

    fn insert_row(&mut self, slot: Option<usize>, text: &str) {
        match slot {
            Some(after) => self.lines.insert(after + 1, text.to_string()),
            None => {
                let index = self.events_top_index();
                self.lines.insert(index, text.to_string());
            }
        }
    }

    fn insert_block(&mut self, slot: Option<usize>, block: &[String]) {
        match slot {
            Some(after) => {
                for (offset, line) in block.iter().enumerate() {
                    self.lines.insert(after + 1 + offset, line.clone());
                }
            }
            None => {
                let start = self.events_top_index();
                for (offset, line) in block.iter().enumerate() {
                    self.lines.insert(start + offset, line.clone());
                }
            }
        }
    }

    /// Return the insertion point for a row that sorts before every other
    /// row, keeping it below the heading gap, below a recognized stamp,
    /// and together with the list.
    fn events_top_index(&self) -> usize {
        let range = self.events_range().expect("Events section exists");
        let refs = self.line_refs();
        let stamps: HashSet<usize> =
            stamp::recognize(&refs, range.clone()).into_iter().collect();
        if let Some(index) = range.clone().find(|index| {
            !self.lines[*index].trim().is_empty() && !stamps.contains(index)
        }) {
            return index;
        }
        if let Some(last) = stamps.iter().max().copied() {
            return last + 1;
        }
        if range.len() >= 2 {
            range.start + 1
        } else {
            range.start
        }
    }

    /// Ensure a non-empty Events section has at least one blank line below
    /// its heading. Existing and extra blank lines are preserved.
    fn ensure_events_heading_gap(&mut self) {
        let Some(range) = self.events_range() else {
            return;
        };
        if range.start < range.end && !self.lines[range.start].trim().is_empty()
        {
            self.lines.insert(range.start, String::new());
        }
    }
}

fn collect_block_ids(lines: &[String]) -> HashSet<String> {
    let mut used = HashSet::new();
    for line in lines {
        if let Some(id) = parse_trailing_block_id(line) {
            used.insert(id);
        }
    }
    used
}

fn parse_trailing_block_id(line: &str) -> Option<String> {
    let trimmed = line.trim_end();
    let caret = trimmed.rfind('^')?;
    let id = trimmed.get(caret + 1..)?;
    if id.is_empty()
        || !id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        || (caret > 0
            && !trimmed[..caret]
                .chars()
                .next_back()
                .is_some_and(char::is_whitespace))
    {
        return None;
    }
    Some(id.to_string())
}

fn trailing_block_id(line: &str) -> String {
    parse_trailing_block_id(line).unwrap_or_default()
}

/// Find a row line inside the Events section: a task matches its
/// journaled `^block_id` first, then its `📅` URL; a context row
/// matches its `📅` URL, or its full rendered text when it has none.
fn find_row_line(
    lines: &[String],
    events_range: Option<Range<usize>>,
    block_id: &str,
    url: Option<&str>,
    task: bool,
    text_match: Option<&str>,
) -> Option<usize> {
    let range = events_range?;
    if task && !block_id.is_empty() {
        let suffix = format!("^{block_id}");
        for index in range.clone() {
            let line = lines.get(index)?.trim_end();
            if line.strip_suffix(suffix.as_str()).is_some_and(|before| {
                before.chars().last().is_some_and(char::is_whitespace)
            }) {
                return Some(index);
            }
        }
    }
    if let Some(url) = url.filter(|url| !url.is_empty()) {
        for index in range.clone() {
            if lines.get(index).is_some_and(|line| line.contains(url)) {
                return Some(index);
            }
        }
    }
    if let Some(text) = text_match.filter(|text| !text.is_empty()) {
        for index in range {
            if lines.get(index).is_some_and(|line| line.trim_end() == text) {
                return Some(index);
            }
        }
    }
    None
}

fn pomodoro_link_present(lines: &[String], block_id: &str) -> bool {
    let link = format!("[[#^{block_id}]]");
    let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
    let range = match pomodoros_section_range(&refs) {
        Some(range) => range,
        None => return false,
    };
    range
        .filter_map(|index| refs.get(index))
        .any(|line| line.contains(link.as_str()))
}

/// Time-ordered slot: the 0-based line to insert after, or `None` for
/// the top of the section. After the last parsable row that starts at
/// or before the new row; all-day rows sort before timed rows.
fn insert_slot(
    working: &WorkingNote,
    all_day: bool,
    start_min: i32,
) -> Option<usize> {
    let range = working.events_range().expect("Events section exists");
    let key = (!all_day, start_min);
    let current = working.as_string();
    let spans = line_spans(&current);
    let mut slot: Option<usize> = None;
    for index in range {
        let line = &working.lines[index];
        if line.trim().is_empty() {
            continue;
        }
        let Some(row_key) = row_sort_key(line) else {
            continue;
        };
        if row_key <= key {
            slot = Some(child_block_end_line(&spans, index));
        } else {
            break;
        }
    }
    slot
}

/// Sort key for one Events row: all-day rows sort before timed rows
/// (the inverted flag sorts `false` first), then by start minute.
/// Unparsable lines are skipped by the caller.
fn row_sort_key(line: &str) -> Option<(bool, i32)> {
    if let Some(span) = event_task::parse_event_span(line) {
        return Some((true, i32::from(span.start_min)));
    }
    let body = line.trim_start_matches([' ', '\t']).strip_prefix("- ")?;
    let body = body.strip_prefix("**")?;
    let raw = body.get(..body.find("**")?)?;
    if raw == "all day" {
        return Some((false, 0));
    }
    if raw.len() == 9 && raw.as_bytes().get(4) == Some(&b'-') {
        let start = parse_hhmm(&raw[..4])?;
        return Some((true, i32::from(start)));
    }
    None
}

fn parse_hhmm(value: &str) -> Option<u16> {
    if value.len() != 4 || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let hour = value[..2].parse::<u16>().ok()?;
    let minute = value[2..].parse::<u16>().ok()?;
    if hour > 23 || minute > 59 {
        return None;
    }
    Some(hour * 60 + minute)
}

/// Write or refresh the Events stamp as the last planning step.
/// A stamp whose minute is not this run's minute is due.
fn plan_stamp(
    input: &SyncPlanInput<'_>,
    working: &mut WorkingNote,
    plan: &mut SyncPlan,
) {
    let Some(range) = working.events_range() else {
        plan.stamp = None;
        plan.stamp_refreshed = false;
        return;
    };
    let note_day = note_day(input);
    let now = stamp::truncate_to_minute(input.now);
    let other_changed = working.finish() != input.contents;
    let (due, mut stamps) = {
        let refs = working.line_refs();
        let due =
            other_changed || stamp::is_due(&refs, range.clone(), now, note_day);
        (due, stamp::recognize(&refs, range.clone()))
    };
    if !due {
        plan.stamp = stamps
            .first()
            .and_then(|&index| stamp::parse(&working.lines[index], note_day));
        plan.stamp_refreshed = false;
        return;
    }

    let mut meet_0: Vec<usize> = plan
        .pomodoro_entries
        .iter()
        .map(|entry| entry.line - 1)
        .collect();
    let pre_canonical = {
        let refs = working.line_refs();
        stamp::canonical_slot(&refs, range.clone())
    };
    if stamps.len() == 1 && stamps[0] == pre_canonical {
        let anchor = stamps[0];
        shift_meet_0(&mut meet_0, |line| anchor < line, -1);
        working.lines.remove(anchor);
        let need_blank_after = working.lines.get(anchor).is_some_and(|line| {
            !line.trim().is_empty()
                && !stamp::is_list_item(line)
                && markdown::atx_heading(line).is_none()
        });
        let rendered = stamp::render(input.account, now, note_day);
        working.lines.insert(anchor, rendered);
        shift_meet_0(&mut meet_0, |line| anchor <= line, 1);
        if need_blank_after {
            working.lines.insert(anchor + 1, String::new());
            shift_meet_0(&mut meet_0, |line| anchor < line, 1);
        }
        write_meet_lines(plan, &meet_0);
        plan.stamp = Some(Stamp {
            account: input.account.to_string(),
            synced_at: now,
        });
        plan.stamp_refreshed = true;
        return;
    }
    stamps.sort_unstable();
    for index in stamps.into_iter().rev() {
        shift_meet_0(&mut meet_0, |line| index < line, -1);
        working.lines.remove(index);
    }

    let range = working.events_range().expect("Events section exists");
    let mut slot = {
        let refs = working.line_refs();
        stamp::canonical_slot(&refs, range.clone())
    };
    if slot == range.start {
        working.lines.insert(slot, String::new());
        shift_meet_0(&mut meet_0, |line| slot <= line, 1);
        slot += 1;
    }
    let need_blank_after = working.lines.get(slot).is_some_and(|line| {
        !line.trim().is_empty()
            && !stamp::is_list_item(line)
            && markdown::atx_heading(line).is_none()
    });
    let rendered = stamp::render(input.account, now, note_day);
    working.lines.insert(slot, rendered);
    shift_meet_0(&mut meet_0, |line| slot <= line, 1);
    if need_blank_after {
        working.lines.insert(slot + 1, String::new());
        shift_meet_0(&mut meet_0, |line| slot < line, 1);
    }
    write_meet_lines(plan, &meet_0);
    plan.stamp = Some(Stamp {
        account: input.account.to_string(),
        synced_at: now,
    });
    plan.stamp_refreshed = true;
}

fn shift_meet_0(lines: &mut [usize], pred: impl Fn(usize) -> bool, delta: i32) {
    for line in lines {
        if pred(*line) {
            if delta < 0 {
                *line = line.saturating_sub(delta.unsigned_abs() as usize);
            } else {
                *line = line.saturating_add(delta as usize);
            }
        }
    }
}

fn write_meet_lines(plan: &mut SyncPlan, lines: &[usize]) {
    for (entry, line) in plan.pomodoro_entries.iter_mut().zip(lines) {
        entry.line = line + 1;
    }
}

fn note_day(input: &SyncPlanInput<'_>) -> NaiveDate {
    NaiveDate::parse_from_str(&input.day_string, "%Y-%m-%d")
        .unwrap_or(input.today)
}

/// File each eligible Event Task under a past, running, or planned MEET.
fn plan_meet_pomodoros(
    input: &SyncPlanInput<'_>,
    working: &mut WorkingNote,
    plan: &mut SyncPlan,
) {
    let wanted = collect_wanted_links(input, working, plan);
    if wanted.is_empty() {
        return;
    }
    let mut contents = working.as_string();
    let mut touched: Vec<TouchedMeet> = Vec::new();
    let mut past: Vec<WantedLink> = Vec::new();
    let mut current: Vec<WantedLink> = Vec::new();
    let mut future: Vec<WantedLink> = Vec::new();
    for link in wanted {
        match link.phase {
            LinkPhase::Past => past.push(link),
            LinkPhase::Current => current.push(link),
            LinkPhase::Future => future.push(link),
        }
    }

    for cluster in cluster_past(past) {
        let start_min =
            cluster.iter().map(|link| link.start_min).min().unwrap_or(0);
        let end_min =
            cluster.iter().map(|link| link.end_min).max().unwrap_or(0);
        let (span, time) = format_meet_span(start_min, end_min);
        let line_text = format!("- [x] {span} — MEET");
        let at = closed_insert_at(&contents, start_min);
        let (updated, line_0) = insert_ledger_line(&contents, at, &line_text);
        contents = updated;
        shift_touched(&mut touched, line_0);
        let added = append_links(
            &mut contents,
            line_0,
            &cluster,
            true,
            plan,
            &mut touched,
        );
        if added > 0 {
            touched.push(TouchedMeet {
                state: MeetEntryState::Closed,
                line_0,
                time: Some(time),
                created: true,
                links: added,
            });
        }
    }

    let mut routed_future = future;
    if !current.is_empty() {
        let targets = open_meet_targets(&contents);
        if let Some(running) = targets.running_meet() {
            let added = append_links(
                &mut contents,
                running,
                &current,
                false,
                plan,
                &mut touched,
            );
            record_or_bump(
                &mut touched,
                MeetEntryState::Running,
                running,
                targets.running_time.clone(),
                false,
                added,
            );
        } else if targets.single_timed.is_some() || targets.several_timed {
            let label = targets.running_label();
            for link in &current {
                plan.warnings.push(format!(
                    "{} is running now, but {label} is the running Pomodoro; \
                     linked it under the MEET placeholder",
                    link.title
                ));
            }
            routed_future.splice(0..0, current);
        } else if let Some(placeholder) = targets.placeholder {
            let added = append_links(
                &mut contents,
                placeholder,
                &current,
                false,
                plan,
                &mut touched,
            );
            record_or_bump(
                &mut touched,
                MeetEntryState::Planned,
                placeholder,
                None,
                false,
                added,
            );
        } else {
            let start_min =
                current.iter().map(|link| link.start_min).min().unwrap_or(0);
            let end_min =
                current.iter().map(|link| link.end_min).max().unwrap_or(0);
            let (span, time) = format_meet_span(start_min, end_min);
            let line_text = format!("- [ ] {span} — MEET");
            let at = running_insert_at(&contents);
            let (updated, line_0) =
                insert_ledger_line(&contents, at, &line_text);
            contents = updated;
            shift_touched(&mut touched, line_0);
            let added = append_links(
                &mut contents,
                line_0,
                &current,
                false,
                plan,
                &mut touched,
            );
            if added > 0 {
                touched.push(TouchedMeet {
                    state: MeetEntryState::Running,
                    line_0,
                    time: Some(time),
                    created: true,
                    links: added,
                });
            }
        }
    }

    if !routed_future.is_empty() {
        let targets = open_meet_targets(&contents);
        let (entry_line, state, time, created) =
            if let Some(running) = targets.running_meet() {
                (
                    running,
                    MeetEntryState::Running,
                    targets.running_time.clone(),
                    false,
                )
            } else if let Some(placeholder) = targets.placeholder {
                (placeholder, MeetEntryState::Planned, None, false)
            } else {
                let (updated, created_line, _) =
                    insert_named_placeholder_after_last_open(&contents, "MEET")
                        .expect("Pomodoros section exists");
                contents = updated;
                shift_touched(&mut touched, created_line);
                (created_line, MeetEntryState::Planned, None, true)
            };
        let added = append_links(
            &mut contents,
            entry_line,
            &routed_future,
            false,
            plan,
            &mut touched,
        );
        record_or_bump(&mut touched, state, entry_line, time, created, added);
    }

    if !plan.links_added.is_empty() {
        working.replace(&contents);
        touched.sort_by_key(|entry| entry.line_0);
        plan.pomodoro_entries = touched
            .into_iter()
            .filter(|entry| entry.links > 0)
            .map(|entry| MeetPomodoroEntry {
                state: entry.state,
                line: entry.line_0 + 1,
                time: entry.time,
                created: entry.created,
                links: entry.links,
            })
            .collect();
    }
}

#[derive(Debug, Clone)]
struct WantedLink {
    calendar_id: String,
    event_id: String,
    block_id: String,
    phase: LinkPhase,
    start_min: i32,
    end_min: i32,
    title: String,
}

#[derive(Debug, Clone)]
struct TouchedMeet {
    state: MeetEntryState,
    line_0: usize,
    time: Option<String>,
    created: bool,
    links: usize,
}

enum LineInsert {
    Before(usize),
    After(usize),
}

struct OpenMeetTargets {
    single_timed: Option<usize>,
    single_timed_name: Option<String>,
    several_timed: bool,
    placeholder: Option<usize>,
    running_time: Option<String>,
}

impl OpenMeetTargets {
    fn running_meet(&self) -> Option<usize> {
        let line = self.single_timed?;
        (self.single_timed_name.as_deref() == Some("MEET")).then_some(line)
    }

    fn running_label(&self) -> String {
        if self.several_timed {
            return "multiple timed Pomodoros".to_string();
        }
        self.single_timed_name
            .clone()
            .unwrap_or_else(|| "an unnamed Pomodoro".to_string())
    }
}

fn collect_wanted_links(
    input: &SyncPlanInput<'_>,
    working: &WorkingNote,
    plan: &SyncPlan,
) -> Vec<WantedLink> {
    let mut wanted: Vec<WantedLink> = Vec::new();
    for occurrence in &input.occurrences {
        let Some(phase) = occurrence.link_phase else {
            continue;
        };
        if !occurrence.task {
            continue;
        }
        let created = row_was_created(plan, occurrence);
        if !created && (!input.is_today || matches!(phase, LinkPhase::Past)) {
            continue;
        }
        let key = occurrence_key(&occurrence.calendar_id, &occurrence.event_id);
        if let Some(state) =
            input.journal.get(&(key.clone(), input.day_string.clone()))
            && (state.link || state.gone)
        {
            continue;
        }
        let Some(block_id) =
            journaled_or_planned_block_id(input, plan, occurrence)
                .filter(|id| !id.is_empty())
        else {
            continue;
        };
        if wanted.iter().any(|link| link.block_id == block_id) {
            continue;
        }
        let refs = working.line_refs();
        if find_row_line(
            &working.lines,
            event_task::events_section_range(&refs),
            &block_id,
            occurrence.url(),
            true,
            None,
        )
        .is_none()
        {
            continue;
        }
        wanted.push(WantedLink {
            calendar_id: occurrence.calendar_id.clone(),
            event_id: occurrence.event_id.clone(),
            block_id,
            phase,
            start_min: occurrence.start_min,
            end_min: occurrence.end_min,
            title: occurrence.title.clone(),
        });
    }
    for (calendar_id, event_id, block_id) in &plan.extra_link_wants {
        if wanted.iter().any(|link| link.block_id == *block_id) {
            continue;
        }
        let key = occurrence_key(calendar_id, event_id);
        if let Some(state) = input.journal.get(&(key, input.day_string.clone()))
            && (state.link || state.gone)
        {
            continue;
        }
        let refs = working.line_refs();
        if find_row_line(
            &working.lines,
            event_task::events_section_range(&refs),
            block_id,
            None,
            true,
            None,
        )
        .is_none()
        {
            continue;
        }
        let Some(occurrence) = input.occurrences.iter().find(|occurrence| {
            occurrence.calendar_id == *calendar_id
                && occurrence.event_id == *event_id
        }) else {
            continue;
        };
        let Some(phase) = occurrence.link_phase else {
            continue;
        };
        wanted.push(WantedLink {
            calendar_id: calendar_id.clone(),
            event_id: event_id.clone(),
            block_id: block_id.clone(),
            phase,
            start_min: occurrence.start_min,
            end_min: occurrence.end_min,
            title: occurrence.title.clone(),
        });
    }
    wanted
}

fn row_was_created(plan: &SyncPlan, occurrence: &PlannedOccurrence) -> bool {
    plan.rows_added.iter().any(|row| {
        row.calendar_id == occurrence.calendar_id
            && row.event_id == occurrence.event_id
            && row.row_kind == "task"
    }) || plan.rows_upgraded.iter().any(|row| {
        row.calendar_id == occurrence.calendar_id
            && row.event_id == occurrence.event_id
    })
}

fn cluster_past(mut items: Vec<WantedLink>) -> Vec<Vec<WantedLink>> {
    items.sort_by(|left, right| {
        (left.start_min, left.end_min, left.block_id.as_str()).cmp(&(
            right.start_min,
            right.end_min,
            right.block_id.as_str(),
        ))
    });
    let mut clusters: Vec<Vec<WantedLink>> = Vec::new();
    for item in items {
        if let Some(last) = clusters.last_mut() {
            let cluster_end =
                last.iter().map(|link| link.end_min).max().unwrap_or(0);
            if item.start_min <= cluster_end {
                last.push(item);
                continue;
            }
        }
        clusters.push(vec![item]);
    }
    clusters
}

fn format_meet_span(start_min: i32, end_min: i32) -> (String, String) {
    let duration = (end_min - start_min).max(0);
    let start_mod = start_min.rem_euclid(1440);
    let end_mod = end_min.rem_euclid(1440);
    let start = format!("{:02}{:02}", start_mod / 60, start_mod % 60);
    let end = format!("{:02}{:02}", end_mod / 60, end_mod % 60);
    (
        format!("(**{start}-{end}** [t:: {duration}m])"),
        format!("{start}-{end}"),
    )
}

fn status_for_phase(phase: Option<LinkPhase>) -> char {
    match phase {
        Some(LinkPhase::Past) => '/',
        Some(LinkPhase::Current | LinkPhase::Future) => '*',
        None => ' ',
    }
}

fn closed_insert_at(contents: &str, cluster_start: i32) -> LineInsert {
    let scan = capture_pomodoros::scan(contents);
    let lines = line_spans(contents);
    let line_text: Vec<&str> = lines.iter().map(|line| line.text).collect();
    let section =
        pomodoros_section_range(&line_text).expect("Pomodoros section exists");
    let completed: Vec<&PomodoroEntry> = scan
        .entries
        .iter()
        .filter(|entry| entry.state == PomodoroState::Completed)
        .collect();
    if let Some(last) = completed
        .iter()
        .rev()
        .find(|entry| entry_start_min(entry) <= cluster_start)
    {
        let end = child_block_end_line(&lines, last.line - 1);
        return LineInsert::After(end);
    }
    if let Some(first) = completed.first() {
        return LineInsert::Before(first.line - 1);
    }
    if let Some(first_open) = scan
        .entries
        .iter()
        .find(|entry| entry.state == PomodoroState::Open)
    {
        return LineInsert::Before(first_open.line - 1);
    }
    LineInsert::Before(section.start)
}

fn running_insert_at(contents: &str) -> LineInsert {
    let scan = capture_pomodoros::scan(contents);
    let lines = line_spans(contents);
    let line_text: Vec<&str> = lines.iter().map(|line| line.text).collect();
    let section =
        pomodoros_section_range(&line_text).expect("Pomodoros section exists");
    if let Some(last) = scan
        .entries
        .iter()
        .rfind(|entry| entry.state == PomodoroState::Completed)
    {
        let end = child_block_end_line(&lines, last.line - 1);
        return LineInsert::After(end);
    }
    if let Some(first_open) = scan
        .entries
        .iter()
        .find(|entry| entry.state == PomodoroState::Open)
    {
        return LineInsert::Before(first_open.line - 1);
    }
    LineInsert::Before(section.start)
}

fn entry_start_min(entry: &PomodoroEntry) -> i32 {
    entry
        .time_range
        .as_deref()
        .and_then(|range| range.split_once('-'))
        .and_then(|(start, _)| parse_hhmm(start).map(i32::from))
        .unwrap_or(0)
}

fn insert_ledger_line(
    contents: &str,
    at: LineInsert,
    text: &str,
) -> (String, usize) {
    let lines = line_spans(contents);
    match at {
        LineInsert::Before(index) if index < lines.len() => {
            (insert_line_before(contents, &lines, index, text), index)
        }
        LineInsert::Before(_) if !lines.is_empty() => {
            let after = lines.len() - 1;
            (insert_line_after(contents, &lines, after, text), after + 1)
        }
        LineInsert::After(index) => {
            (insert_line_after(contents, &lines, index, text), index + 1)
        }
        LineInsert::Before(_) => (format!("{text}\n"), 0),
    }
}

fn shift_touched(touched: &mut [TouchedMeet], new_line: usize) {
    for entry in touched {
        if entry.line_0 >= new_line {
            entry.line_0 += 1;
        }
    }
}

fn append_links(
    contents: &mut String,
    entry_line: usize,
    links: &[WantedLink],
    tomato: bool,
    plan: &mut SyncPlan,
    touched: &mut [TouchedMeet],
) -> usize {
    let mut added = 0;
    for link in links {
        let body = if tomato {
            format!("🍅 [[#^{}]]", link.block_id)
        } else {
            format!("[[#^{}]]", link.block_id)
        };
        let spans = line_spans(contents);
        let end = child_block_end_line(&spans, entry_line);
        let (updated, already, _) =
            insert_link_into_entry(contents, &spans, entry_line, &body);
        if already {
            continue;
        }
        *contents = updated;
        shift_touched(touched, end + 1);
        plan.links_added.push(PlannedLink {
            calendar_id: link.calendar_id.clone(),
            event_id: link.event_id.clone(),
            block_id: link.block_id.clone(),
        });
        added += 1;
    }
    added
}

fn record_or_bump(
    touched: &mut Vec<TouchedMeet>,
    state: MeetEntryState,
    line_0: usize,
    time: Option<String>,
    created: bool,
    added: usize,
) {
    if added == 0 {
        return;
    }
    if let Some(existing) =
        touched.iter_mut().find(|entry| entry.line_0 == line_0)
    {
        existing.links += added;
        return;
    }
    touched.push(TouchedMeet {
        state,
        line_0,
        time,
        created,
        links: added,
    });
}

fn open_meet_targets(contents: &str) -> OpenMeetTargets {
    let scan = capture_pomodoros::scan(contents);
    let timed: Vec<&PomodoroEntry> = scan
        .entries
        .iter()
        .filter(|entry| {
            entry.state == PomodoroState::Open && entry.time_range.is_some()
        })
        .collect();
    let placeholder = scan.entries.iter().find_map(|entry| {
        (entry.state == PomodoroState::Open
            && entry.time_range.is_none()
            && entry.name.as_deref() == Some("MEET"))
        .then_some(entry.line - 1)
    });
    let several_timed = timed.len() > 1;
    let (single_timed, single_timed_name, running_time) = if timed.len() == 1 {
        let entry = timed[0];
        (
            Some(entry.line - 1),
            entry.name.clone(),
            entry.time_range.clone(),
        )
    } else {
        (None, None, None)
    };
    OpenMeetTargets {
        single_timed,
        single_timed_name,
        several_timed,
        placeholder,
        running_time,
    }
}

fn journaled_or_planned_block_id(
    input: &SyncPlanInput<'_>,
    plan: &SyncPlan,
    occurrence: &PlannedOccurrence,
) -> Option<String> {
    if let Some(upgraded) = plan.rows_upgraded.iter().find(|row| {
        row.calendar_id == occurrence.calendar_id
            && row.event_id == occurrence.event_id
    }) {
        return Some(upgraded.block_id.clone());
    }
    let key = occurrence_key(&occurrence.calendar_id, &occurrence.event_id);
    if let Some(state) = input.journal.get(&(key, input.day_string.clone()))
        && let Some(row) = state.row.as_ref()
        && !row.block_id.is_empty()
    {
        return Some(row.block_id.clone());
    }
    if let Some(added) = plan.rows_added.iter().find(|row| {
        row.calendar_id == occurrence.calendar_id
            && row.event_id == occurrence.event_id
    }) {
        return Some(added.block_id.clone());
    }
    plan.rows_adopted
        .iter()
        .find(|row| {
            row.calendar_id == occurrence.calendar_id
                && row.event_id == occurrence.event_id
        })
        .map(|row| row.block_id.clone())
}

#[cfg(test)]
mod tests;
