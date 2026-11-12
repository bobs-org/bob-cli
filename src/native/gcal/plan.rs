//! Pure `bob gcal sync` planner: snapshot plus journal state in,
//! `SyncPlan{contents, records, report}` out.
//!
//! The plan inserts new rows in time order (never reordering existing
//! rows), adopts crash-recovery rows by their `📅` URL, tombstones
//! journaled rows whose line is gone, and files each created Event Task
//! under a past, running, or planned MEET Pomodoro in the same write. Each
//! run also keeps every Pomodoro entry's Event Task links in meeting-time
//! order. It
//! keeps a blank line under the `## Events` heading, then writes or
//! refreshes the Bob-owned Events stamp as its last step. A run at a
//! later minute rewrites only that stamp. When the computed contents
//! equal the input, the plan reports no change and carries no records.

use std::{
    collections::{BTreeMap, HashSet},
    ops::Range,
    path::Path,
};

use chrono::{NaiveDate, NaiveDateTime};

use crate::native::{
    capture::{
        dominant_indent_unit, first_child_indentation, leading_whitespace,
        line_spans,
    },
    capture_pomodoros::{self, PomodoroEntry, PomodoroState},
    capture_task_toggle::{
        child_block_end_line, insert_line_after, insert_line_before,
        insert_link_into_entry, insert_named_placeholder_after_last_open,
    },
    clock_tags::{self, ClockKind, ClockTag},
    event_task,
    ledger_place::{self, LedgerInsert},
    markdown,
    pomodoro::pomodoros_section_range,
    pomodoro_homes::ClockLookup,
};

use super::{
    description::bullets::{bullets_hash, DescriptionBullets},
    journal::{occurrence_key, DescriptionState, OccurrenceState},
    lifecycle::{
        cancel_task_row, line_root, remove_childless_untimed_meet,
        remove_meet_link, row_untouched, split_row_spans, status_char,
        AbsentVerdict, KeptNotice, LearnedSeries, LifecycleItem,
    },
    render::{
        escape_title, render_context_row, render_task_row, ContextRowInput,
    },
    reset::DayReset,
    root::RootSourceKind,
    stamp::{self, Stamp},
};

/// One included occurrence with its decided rendering.
#[derive(Debug, Clone)]
pub(crate) struct PlannedOccurrence {
    pub calendar_id: String,
    pub event_id: String,
    pub ical_uid: Option<String>,
    pub series_id: Option<String>,
    /// Fresh private-note provenance for this task row, when a note
    /// contributed. `None` for context rows and note-free tasks.
    pub note: Option<super::journal::NoteProvenance>,
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
    /// How `root` was chosen; series memory never reroutes existing rows.
    pub root_kind: RootSourceKind,
    /// Context-row qualifier (`maybe`, `free`, `day 2 of 2`); tasks ignore it.
    pub qualifier: Option<String>,
    /// Short SHA-256 hashes of each managed span, for the journal.
    pub spans: BTreeMap<String, String>,
    /// Write `[type:: MEET]` (the PT13 suggestion is not MEET).
    pub needs_type: bool,
    /// Google `[buffer::]` field (description directives). Context rows
    /// never carry it; task rows render it at the end of the badge run.
    pub buffer: crate::native::event_task::EventBuffer,
    /// Rendered, capped description bullets for task occurrences in
    /// `bullets` mode (`Some` even when the block is empty). `None`
    /// for context rows and in `off` mode.
    pub description: Option<DescriptionBullets>,
    /// Google's explicit clock state (title or description). Context rows
    /// strip it; task rows render only its canonical tag in the badge run.
    pub clock_tag: ClockTag,
    /// Tag-only fallback used when no clock lookup is supplied. MEET
    /// planning and task-row reports prefer the retained line's effective
    /// clock and must never materialize an inherited tag.
    pub off_clock: bool,
    /// Phase at sync time; `None` for context rows.
    pub link_phase: Option<LinkPhase>,
    pub all_day: bool,
    /// Meeting span in minutes from the note day's midnight (the end may
    /// exceed 1440). The bold time label and its journal hash keep this
    /// span; MEET filing derives the buffered span from it plus `buffer`.
    pub start_min: i32,
    /// End in minutes from the note day's midnight (may exceed 1440).
    pub end_min: i32,
}

/// How a created Event Task is filed under a MEET Pomodoro.
/// Past rows are born done on the note's day with a struck 🍅 link;
/// Current and Future rows start Next.
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
    pub notes: &'a super::journal::NoteBook,
    pub malformed: usize,
    pub day_string: String,
    /// Cancel reasons and similar lifecycle notices land under this date.
    pub today: NaiveDate,
    /// Journaled occurrences with no included snapshot row, each already
    /// resolved to a verdict (declined, deleted, moved, left alone).
    pub lifecycle: Vec<LifecycleItem>,
    /// Pre-reset and synthesized state for `bob gcal sync -F`.
    pub reset: Option<&'a DayReset>,
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
    pub off_clock: bool,
    pub note: Option<super::journal::NoteProvenance>,
    pub buffer: crate::native::event_task::EventBuffer,
    /// The `gcal.row` description record for this row. Every construction
    /// site sets it: there is no `Default`, so the compiler flags any
    /// site a later phase adds.
    pub description: DescriptionState,
    /// Bullet lines this run wrote for the row, 0 when the block was
    /// left alone or adopted without writing.
    pub wrote_description_lines: usize,
    /// Explanation for a force reset of a Google-owned span.
    pub detail: Option<String>,
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
    pub off_clock: bool,
    pub note: Option<super::journal::NoteProvenance>,
    pub buffer: crate::native::event_task::EventBuffer,
    /// Crash recovery: `Tracked` when a window matches the fresh
    /// render (an empty render always matches), else `HandsOff`.
    /// Context rows and `off` mode stay `Absent`. Adoption never
    /// writes bullets.
    pub description: DescriptionState,
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
    /// The occurrence's time label (`1400-1500`, `all day`) and bare
    /// title, so the tombstoning run can name the row it lost. Empty
    /// when the event is not in this run's snapshot.
    pub time_label: String,
    pub title: String,
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

/// A journaled Event Task whose Google-owned occupied span changed during
/// this plan. `old_line` is the pre-update task line used to validate any
/// Bob-written closed MEET that currently files it.
#[derive(Debug, Clone)]
pub(super) struct MovedRow {
    pub calendar_id: String,
    pub event_id: String,
    pub block_id: String,
    pub old_line: String,
}

/// The pure plan outcome.
#[derive(Debug, Clone, Default)]
pub(crate) struct SyncPlan {
    pub contents: String,
    pub changed: bool,
    pub rows_added: Vec<PlannedRow>,
    /// Rows restored from a prior day's journal by a forced reset.
    pub rows_restored: Vec<PlannedRow>,
    pub rows_adopted: Vec<AdoptedRow>,
    pub rows_gone: Vec<GoneRow>,
    pub links_added: Vec<PlannedLink>,
    /// Existing MEET links moved after a Google-side span update.
    pub links_refiled: Vec<PlannedLink>,
    /// Rows whose Google-owned occupied span changed this run.
    pub(super) moved_rows: Vec<MovedRow>,
    /// Event Task link lines this run moved into meeting-time order.
    pub links_sorted: usize,
    /// Google-owned spans and unedited `[root::]` values replaced in place.
    pub rows_updated: Vec<PlannedRow>,
    /// Journaled rows held because their private note could not be read.
    /// These are report-only: they do not mark the run written and they
    /// do not append a `gcal.row`.
    pub rows_held: Vec<PlannedRow>,
    /// Untouched tasks cancelled for declined, deleted, or moved events.
    pub rows_cancelled: Vec<CancelledRow>,
    /// Untouched context rows removed for declined or deleted events.
    pub rows_removed: Vec<GoneRow>,
    /// Context rows upgraded in place to Event Tasks (Maybe went Yes).
    pub rows_upgraded: Vec<PlannedRow>,
    /// Unchanged rows that need a current `gcal.row` after reset clears
    /// their previous journal records.
    pub journal_refreshes: Vec<PlannedRow>,
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

#[cfg(test)]
enum NoClock {}

#[cfg(test)]
impl ClockLookup for NoClock {
    fn resolve_clock(
        &mut self,
        _residence_path: &Path,
        _line: &str,
    ) -> crate::native::pomodoro_homes::ClockResolution {
        unreachable!("plan_sync never resolves clocks")
    }
}

#[cfg(test)]
pub(crate) fn plan_sync(input: &SyncPlanInput<'_>) -> SyncPlan {
    plan_with_clock::<NoClock>(input, None, None)
}

/// Same as [`plan_sync`], resolving MEET clocks from retained rows through
/// `clock` when present. Unit tests inject an in-memory home source here.
pub(crate) fn plan_with_clock<C: ClockLookup + ?Sized>(
    input: &SyncPlanInput<'_>,
    daily_rel: Option<&Path>,
    mut clock: Option<&mut C>,
) -> SyncPlan {
    let mut working = WorkingNote::new(input.contents);
    let mut plan = SyncPlan {
        contents: input.contents.to_string(),
        ..SyncPlan::default()
    };
    let mut used: HashSet<String> = collect_block_ids(&working.lines);

    for occurrence in &input.occurrences {
        let key = occurrence_key(&occurrence.calendar_id, &occurrence.event_id);
        let state = input.journal.get(&(key.clone(), input.day_string.clone()));
        let journaled = state.and_then(|state| state.row.as_ref());
        let gone = state.is_some_and(|state| state.gone);
        // Context rows without a 📅 URL match by their full rendered
        // text, so URL-less events stay idempotent.
        let text_match = if !occurrence.task && occurrence.url().is_none() {
            Some(render_occurrence(occurrence, "", note_day(input)))
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
                    // A tombstoned row whose line is still missing stays
                    // gone: no new record, no report row, so a rerun is
                    // unchanged. Only a first sighting tombstones.
                    if !gone {
                        plan.rows_gone.push(GoneRow {
                            calendar_id: occurrence.calendar_id.clone(),
                            event_id: occurrence.event_id.clone(),
                            block_id: journaled.block_id.clone(),
                            reason: "line_removed".to_string(),
                            time_label: occurrence.time_label.clone(),
                            title: occurrence.title.clone(),
                        });
                    }
                    continue;
                };
                if gone {
                    // Pasted back: adopt the line again. Its `gcal.row`
                    // revives the occurrence in the journal fold.
                    adopt_occurrence(
                        &working, &mut plan, &mut used, occurrence, line_index,
                    );
                    continue;
                }
                let journaled_kind_is_task = journaled.row_kind == "task";
                if journaled_kind_is_task == occurrence.task {
                    if let Some(held) =
                        hold_note_row(input, occurrence, journaled)
                    {
                        hold_journaled_row(
                            &working, &mut plan, occurrence, journaled,
                            line_index, held,
                        );
                        continue;
                    }
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
                    let description = apply_description_update(
                        input,
                        &mut working,
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
                        description,
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
                    adopt_occurrence(
                        &working, &mut plan, &mut used, occurrence, line_index,
                    );
                    continue;
                }
                let prior_row = input
                    .reset
                    .and_then(|reset| {
                        reset
                            .previous
                            .get(&(key.clone(), input.day_string.clone()))
                    })
                    .and_then(|state| state.row.as_ref());
                let restored_id = prior_row
                    .map(|row| row.block_id.clone())
                    .filter(|id| !id.is_empty() && !used.contains(id));
                let block_id = if occurrence.task {
                    if let Some(block_id) = restored_id {
                        used.insert(block_id.clone());
                        block_id
                    } else {
                        let minted =
                            crate::native::capture_block_ids::mint_block_id(
                                &occurrence.id_hint,
                                &used.iter().map(String::as_str).collect(),
                            );
                        used.insert(minted.clone());
                        minted
                    }
                } else {
                    String::new()
                };
                let text =
                    render_occurrence(occurrence, &block_id, note_day(input));
                working.ensure_events_section();
                let slot = insert_slot(
                    &working,
                    occurrence.all_day,
                    occurrence.start_min,
                );
                // A new task row and its materialized bullets go into the
                // note in one block, in the same write as the MEET link.
                // New context rows record `Absent`.
                let mut block = vec![text];
                if occurrence.task {
                    let (depth1, unit) = fresh_bullet_indents(&working);
                    if let Some(description) = occurrence.description.as_ref() {
                        block.extend(description.materialize(&depth1, &unit));
                    }
                }
                working.insert_block(slot, &block);
                let row = PlannedRow {
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
                    off_clock: occurrence.off_clock,
                    note: occurrence.note.clone(),
                    buffer: buffer_for_row(occurrence),
                    description: tracked_state(occurrence.description.as_ref()),
                    wrote_description_lines: occurrence
                        .description
                        .as_ref()
                        .map(|bullets| bullets.bullets.len())
                        .unwrap_or(0),
                    detail: None,
                };
                if prior_row.is_some() {
                    plan.rows_restored.push(row);
                } else {
                    plan.rows_added.push(row);
                }
            }
        }
    }

    if !input.lifecycle.is_empty() {
        apply_lifecycle(input, &mut working, &mut plan);
    }

    working.ensure_events_heading_gap();

    if let (Some(daily_rel), Some(clock)) =
        (daily_rel, reborrow_clock(&mut clock))
    {
        apply_effective_clocks(clock, daily_rel, &working, &mut plan);
    }

    let refile_wants = super::refile::plan_refile(
        input,
        &mut working,
        &mut plan,
        daily_rel,
        reborrow_clock(&mut clock),
    );

    plan_meet_pomodoros(
        input,
        &mut working,
        &mut plan,
        refile_wants,
        daily_rel,
        reborrow_clock(&mut clock),
    );

    let (sorted, links_sorted) = super::link_order::sort_event_task_links(
        &working.finish(),
        note_day(input),
    );
    if links_sorted > 0 {
        working.replace(&sorted);
        plan.links_sorted = links_sorted;
    }

    plan_stamp(input, &mut working, &mut plan);

    plan.contents = working.finish();
    if input.reset.is_some() {
        plan.journal_refreshes = reset_refreshes(input, &plan);
    }
    // A no-op plan writes nothing and appends nothing: identical
    // contents and no records. A tombstone-only run leaves the note
    // alone but still records `gcal.gone`, so it is not a no-op; kept
    // notices, series learning, and root reroutes likewise append
    // journal records.
    let records_empty = plan.rows_added.is_empty()
        && plan.rows_restored.is_empty()
        && plan.rows_adopted.is_empty()
        && plan.rows_gone.is_empty()
        && plan.rows_removed.is_empty()
        && plan.links_added.is_empty()
        && plan.links_refiled.is_empty()
        && plan.rows_updated.is_empty()
        && plan.rows_cancelled.is_empty()
        && plan.rows_upgraded.is_empty()
        && plan.journal_refreshes.is_empty()
        && plan.kept.is_empty()
        && plan.learned.is_empty();
    plan.changed = plan.contents != input.contents || !records_empty;
    if !plan.changed {
        plan.rows_added.clear();
        plan.rows_restored.clear();
        plan.rows_adopted.clear();
        plan.rows_gone.clear();
        plan.rows_removed.clear();
        plan.links_added.clear();
        plan.links_refiled.clear();
        plan.moved_rows.clear();
        plan.links_sorted = 0;
        plan.rows_updated.clear();
        plan.rows_cancelled.clear();
        plan.rows_upgraded.clear();
        plan.journal_refreshes.clear();
        plan.kept.clear();
        plan.learned.clear();
        plan.extra_link_wants.clear();
        plan.entries_removed = 0;
        plan.pomodoro_entries.clear();
        if plan.rows_held.is_empty() {
            plan.warnings.clear();
        }
        // Keep `stamp` / `stamp_refreshed` so an unchanged run still
        // reports the line the note already carries. Held rows and their
        // warnings stay so a hold-only run can report them without writing.
    }
    plan
}

/// Emit current row records for previously journaled rows whose visible
/// contents needed no normal update. The reset marker clears all old row
/// records, so these snapshots make the following ordinary sync a no-op.
fn reset_refreshes(
    input: &SyncPlanInput<'_>,
    plan: &SyncPlan,
) -> Vec<PlannedRow> {
    let Some(reset) = input.reset else {
        return Vec::new();
    };
    let lines: Vec<String> =
        plan.contents.lines().map(str::to_string).collect();
    let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
    let events = event_task::events_section_range(&refs);
    let mut refreshed = Vec::new();
    for occurrence in &input.occurrences {
        let key = occurrence_key(&occurrence.calendar_id, &occurrence.event_id);
        let Some(old_row) = reset
            .previous
            .get(&(key.clone(), reset.day.clone()))
            .and_then(|state| state.row.as_ref())
        else {
            continue;
        };
        let already_recorded =
            plan.rows_added
                .iter()
                .chain(&plan.rows_restored)
                .any(|row| {
                    row.calendar_id == occurrence.calendar_id
                        && row.event_id == occurrence.event_id
                })
                || plan.rows_adopted.iter().any(|row| {
                    row.calendar_id == occurrence.calendar_id
                        && row.event_id == occurrence.event_id
                })
                || plan.rows_updated.iter().chain(&plan.rows_upgraded).any(
                    |row| {
                        row.calendar_id == occurrence.calendar_id
                            && row.event_id == occurrence.event_id
                    },
                );
        if already_recorded {
            continue;
        }
        let text_match = (!occurrence.task && occurrence.html_link.is_none())
            .then(|| render_occurrence(occurrence, "", note_day(input)));
        let Some(line_index) = find_row_line(
            &lines,
            events.clone(),
            &old_row.block_id,
            occurrence.html_link.as_deref(),
            occurrence.task,
            text_match.as_deref(),
        ) else {
            continue;
        };
        let line = &lines[line_index];
        let Some(spans) = split_row_spans(line) else {
            continue;
        };
        let block_id = trailing_block_id(line);
        let root = line_root(line);
        refreshed.push(PlannedRow {
            calendar_id: occurrence.calendar_id.clone(),
            event_id: occurrence.event_id.clone(),
            ical_uid: occurrence.ical_uid.clone(),
            series_id: occurrence.series_id.clone(),
            block_id: if block_id.is_empty() {
                old_row.block_id.clone()
            } else {
                block_id
            },
            url: occurrence.html_link.clone(),
            row_kind: row_kind(spans.task),
            spans: spans.hashes(),
            root: root.clone(),
            source: if root == occurrence.root {
                occurrence.source.clone()
            } else {
                old_row.source.clone()
            },
            time_label: occurrence.time_label.clone(),
            title: occurrence.title.clone(),
            off_clock: occurrence.off_clock,
            note: occurrence.note.clone(),
            buffer: buffer_for_row(occurrence),
            description: old_row.description.clone(),
            wrote_description_lines: 0,
            detail: None,
        });
    }
    refreshed
}

/// Adopt the line at `line_index` as this occurrence's row: crash
/// recovery for an unjournaled line, or a tombstoned row pasted back.
/// The new `gcal.row` records what Google says now; a line that differs
/// reads as your edit on the next run.
fn adopt_occurrence(
    working: &WorkingNote,
    plan: &mut SyncPlan,
    used: &mut HashSet<String>,
    occurrence: &PlannedOccurrence,
    line_index: usize,
) {
    let block_id = trailing_block_id(&working.lines[line_index]);
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
        off_clock: occurrence.off_clock,
        note: occurrence.note.clone(),
        buffer: buffer_for_row(occurrence),
        description: adopt_description(&working.lines, line_index, occurrence),
    });
}

/// Outcome of the description-bullet three-way for one journaled row.
struct DescriptionUpdate {
    state: DescriptionState,
    wrote_lines: usize,
    changed: bool,
}

fn empty_bullets_hash() -> String {
    bullets_hash(std::iter::empty())
}

fn bullets_tracked(description: &DescriptionBullets) -> DescriptionState {
    DescriptionState::Tracked {
        hash: bullets_hash(
            description
                .bullets
                .iter()
                .map(|bullet| (bullet.depth, bullet.body.as_str())),
        ),
        lines: description.bullets.len(),
    }
}

/// Three-way update of one journaled task row's description bullets.
/// Edits only lines below the row, so `line_index` stays valid for a
/// later span update / re-slot. `Absent` is an empty tracked block
/// (backfill); `HandsOff`, context rows, Maybe rows, and `off` mode
/// skip. A matching window is rewritten in place; Bryan's edit, a
/// nested note, or a deletion keeps the old record and reports
/// `kept_description` once only when Google's text also changed.
fn apply_description_update(
    input: &SyncPlanInput<'_>,
    working: &mut WorkingNote,
    plan: &mut SyncPlan,
    occurrence: &PlannedOccurrence,
    journaled: &super::journal::RowRecord,
    line_index: usize,
) -> DescriptionUpdate {
    let noop = DescriptionUpdate {
        state: journaled.description.clone(),
        wrote_lines: 0,
        changed: false,
    };
    if journaled.row_kind != "task" || !occurrence.task {
        return noop;
    }
    let Some(fresh) = occurrence.description.as_ref() else {
        return noop;
    };
    if matches!(journaled.description, DescriptionState::HandsOff) {
        return noop;
    }
    // Bullets move only together with a `gcal.row` record. A row found
    // by block ID whose line no longer parses as a task row gets no span
    // update (so no record), so its bullets stay put too.
    if !working
        .lines
        .get(line_index)
        .and_then(|line| split_row_spans(line))
        .is_some_and(|spans| spans.task)
    {
        return noop;
    }
    let fresh_state = bullets_tracked(fresh);
    let (fresh_hash, fresh_lines) = match &fresh_state {
        DescriptionState::Tracked { hash, lines } => (hash.clone(), *lines),
        _ => return noop,
    };

    // Backfill guard: an Absent row whose child block already matches
    // the fresh render (a hand copy or a crash) is adopted without
    // writing, so a later run never inserts a duplicate.
    if matches!(journaled.description, DescriptionState::Absent)
        && fresh_lines > 0
        && super::bullet_block::find_block(
            &working.lines,
            line_index,
            &fresh_hash,
            fresh_lines,
        )
        .is_some()
    {
        return DescriptionUpdate {
            state: fresh_state,
            wrote_lines: 0,
            changed: true,
        };
    }

    let (recorded_hash, recorded_lines) = match &journaled.description {
        DescriptionState::Absent => (empty_bullets_hash(), 0usize),
        DescriptionState::Tracked { hash, lines } => (hash.clone(), *lines),
        DescriptionState::HandsOff => return noop,
    };
    let Some(window) = super::bullet_block::find_block(
        &working.lines,
        line_index,
        &recorded_hash,
        recorded_lines,
    ) else {
        if fresh_hash != recorded_hash {
            let journaled_note_hash =
                journaled.note.as_ref().map(|note| note.hash.as_str());
            let fresh_note_hash =
                occurrence.note.as_ref().map(|note| note.hash.as_str());
            let detail = if journaled_note_hash != fresh_note_hash {
                "kept your edited description bullets; your private note changed"
                    .to_string()
            } else {
                "kept your edited description bullets; Google's description changed"
                    .to_string()
            };
            push_kept_once(
                input,
                plan,
                &occurrence.calendar_id,
                &occurrence.event_id,
                occurrence.title.clone(),
                detail,
                "kept_description",
            );
        }
        return noop;
    };
    if fresh_hash == recorded_hash {
        return noop;
    }
    let (depth1, unit) = child_bullet_indents(working, line_index);
    let materialized = fresh.materialize(&depth1, &unit);
    super::bullet_block::replace_window(
        &mut working.lines,
        window,
        &materialized,
    );
    DescriptionUpdate {
        state: fresh_state,
        wrote_lines: fresh_lines,
        changed: true,
    }
}

/// Crash-recovery description record: `Tracked` when a window matches
/// the fresh render (empty always matches), else `HandsOff`. Context
/// rows and `off` mode stay `Absent`. Never writes.
fn adopt_description(
    lines: &[String],
    line_index: usize,
    occurrence: &PlannedOccurrence,
) -> DescriptionState {
    let Some(bullets) = occurrence.description.as_ref() else {
        return DescriptionState::Absent;
    };
    if bullets.bullets.is_empty() {
        return DescriptionState::Tracked {
            hash: empty_bullets_hash(),
            lines: 0,
        };
    }
    let state = bullets_tracked(bullets);
    let DescriptionState::Tracked { hash, lines: count } = &state else {
        return DescriptionState::HandsOff;
    };
    if super::bullet_block::find_block(lines, line_index, hash, *count)
        .is_some()
    {
        state
    } else {
        DescriptionState::HandsOff
    }
}

/// Three-way update of one still-included row: re-render the time,
/// title, and badge spans and replace each span whose current text
/// still matches the journal hash. A span that differs from both the
/// journal and the fresh render is Bryan's edit: it is kept and
/// reported once. An unedited `[root::]` follows Google's chosen root
/// (never series memory); a root Bryan edited is kept and reported
/// once when Google later routes elsewhere. `[type:: MEET]` is added
/// on a reroute only when the line has no type field. The checkbox
/// and block ID are never rewritten.
#[allow(clippy::too_many_arguments)]
fn apply_span_updates(
    input: &SyncPlanInput<'_>,
    working: &mut WorkingNote,
    plan: &mut SyncPlan,
    occurrence: &PlannedOccurrence,
    journaled: &super::journal::RowRecord,
    line_index: usize,
    description: DescriptionUpdate,
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
    let original_buffer = event_task::parse_event_buffer(&current_line);
    // Context rows strip the token, so only task rows hash the tag.
    // The badges span covers explicit `#off`/`#on` and `[buffer::]`, so flips
    // and buffer changes update in place while untouched.
    let fresh_badges = super::render::badge_text(
        occurrence.html_link.as_deref(),
        occurrence.join_link.as_deref(),
        occurrence.location.as_deref(),
        occurrence.task.then_some(&occurrence.clock_tag),
        buffer_for_row(occurrence),
    );
    // A tag flip on a worked row is reported once and left for a hand
    // edit: sync never retimes existing entries, and the ledger row keeps
    // its clock classification until Bryan edits it.
    let current_clock = clock_tags::line_clock_tag(&current_line).kind();
    let google_clock = occurrence.clock_tag.kind();
    let untouched =
        row_untouched(&working.as_string(), line_index, &journaled.block_id);
    let keep_off_clock =
        occurrence.task && current_clock != google_clock && !untouched;
    if keep_off_clock {
        let detail = kept_clock_detail(current_clock, google_clock);
        push_kept_once(
            input,
            plan,
            &occurrence.calendar_id,
            &occurrence.event_id,
            occurrence.title.clone(),
            detail,
            "kept_off_clock",
        );
    }
    let fresh = [
        ("time", occurrence.time_label.clone()),
        ("title", escape_title(&occurrence.title)),
        ("badges", fresh_badges),
    ];
    let mut edited: Vec<&str> = Vec::new();
    let mut replaced_time = false;
    let mut hashes_changed = false;
    let mut updated = journaled.spans.clone();
    for (name, fresh_text) in fresh {
        if name == "badges" && keep_off_clock {
            continue;
        }
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
    let qualifier_changed = !occurrence.task
        && spans.qualifier != occurrence.qualifier
        && edited.is_empty();
    if qualifier_changed {
        spans.qualifier.clone_from(&occurrence.qualifier);
    }
    if !edited.is_empty() {
        push_kept_once(
            input,
            plan,
            &occurrence.calendar_id,
            &occurrence.event_id,
            occurrence.title.clone(),
            format!("kept your edit to {}", edited.join(" and ")),
            "kept_edit",
        );
    }
    let root_action = root_update_action(occurrence, journaled, &current_line);
    if let RootAction::Keep {
        current,
        fresh,
        source,
    } = &root_action
    {
        push_kept_once(
            input,
            plan,
            &occurrence.calendar_id,
            &occurrence.event_id,
            occurrence.title.clone(),
            kept_root_detail(current.as_deref(), fresh, source.as_deref()),
            "kept_root",
        );
    }
    let apply_spans =
        edited.is_empty() && (hashes_changed || qualifier_changed);
    let apply_root = matches!(root_action, RootAction::Reroute { .. });
    if !apply_spans && !apply_root && !description.changed {
        return;
    }
    let mut line = if apply_spans {
        spans.rebuild()
    } else {
        current_line.clone()
    };
    let (root, source) =
        if let RootAction::Reroute { value, source } = &root_action {
            // The gcal setter replaces `[root::]` in place (adding
            // `[type:: MEET]` after it when needed) without moving the
            // Google run, so the next sync reports no badges edit.
            line = super::render::set_row_metadata(
                &line,
                Some(value),
                occurrence.needs_type,
            );
            (Some(value.clone()), source.clone())
        } else {
            (journaled.root.clone(), journaled.source.clone())
        };
    let buffer_changed =
        apply_spans && event_task::parse_event_buffer(&line) != original_buffer;
    if journaled.row_kind == "task" && (replaced_time || buffer_changed) {
        plan.moved_rows.push(MovedRow {
            calendar_id: occurrence.calendar_id.clone(),
            event_id: occurrence.event_id.clone(),
            block_id: journaled.block_id.clone(),
            old_line: current_line.clone(),
        });
    }
    working.lines[line_index] = line;
    if apply_spans && replaced_time {
        reslot_block(
            working,
            line_index,
            occurrence.all_day,
            occurrence.start_min,
        );
    }
    let record_spans = if edited.is_empty() {
        updated
    } else {
        journaled.spans.clone()
    };
    plan.rows_updated.push(PlannedRow {
        calendar_id: occurrence.calendar_id.clone(),
        event_id: occurrence.event_id.clone(),
        ical_uid: occurrence.ical_uid.clone(),
        series_id: occurrence.series_id.clone(),
        block_id: journaled.block_id.clone(),
        url: occurrence.html_link.clone(),
        row_kind: journaled.row_kind.clone(),
        spans: record_spans,
        root,
        source,
        time_label: occurrence.time_label.clone(),
        title: occurrence.title.clone(),
        off_clock: occurrence.off_clock,
        note: occurrence.note.clone(),
        buffer: buffer_for_row(occurrence),
        // A title update or root reroute keeps the description record
        // unless this run also rewrote or adopted the bullet block.
        description: if description.changed {
            description.state
        } else {
            journaled.description.clone()
        },
        wrote_description_lines: description.wrote_lines,
        detail: forced_reset_detail(
            input,
            occurrence,
            &current_line,
            keep_off_clock,
        ),
    });
}

/// Fail-safe hold for a journaled row whose private note contributed
/// fields, while the current fold has neither a live note nor a deliberate
/// clear for that key and the journal reports malformed gcal lines.
/// Returns the hold warning when the row must be held.
fn hold_note_row(
    input: &SyncPlanInput<'_>,
    occurrence: &PlannedOccurrence,
    journaled: &super::journal::RowRecord,
) -> Option<String> {
    let journaled_note = journaled.note.as_ref()?;
    if journaled_note.fields.is_empty() {
        return None;
    }
    if input.malformed == 0 {
        return None;
    }
    let has_note = input
        .notes
        .event_record(
            input.account,
            &occurrence.calendar_id,
            &occurrence.event_id,
        )
        .is_some()
        || occurrence.series_id.as_deref().is_some_and(|series| {
            input
                .notes
                .series_record(input.account, &occurrence.calendar_id, series)
                .is_some()
        });
    if has_note {
        return None;
    }
    let noun = if input.malformed == 1 {
        "1 malformed gcal journal line"
    } else {
        &format!("{} malformed gcal journal lines", input.malformed)
    };
    Some(format!(
        "held {}: its private note could not be read ({noun}) · bob gcal doctor",
        occurrence.title
    ))
}

/// Carry a held journal record forward unchanged: skip span, root, and
/// description updates, report the hold warning, and record the row with
/// its journaled spans so the report carries the row's detail.
fn hold_journaled_row(
    working: &WorkingNote,
    plan: &mut SyncPlan,
    occurrence: &PlannedOccurrence,
    journaled: &super::journal::RowRecord,
    line_index: usize,
    warning: String,
) {
    plan.warnings.push(warning.clone());
    let buffer = working
        .lines
        .get(line_index)
        .map(|line| event_task::parse_event_buffer(line))
        .unwrap_or_default();
    plan.rows_held.push(PlannedRow {
        calendar_id: occurrence.calendar_id.clone(),
        event_id: occurrence.event_id.clone(),
        ical_uid: occurrence.ical_uid.clone(),
        series_id: occurrence.series_id.clone(),
        block_id: journaled.block_id.clone(),
        url: occurrence.html_link.clone(),
        row_kind: journaled.row_kind.clone(),
        spans: journaled.spans.clone(),
        root: journaled.root.clone(),
        source: journaled.source.clone(),
        time_label: occurrence.time_label.clone(),
        title: occurrence.title.clone(),
        off_clock: occurrence.off_clock,
        note: journaled.note.clone(),
        buffer,
        description: journaled.description.clone(),
        wrote_description_lines: 0,
        detail: Some(warning),
    });
}

fn forced_reset_detail(
    input: &SyncPlanInput<'_>,
    occurrence: &PlannedOccurrence,
    line: &str,
    keep_off_clock: bool,
) -> Option<String> {
    let reset = input.reset?;
    let key = occurrence_key(&occurrence.calendar_id, &occurrence.event_id);
    let old = reset
        .previous
        .get(&(key, reset.day.clone()))?
        .row
        .as_ref()?;
    let current = split_row_spans(line)?.hashes();
    let mut edited = Vec::new();
    for name in ["time", "title", "badges"] {
        if name == "badges" && keep_off_clock {
            continue;
        }
        if current.get(name) != occurrence.spans.get(name)
            && current.get(name) != old.spans.get(name)
        {
            edited.push(name);
        }
    }
    let current_root = line_root(line);
    let fresh_root = nonempty_root(occurrence.root.as_deref());
    if current_root.as_deref() != fresh_root
        && current_root.as_deref() != nonempty_root(old.root.as_deref())
    {
        edited.push("root");
    }
    match edited.as_slice() {
        [] => None,
        [name] => Some(format!("reset your {name} edit")),
        names => Some(format!("reset your {} edits", names.join(" and "))),
    }
}

/// What to do with one journaled task row's `[root::]`.
enum RootAction {
    Skip,
    Reroute {
        value: String,
        source: Option<String>,
    },
    Keep {
        current: Option<String>,
        fresh: String,
        source: Option<String>,
    },
}

fn nonempty_root(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|value| !value.is_empty())
}

fn root_update_action(
    occurrence: &PlannedOccurrence,
    journaled: &super::journal::RowRecord,
    line: &str,
) -> RootAction {
    if journaled.row_kind != "task" || !occurrence.task {
        return RootAction::Skip;
    }
    let Some(fresh) = nonempty_root(occurrence.root.as_deref()) else {
        return RootAction::Skip;
    };
    if occurrence.root_kind == RootSourceKind::SeriesMemory {
        return RootAction::Skip;
    }
    // Note and SeriesNote reroute like Token: an untouched `[root::]`
    // follows the note, and a vault edit wins (`kept_root`).
    let current_owned = line_root(line);
    let current = nonempty_root(current_owned.as_deref()).map(str::to_string);
    if current.as_deref() == Some(fresh) {
        return RootAction::Skip;
    }
    let recorded = nonempty_root(journaled.root.as_deref());
    if current.as_deref() == recorded {
        return RootAction::Reroute {
            value: fresh.to_string(),
            source: occurrence.source.clone(),
        };
    }
    if Some(fresh) != recorded {
        return RootAction::Keep {
            current,
            fresh: fresh.to_string(),
            source: occurrence.source.clone(),
        };
    }
    RootAction::Skip
}

fn kept_root_detail(
    current: Option<&str>,
    fresh: &str,
    source: Option<&str>,
) -> String {
    let yours = match current {
        Some(value) => format!("[root:: {value}]"),
        None => "[root::]".to_string(),
    };
    match source {
        Some(source) => {
            format!(
                "kept your {yours}; Google now routes to {fresh} ({source})"
            )
        }
        None => format!("kept your {yours}; Google now routes to {fresh}"),
    }
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
    working.lines[line_index] =
        render_occurrence(occurrence, &block_id, note_day(input));
    // The bullets land directly under the upgraded row, above any notes
    // Bryan already has there.
    if let Some(description) = occurrence.description.as_ref() {
        let (depth1, unit) = child_bullet_indents(working, line_index);
        let bullets = description.materialize(&depth1, &unit);
        if !bullets.is_empty() {
            working.insert_block(Some(line_index), &bullets);
        }
    }
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
        off_clock: occurrence.off_clock,
        note: occurrence.note.clone(),
        buffer: buffer_for_row(occurrence),
        description: tracked_state(occurrence.description.as_ref()),
        wrote_description_lines: occurrence
            .description
            .as_ref()
            .map(|bullets| bullets.bullets.len())
            .unwrap_or(0),
        detail: None,
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
                time_label: String::new(),
                title: String::new(),
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
        time_label: String::new(),
        title: journaled_title(current_line),
    });
}

/// Report one lifecycle notice once per (occurrence, day).
pub(super) fn push_kept_once(
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

fn kept_clock_detail(current: ClockKind, google: ClockKind) -> String {
    match (current, google) {
        (ClockKind::None, ClockKind::Off) => {
            "kept the row without an explicit clock tag while Google now says #off; add #off by hand to follow Google".to_string()
        }
        (ClockKind::On, ClockKind::Off) => {
            "kept the row's #on override while Google now says #off; replace #on with #off by hand to follow Google".to_string()
        }
        (ClockKind::Off, ClockKind::On) => {
            "kept the row's #off tag while Google now says #on; replace #off with #on by hand to follow Google".to_string()
        }
        (ClockKind::None, ClockKind::On) => {
            "kept the row without an explicit clock tag while Google now says #on; add #on by hand to follow Google".to_string()
        }
        (ClockKind::Off, ClockKind::None) => {
            "kept the row's #off tag while Google dropped its explicit clock tag; remove #off by hand to follow Google".to_string()
        }
        (ClockKind::On, ClockKind::None) => {
            "kept the row's #on override while Google dropped its explicit clock tag; remove #on by hand to follow Google".to_string()
        }
        _ => "kept your explicit clock tag while Google changed its clock tag".to_string(),
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

pub(super) fn render_occurrence(
    occurrence: &PlannedOccurrence,
    block_id: &str,
    completion_day: NaiveDate,
) -> String {
    if occurrence.task {
        let row = render_task_row(
            &super::render::TaskRowInput {
                time_label: &occurrence.time_label,
                title: &occurrence.title,
                html_link: occurrence.html_link.as_deref(),
                join_link: occurrence.join_link.as_deref(),
                location: occurrence.location.as_deref(),
                root: occurrence.root.as_deref(),
                needs_type: occurrence.needs_type,
                clock_tag: occurrence.clock_tag.clone(),
                buffer: buffer_for_row(occurrence),
                status: status_for_phase(occurrence.link_phase),
            },
            block_id,
        );
        if occurrence.link_phase == Some(LinkPhase::Past) {
            crate::native::task_complete::tree::add_or_replace_completion_field(
                &row,
                &completion_day.format("%Y-%m-%d").to_string(),
            )
        } else {
            row
        }
    } else {
        render_context_row(&ContextRowInput {
            time_label: &occurrence.time_label,
            title: &occurrence.title,
            html_link: occurrence.html_link.as_deref(),
            qualifier: occurrence.qualifier.as_deref(),
        })
    }
}

/// The buffer one occurrence renders: task rows carry Google's buffer,
/// context rows never do.
fn buffer_for_row(
    occurrence: &PlannedOccurrence,
) -> crate::native::event_task::EventBuffer {
    if occurrence.task {
        occurrence.buffer
    } else {
        crate::native::event_task::EventBuffer::default()
    }
}

/// Owned working copy of the note: owned lines plus the input's
/// trailing-newline state, so an unchanged plan is byte-identical.
pub(super) struct WorkingNote {
    pub(super) lines: Vec<String>,
    trailing_newline: bool,
}

impl WorkingNote {
    fn new(contents: &str) -> Self {
        Self {
            lines: contents.lines().map(str::to_string).collect(),
            trailing_newline: contents.ends_with('\n'),
        }
    }

    pub(super) fn replace(&mut self, contents: &str) {
        self.lines = contents.lines().map(str::to_string).collect();
        self.trailing_newline = contents.ends_with('\n');
    }

    pub(super) fn as_string(&self) -> String {
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

pub(super) fn collect_block_ids(lines: &[String]) -> HashSet<String> {
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

pub(super) fn trailing_block_id(line: &str) -> String {
    parse_trailing_block_id(line).unwrap_or_default()
}

/// Resolve the identity and child indentation a sync would reuse for a
/// previewed occurrence already present in the note.
pub(super) fn preview_existing_block_id(
    contents: &str,
    occurrence: &PlannedOccurrence,
) -> Option<String> {
    let working = WorkingNote::new(contents);
    let row = find_row_line(
        &working.lines,
        working.events_range(),
        "",
        occurrence.html_link.as_deref(),
        occurrence.task,
        Some(&occurrence.title),
    )?;
    parse_trailing_block_id(&working.lines[row])
}

/// Materialize proposed description bullets with the same indentation
/// policy as the sync writer, including an existing row's child style.
pub(super) fn preview_bullets(
    contents: &str,
    occurrence: &PlannedOccurrence,
    block_id: Option<&str>,
) -> Vec<String> {
    let Some(description) = occurrence.description.as_ref() else {
        return Vec::new();
    };
    let working = WorkingNote::new(contents);
    let row = find_row_line(
        &working.lines,
        working.events_range(),
        block_id.unwrap_or_default(),
        occurrence.html_link.as_deref(),
        occurrence.task,
        Some(&occurrence.title),
    );
    let (depth1, unit) = row
        .map(|line| child_bullet_indents(&working, line))
        .unwrap_or_else(|| fresh_bullet_indents(&working));
    description.materialize(&depth1, &unit)
}

/// The journal description record for freshly materialized bullets:
/// `Tracked` with the rendered hash and line count (even when the block
/// is empty), or `Absent` for context rows and `off` mode.
fn tracked_state(description: Option<&DescriptionBullets>) -> DescriptionState {
    match description {
        Some(bullets) => DescriptionState::Tracked {
            hash: bullets_hash(
                bullets
                    .bullets
                    .iter()
                    .map(|bullet| (bullet.depth, bullet.body.as_str())),
            ),
            lines: bullets.bullets.len(),
        },
        None => DescriptionState::Absent,
    }
}

/// Bullet indentation for a brand-new row: the row has no children yet,
/// so depth 1 is the row indent (column 0) plus the note's dominant
/// indent unit, falling back to TAB.
fn fresh_bullet_indents(working: &WorkingNote) -> (String, String) {
    let unit = dominant_indent_unit(&line_spans(&working.as_string()))
        .unwrap_or("\t")
        .to_string();
    (unit.clone(), unit)
}

/// Bullet indentation under an existing row: depth 1 reuses the row's
/// first direct child indent when one exists, else the row indent plus
/// the note's dominant unit (TAB fallback). The depth-2 unit is the
/// depth-1 indent minus the row indent.
fn child_bullet_indents(
    working: &WorkingNote,
    line_index: usize,
) -> (String, String) {
    let fallback = || fresh_bullet_indents(working);
    let Some(row) = working.lines.get(line_index) else {
        return fallback();
    };
    let row_indent = leading_whitespace(row).to_string();
    let contents = working.as_string();
    let spans = line_spans(&contents);
    if line_index >= spans.len() {
        return fallback();
    }
    let end_line =
        child_block_end_line(&spans, line_index).min(spans.len() - 1);
    let end_offset = spans[end_line].end;
    let depth1 =
        first_child_indentation(&spans, line_index, end_offset, &row_indent)
            .unwrap_or_else(|| {
                format!(
                    "{row_indent}{}",
                    dominant_indent_unit(&spans).unwrap_or("\t")
                )
            });
    let mut unit = depth1
        .strip_prefix(row_indent.as_str())
        .unwrap_or("")
        .to_string();
    if unit.is_empty() {
        unit = "\t".to_string();
    }
    (depth1, unit)
}

/// Whether `line` is a top-level Events row: only lines that
/// `split_row_spans` parses (column-0 `- ` rows) count. Indented
/// description bullets — including the overflow line that carries the
/// event's URL, or a bullet starting with `**HHMM-HHMM**` — never do.
fn is_top_level_row(line: &str) -> bool {
    split_row_spans(line).is_some()
}

/// Find a row line inside the Events section: a task matches its
/// journaled `^block_id` first, then its `📅` URL; a context row
/// matches its `📅` URL, or its full rendered text when it has none.
/// URL and text matching consider only top-level lines.
pub(super) fn find_row_line(
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
            if lines.get(index).is_some_and(|line| {
                line.contains(url) && is_top_level_row(line)
            }) {
                return Some(index);
            }
        }
    }
    if let Some(text) = text_match.filter(|text| !text.is_empty()) {
        for index in range {
            if lines.get(index).is_some_and(|line| {
                line.trim_end() == text && is_top_level_row(line)
            }) {
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
        if !is_top_level_row(line) {
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
fn plan_meet_pomodoros<C: ClockLookup + ?Sized>(
    input: &SyncPlanInput<'_>,
    working: &mut WorkingNote,
    plan: &mut SyncPlan,
    refile_wants: Vec<WantedLink>,
    daily_rel: Option<&Path>,
    clock: Option<&mut C>,
) {
    let mut wanted =
        collect_wanted_links(input, working, plan, daily_rel, clock);
    let birth_ids: HashSet<String> =
        wanted.iter().map(|link| link.block_id.clone()).collect();
    wanted.extend(
        refile_wants
            .into_iter()
            .filter(|link| !birth_ids.contains(&link.block_id)),
    );
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
        let off_clock = cluster.first().is_some_and(|link| link.off_clock);
        let (span, time) = format_meet_span(start_min, end_min, off_clock);
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
            // One running entry is allowed, so a mixed current run stays
            // on-clock; only an all-off-clock run goes off the clock.
            let off_clock = current.iter().all(|link| link.off_clock);
            let (span, time) = format_meet_span(start_min, end_min, off_clock);
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

    if !plan.links_added.is_empty() || !plan.links_refiled.is_empty() {
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
pub(super) struct WantedLink {
    pub calendar_id: String,
    pub event_id: String,
    pub block_id: String,
    pub phase: LinkPhase,
    pub start_min: i32,
    pub end_min: i32,
    pub title: String,
    pub off_clock: bool,
    pub refiled: bool,
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

fn collect_wanted_links<C: ClockLookup + ?Sized>(
    input: &SyncPlanInput<'_>,
    working: &WorkingNote,
    plan: &SyncPlan,
    daily_rel: Option<&Path>,
    mut clock: Option<&mut C>,
) -> Vec<WantedLink> {
    let mut wanted: Vec<WantedLink> = Vec::new();
    for occurrence in &input.occurrences {
        let Some(phase) = occurrence.link_phase else {
            continue;
        };
        if !occurrence.task {
            continue;
        }
        let created =
            input.reset.is_some() || row_was_created(plan, occurrence);
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
        let Some(line_index) = find_row_line(
            &working.lines,
            event_task::events_section_range(&refs),
            &block_id,
            occurrence.url(),
            true,
            None,
        ) else {
            continue;
        };
        if pomodoro_link_present(&working.lines, &block_id) {
            continue;
        }
        let off_clock = resolved_off_clock(
            &mut clock,
            daily_rel,
            working.lines.get(line_index).map(String::as_str),
            occurrence.off_clock,
        );
        wanted.push(wanted_link(occurrence, block_id, phase, off_clock));
    }
    for (calendar_id, event_id, block_id) in &plan.extra_link_wants {
        if wanted.iter().any(|link| link.block_id == *block_id)
            || pomodoro_link_present(&working.lines, block_id)
        {
            continue;
        }
        let key = occurrence_key(calendar_id, event_id);
        if let Some(state) = input.journal.get(&(key, input.day_string.clone()))
            && (state.link || state.gone)
        {
            continue;
        }
        let refs = working.line_refs();
        let Some(line_index) = find_row_line(
            &working.lines,
            event_task::events_section_range(&refs),
            block_id,
            None,
            true,
            None,
        ) else {
            continue;
        };
        let Some(occurrence) = input.occurrences.iter().find(|occurrence| {
            occurrence.calendar_id == *calendar_id
                && occurrence.event_id == *event_id
        }) else {
            continue;
        };
        let Some(phase) = occurrence.link_phase else {
            continue;
        };
        let off_clock = resolved_off_clock(
            &mut clock,
            daily_rel,
            working.lines.get(line_index).map(String::as_str),
            occurrence.off_clock,
        );
        wanted.push(wanted_link(
            occurrence,
            block_id.clone(),
            phase,
            off_clock,
        ));
    }
    wanted
}

fn apply_effective_clocks<C: ClockLookup + ?Sized>(
    clock: &mut C,
    daily_rel: &Path,
    working: &WorkingNote,
    plan: &mut SyncPlan,
) {
    let mut resolve = |block_id: &str, url: Option<&str>| -> Option<bool> {
        let refs = working.line_refs();
        let index = find_row_line(
            &working.lines,
            event_task::events_section_range(&refs),
            block_id,
            url,
            true,
            None,
        )?;
        let line = working.lines.get(index)?;
        Some(clock.resolve_clock(daily_rel, line).off_clock)
    };
    for row in &mut plan.rows_added {
        if row.row_kind != "task" {
            continue;
        }
        if let Some(off_clock) = resolve(&row.block_id, row.url.as_deref()) {
            row.off_clock = off_clock;
        }
    }
    for row in &mut plan.rows_adopted {
        if row.row_kind != "task" {
            continue;
        }
        if let Some(off_clock) = resolve(&row.block_id, row.url.as_deref()) {
            row.off_clock = off_clock;
        }
    }
    for row in &mut plan.rows_updated {
        if row.row_kind != "task" {
            continue;
        }
        if let Some(off_clock) = resolve(&row.block_id, row.url.as_deref()) {
            row.off_clock = off_clock;
        }
    }
    for row in &mut plan.rows_held {
        if row.row_kind != "task" {
            continue;
        }
        if let Some(off_clock) = resolve(&row.block_id, row.url.as_deref()) {
            row.off_clock = off_clock;
        }
    }
    for row in &mut plan.rows_upgraded {
        if let Some(off_clock) = resolve(&row.block_id, row.url.as_deref()) {
            row.off_clock = off_clock;
        }
    }
}

fn reborrow_clock<'a, C: ClockLookup + ?Sized>(
    clock: &'a mut Option<&mut C>,
) -> Option<&'a mut C> {
    clock.as_mut().map(|clock| &mut **clock)
}

pub(super) fn resolved_off_clock<C: ClockLookup + ?Sized>(
    clock: &mut Option<&mut C>,
    daily_rel: Option<&Path>,
    line: Option<&str>,
    fallback: bool,
) -> bool {
    match (reborrow_clock(clock), daily_rel, line) {
        (Some(clock), Some(daily_rel), Some(line)) => {
            clock.resolve_clock(daily_rel, line).off_clock
        }
        _ => fallback,
    }
}

/// One MEET filing link: the buffered span is what the event occupies,
/// so clusters, running entries, and MEET spans use it while the bold
/// row label keeps the meeting span.
fn wanted_link(
    occurrence: &PlannedOccurrence,
    block_id: String,
    phase: LinkPhase,
    off_clock: bool,
) -> WantedLink {
    let (start_min, end_min) = event_task::buffered_timeline_range(
        occurrence.start_min,
        occurrence.end_min,
        occurrence.buffer,
    );
    WantedLink {
        calendar_id: occurrence.calendar_id.clone(),
        event_id: occurrence.event_id.clone(),
        block_id,
        phase,
        start_min,
        end_min,
        title: occurrence.title.clone(),
        off_clock,
        refiled: false,
    }
}

fn row_was_created(plan: &SyncPlan, occurrence: &PlannedOccurrence) -> bool {
    plan.rows_added.iter().any(|row| {
        row.calendar_id == occurrence.calendar_id
            && row.event_id == occurrence.event_id
            && row.row_kind == "task"
    }) || plan.rows_restored.iter().any(|row| {
        row.calendar_id == occurrence.calendar_id
            && row.event_id == occurrence.event_id
            && row.row_kind == "task"
    }) || plan.rows_upgraded.iter().any(|row| {
        row.calendar_id == occurrence.calendar_id
            && row.event_id == occurrence.event_id
    })
}

pub(super) fn cluster_past(mut items: Vec<WantedLink>) -> Vec<Vec<WantedLink>> {
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
            let cluster_off_clock =
                last.first().is_some_and(|link| link.off_clock);
            // Past clusters split by clock verdict: two closed MEETs may
            // overlap, and that is truthful (one on the clock, one off).
            if item.start_min <= cluster_end
                && item.off_clock == cluster_off_clock
            {
                last.push(item);
                continue;
            }
        }
        clusters.push(vec![item]);
    }
    clusters
}

pub(super) fn format_meet_span(
    start_min: i32,
    end_min: i32,
    off_clock: bool,
) -> (String, String) {
    let duration = (end_min - start_min).max(0);
    let start_mod = start_min.rem_euclid(1440);
    let end_mod = end_min.rem_euclid(1440);
    let start = format!("{:02}{:02}", start_mod / 60, start_mod % 60);
    let end = format!("{:02}{:02}", end_mod / 60, end_mod % 60);
    if off_clock {
        (
            format!("(**{start}-{end}** [off:: {duration}m])"),
            format!("{start}-{end}"),
        )
    } else {
        (
            format!("(**{start}-{end}** [t:: {duration}m])"),
            format!("{start}-{end}"),
        )
    }
}

fn status_for_phase(phase: Option<LinkPhase>) -> char {
    match phase {
        Some(LinkPhase::Past) => 'x',
        Some(LinkPhase::Current | LinkPhase::Future) => '*',
        None => ' ',
    }
}

fn closed_insert_at(contents: &str, cluster_start: i32) -> LineInsert {
    match ledger_place::closed_insert_at(contents, cluster_start) {
        LedgerInsert::Before(index) => LineInsert::Before(index),
        LedgerInsert::After(index) => LineInsert::After(index),
    }
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
            format!("🍅 ~~[[#^{}]]~~", link.block_id)
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
        let planned = PlannedLink {
            calendar_id: link.calendar_id.clone(),
            event_id: link.event_id.clone(),
            block_id: link.block_id.clone(),
        };
        if link.refiled {
            plan.links_refiled.push(planned);
        } else {
            plan.links_added.push(planned);
        }
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
    if let Some(restored) = plan.rows_restored.iter().find(|row| {
        row.calendar_id == occurrence.calendar_id
            && row.event_id == occurrence.event_id
    }) {
        return Some(restored.block_id.clone());
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
