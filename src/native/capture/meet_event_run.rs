//! Calendar-aware MEET starts: the event run a MEET session follows and the
//! clock rule every start-family write applies.
//!
//! A MEET session's lineup holds same-note links to Event Tasks. The **event
//! run** is the earliest open Event Task that has not ended plus every
//! following one that touches or overlaps it (`next.start <= run.end`, the
//! `cluster_by_clock` rule) and shares its clock verdict. Every span here is
//! the event's **buffered** span (`[buffer:: -30m +15m]` widens the meeting
//! by its lead-in and lead-out): candidates end after now, sort, cluster, and
//! the run's written span all use what the event occupies. An empty-suffix
//! start writes the run's span; a MEET with a non-empty run records `[off::]`
//! exactly when that run is off-clock. Every other session stamps the clock
//! from the counted-links rule: off the clock iff it keeps at least one Task
//! Link and every one resolves off-clock (`#off`, a deprecated alias, or an
//! inherited area/project `offclock: true`). The run is computed on
//! the staged daily note, so drops (`~K`) and earlier items of the same draft
//! are already applied.

use std::collections::HashSet;

use super::*;
use crate::native::capture_pomodoro_close::{clock_resolver, CloseVault};
use crate::native::capture_pomodoro_start::CountedLinkFact;
use crate::native::event_task::{
    clock_verdict, enrich_event_clocks, event_task_by_id, events_in_note,
    ClockReason, ClockVerdict, EventKind, EventSpan, EventTask,
};
use crate::native::pomodoro_homes::{via_clock_note, ClockResolution};
use crate::native::task_status_hooks::canonical_daily_date;

/// A run starting further than this from now adds a `pomodoro_event_run_later`
/// warning: a default Pomodoro would fit before it.
pub(super) const FAR_RUN_MINUTES: i32 = 25;

/// True for a session named MEET, the type Google Calendar sync writes and
/// the one whose lineup drives an event run.
pub(super) fn is_meet_name(name: Option<&str>) -> bool {
    name.is_some_and(|name| plan_budget::normalize_component(name) == "meet")
}

/// One lineup Event Task inside a run, with its 1-based lineup number.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct RunEvent {
    pub(super) number: u32,
    pub(super) event: EventTask,
}

impl RunEvent {
    /// The span the event occupies: the meeting plus its buffers.
    fn buffered_span(&self) -> EventSpan {
        self.event.buffered_span()
    }
}

/// The computed run, in minutes on the day's timeline (`end_min` may pass
/// 1440 for a midnight-crossing event). The span is the buffered one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct EventRun {
    pub(super) start_min: i32,
    pub(super) end_min: i32,
    pub(super) off_clock: bool,
    pub(super) events: Vec<RunEvent>,
}

/// Event run of the session headlined at `entry_index` in `contents`.
///
/// `numbers` are the pre-drop lineup numbers of the surviving links in
/// ledger order (a drop leaves gaps); `None` numbers the links 1..n.
#[allow(dead_code)]
pub(super) fn compute_event_run(
    day_relative: &Path,
    contents: &str,
    entry_index: usize,
    numbers: Option<&[u32]>,
    now_min: i32,
) -> Option<EventRun> {
    compute_event_run_with(
        day_relative,
        contents,
        entry_index,
        numbers,
        now_min,
        None::<&dyn CloseVault>,
    )
}

pub(super) fn compute_event_run_with<V: CloseVault + ?Sized>(
    day_relative: &Path,
    contents: &str,
    entry_index: usize,
    numbers: Option<&[u32]>,
    now_min: i32,
    vault: Option<&V>,
) -> Option<EventRun> {
    let links =
        capture_pomodoro_start::list_queued_links(contents, entry_index);
    let mut event_rows = canonical_daily_date(day_relative)
        .map(|date| events_in_note(contents, date))
        .unwrap_or_default();
    if let Some(vault) = vault {
        let mut resolver = clock_resolver(vault);
        enrich_event_clocks(
            day_relative,
            contents,
            &mut event_rows,
            &mut resolver,
        );
    }
    let mut seen = HashSet::new();
    let mut candidates: Vec<(usize, RunEvent)> = Vec::new();
    for (position, link) in links.iter().enumerate() {
        if link.embedded || !link.path_part.is_empty() {
            continue;
        }
        let Some(event) = event_task_from_rows(
            day_relative,
            contents,
            &event_rows,
            &link.block_id,
            vault.is_none(),
        ) else {
            continue;
        };
        let buffered = event.buffered_span();
        let start = i32::from(buffered.start_min);
        let end = buffered.end_on_timeline();
        if !event.is_open() || end <= now_min || end <= start {
            continue;
        }
        if !seen.insert(link.block_id.clone()) {
            continue;
        }
        let number = numbers
            .and_then(|numbers| numbers.get(position))
            .copied()
            .unwrap_or(link.index);
        candidates.push((position, RunEvent { number, event }));
    }
    candidates.sort_by_key(|(position, item)| {
        let buffered = item.buffered_span();
        (buffered.start_min, buffered.end_on_timeline(), *position)
    });
    let mut ordered = candidates.into_iter().map(|(_, item)| item);
    let first = ordered.next()?;
    let off_clock = first.event.off_clock;
    let start_min = i32::from(first.buffered_span().start_min);
    let mut end_min = first.buffered_span().end_on_timeline();
    let mut events = vec![first];
    for next in ordered {
        let buffered = next.buffered_span();
        if i32::from(buffered.start_min) > end_min
            || next.event.off_clock != off_clock
        {
            break;
        }
        end_min = end_min.max(buffered.end_on_timeline());
        events.push(next);
    }
    Some(EventRun {
        start_min,
        end_min,
        off_clock,
        events,
    })
}

fn event_task_from_rows(
    day_relative: &Path,
    contents: &str,
    events: &[crate::native::event_task::EventNoteRow],
    block_id: &str,
    tag_only: bool,
) -> Option<EventTask> {
    if tag_only {
        return event_task_by_id(day_relative, contents, block_id);
    }
    let mut found = None;
    for event in events {
        if event.kind != EventKind::Task {
            continue;
        }
        if event.block_id.as_deref() != Some(block_id) {
            continue;
        }
        if found.replace(event).is_some() {
            return None;
        }
    }
    found.and_then(crate::native::event_task::EventNoteRow::as_event_task)
}

fn start_suffix_too_large() -> CaptureError {
    CaptureError::usage(
        "Pomodoro start suffix is too large; use a smaller duration or offset",
    )
}

/// The unwrapped start minute a typed start writes: now minus `5·offset`,
/// rounded up to 5 minutes. Not the `HHMM` modulo 1440.
pub(crate) fn start_block_minute(
    now: chrono::NaiveDateTime,
    spec: &PomodoroStartSpec,
) -> Result<i64, CaptureError> {
    use chrono::Timelike;
    let offset_minutes = spec
        .offset_units
        .checked_mul(5)
        .ok_or_else(start_suffix_too_large)?;
    let offset_i64: i64 = offset_minutes
        .try_into()
        .map_err(|_| start_suffix_too_large())?;
    let now_minutes = (now.time().hour() * 60 + now.time().minute()) as i64;
    let numerator = now_minutes
        .checked_sub(offset_i64)
        .ok_or_else(start_suffix_too_large)?;
    let quot = if numerator >= 0 {
        numerator
            .checked_add(4)
            .ok_or_else(start_suffix_too_large)?
            / 5
    } else {
        numerator / 5
    };
    quot.checked_mul(5).ok_or_else(start_suffix_too_large)
}

/// Open untimed MEET placeholder whose event run overlaps `[S, S + 5)`.
/// Several matches take the earliest `run.start_min`, then ledger line.
#[allow(dead_code)]
pub(crate) fn meet_overlapping_start_block<'a>(
    day_relative: &Path,
    contents: &str,
    scan: &'a capture_pomodoros::PomodoroScan,
    start_min: i64,
    now: chrono::NaiveDateTime,
) -> Option<&'a capture_pomodoros::PomodoroEntry> {
    meet_overlapping_start_block_with(
        day_relative,
        contents,
        scan,
        start_min,
        now,
        None::<&dyn CloseVault>,
    )
}

pub(crate) fn meet_overlapping_start_block_with<'a, V: CloseVault + ?Sized>(
    day_relative: &Path,
    contents: &str,
    scan: &'a capture_pomodoros::PomodoroScan,
    start_min: i64,
    now: chrono::NaiveDateTime,
    vault: Option<&V>,
) -> Option<&'a capture_pomodoros::PomodoroEntry> {
    use chrono::Timelike;
    let start = i32::try_from(start_min).ok()?;
    let block_end = start.checked_add(5)?;
    let now_min =
        i32::try_from(now.hour() * 60 + now.minute()).unwrap_or_default();
    scan.entries
        .iter()
        .filter(|entry| {
            entry.state == capture_pomodoros::PomodoroState::Open
                && entry.placeholder
                && entry.time_range.is_none()
                && is_meet_name(entry.name.as_deref())
        })
        .filter_map(|entry| {
            let index = entry.line.checked_sub(1)?;
            let run = compute_event_run_with(
                day_relative,
                contents,
                index,
                None,
                now_min,
                vault,
            )?;
            (run.start_min < block_end && run.end_min > start).then_some((
                run.start_min,
                entry.line,
                entry,
            ))
        })
        .min_by_key(|(run_start, line, _)| (*run_start, *line))
        .map(|(_, _, entry)| entry)
}

/// Placeholder a bare unnamed start would take: a MEET whose event run
/// occupies the start block, else [`next_future_pomodoro`].
pub(crate) fn unnamed_start_target<'a>(
    day_relative: &Path,
    contents: &str,
    scan: &'a capture_pomodoros::PomodoroScan,
    now: chrono::NaiveDateTime,
    spec: &PomodoroStartSpec,
) -> Option<&'a capture_pomodoros::PomodoroEntry> {
    unnamed_start_target_with(
        day_relative,
        contents,
        scan,
        now,
        spec,
        None::<&dyn CloseVault>,
    )
}

pub(crate) fn unnamed_start_target_with<'a, V: CloseVault + ?Sized>(
    day_relative: &Path,
    contents: &str,
    scan: &'a capture_pomodoros::PomodoroScan,
    now: chrono::NaiveDateTime,
    spec: &PomodoroStartSpec,
    vault: Option<&V>,
) -> Option<&'a capture_pomodoros::PomodoroEntry> {
    if let Ok(start_min) = start_block_minute(now, spec)
        && let Some(entry) = meet_overlapping_start_block_with(
            day_relative,
            contents,
            scan,
            start_min,
            now,
            vault,
        )
    {
        return Some(entry);
    }
    capture_pomodoros::next_future_pomodoro(scan)
}

/// End, on the day's timeline, of today's latest closed timed session.
fn latest_closed_end(contents: &str) -> Option<i32> {
    let scan = capture_pomodoros::scan(contents);
    let lines = line_spans(contents);
    scan.entries
        .iter()
        .filter(|entry| {
            entry.state == capture_pomodoros::PomodoroState::Completed
        })
        .filter_map(|entry| {
            let text = lines.get(entry.line.checked_sub(1)?)?.text;
            let range = parse_adjustment_range(text)?;
            let start = i32::try_from(range.start_minutes).ok()?;
            let mut end = i32::try_from(range.end_minutes).ok()?;
            if end < start {
                end += 24 * 60;
            }
            Some(end)
        })
        .max()
}

/// How the far-run warning names the run's first event: its title, plus the
/// meeting span and buffer label when it has a buffer, so "starts at 1130"
/// is explained by `Lunch (1200-1300 −30m)`.
fn far_run_event_label(event: &EventTask) -> String {
    match EventBufferJson::new(event.span, event.buffer) {
        Some(buffer) => event_with_buffer_label(
            &event.title,
            &event.span.text(),
            Some(&buffer),
        ),
        None => event.title.clone(),
    }
}

fn hhmm(minute_of_timeline: i32) -> String {
    let minute = minute_of_timeline.rem_euclid(24 * 60);
    format!("{:02}{:02}", minute / 60, minute % 60)
}

fn humanize_minutes(minutes: i32) -> String {
    let (hours, rest) = (minutes / 60, minutes % 60);
    match (hours, rest) {
        (0, rest) => format!("{rest}m"),
        (hours, 0) => format!("{hours}h"),
        (hours, rest) => format!("{hours}h {rest}m"),
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct EventRunEventJson {
    pub(super) index: u32,
    pub(super) title: String,
    pub(super) time_range: String,
    pub(super) line: usize,
    pub(super) block_id: Option<String>,
    pub(super) off_clock: bool,
    pub(super) untracked: bool,
    /// The event's pre/post-event buffer; omitted when it has none.
    /// `time_range` above stays the meeting span.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) buffer: Option<EventBufferJson>,
}

/// `pomodoro_start.event_run`: the run a MEET start followed or overrode.
///
/// `time_range`/`start`/`end` are the run's span as the calendar occupies
/// it (buffered, after the late-join clamp), plain `HHMM-HHMM`; `source` says whether
/// the written session follows it (`calendar`) or a counted suffix or kept
/// ledger set its own span (`explicit`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct EventRunJson {
    pub(super) time_range: String,
    pub(super) start: String,
    pub(super) end: String,
    pub(super) off_clock: bool,
    pub(super) untracked: bool,
    pub(super) source: &'static str,
    pub(super) events: Vec<EventRunEventJson>,
}

impl EventRunJson {
    fn new(
        run: &EventRun,
        start_min: i32,
        source: &'static str,
    ) -> EventRunJson {
        let (start, end) = (hhmm(start_min), hhmm(run.end_min));
        EventRunJson {
            time_range: format!("{start}-{end}"),
            start,
            end,
            off_clock: run.off_clock,
            untracked: run.off_clock,
            source,
            events: run
                .events
                .iter()
                .map(|item| EventRunEventJson {
                    index: item.number,
                    title: item.event.title.clone(),
                    time_range: item.event.span.text(),
                    line: item.event.line_index + 1,
                    block_id: item.event.block_id.clone(),
                    off_clock: item.event.off_clock,
                    untracked: item.event.off_clock,
                    buffer: EventBufferJson::new(
                        item.event.span,
                        item.event.buffer,
                    ),
                })
                .collect(),
        }
    }
}

/// Whether the span of a started MEET follows the calendar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SpanRule {
    /// An empty suffix writes the run's span (plain start, named start,
    /// restart); a counted suffix keeps its explicit span.
    Calendar,
    /// The written span stays (kept-ledger swap, link start); only
    /// clock classification follows the run.
    Written,
}

pub(super) struct MeetStartInput<'a> {
    pub(super) day_relative: &'a Path,
    /// 0-based headline of the started session.
    pub(super) entry_index: usize,
    pub(super) spec: &'a PomodoroStartSpec,
    pub(super) span_rule: SpanRule,
    pub(super) numbers: Option<&'a [u32]>,
    pub(super) now: chrono::NaiveDateTime,
    /// Staged vault view for other-note Task Links. Same-note Event Tasks
    /// resolve from `contents` even when this is `None`.
    pub(super) vault: Option<&'a dyn CloseVault>,
    /// When true, a MEET with a non-empty event run uses the run's clock
    /// instead of the full lineup (start-family writes). Running lineup
    /// edits pass false so an added or dropped link can restamp the key.
    pub(super) use_event_run: bool,
}

/// What a finished start looks like after the MEET rule.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct MeetFinish {
    pub(super) contents: String,
    /// Replacement `(…)` ledger when the rule changed the written bytes.
    pub(super) time_range: Option<String>,
    pub(super) start: Option<String>,
    pub(super) end: Option<String>,
    pub(super) duration_minutes: Option<u64>,
    pub(super) offset_units: Option<u64>,
    pub(super) tracked: bool,
    pub(super) off_clock: bool,
    pub(super) clock_note: Option<String>,
    pub(super) event_run: Option<EventRunJson>,
    pub(super) warnings: Vec<CaptureWarning>,
}

impl MeetFinish {
    /// Fold the outcome into a start summary built from the standard span
    /// and hand back the warnings the rule raised.
    pub(super) fn apply(
        self,
        summary: &mut PomodoroStartSummary,
    ) -> Vec<CaptureWarning> {
        if let Some(time_range) = self.time_range {
            summary.time_range = time_range;
        }
        if let Some(start) = self.start {
            summary.start = start;
        }
        if let Some(end) = self.end {
            summary.end = end;
        }
        if let Some(duration) = self.duration_minutes {
            summary.duration_minutes = duration;
        }
        if let Some(offset) = self.offset_units {
            summary.offset_units = offset;
        }
        summary.tracked = self.tracked;
        summary.off_clock = self.off_clock;
        summary.clock_note = self.clock_note;
        summary.event_run = self.event_run;
        self.warnings
    }
}

/// Re-stamp the running session's clock key from its current counted
/// lineup. Used after a link is added to or dropped from an already-timed
/// session so the Now card stays honest. Does not rewrite the span.
pub(super) fn restamp_session_clock(
    planner: &mut CaptureBatchPlanner,
    bob_dir: &Path,
    day_file: &Path,
    day_relative: &Path,
    entry_line: usize,
    now: chrono::NaiveDateTime,
    warnings: &mut Vec<CaptureWarning>,
) -> Result<(), CaptureError> {
    let contents = planner.read_existing(day_file)?;
    let entry_index = entry_line.saturating_sub(1);
    let headline = line_spans(&contents)
        .get(entry_index)
        .map(|line| line.text)
        .unwrap_or("");
    if parse_adjustment_range(headline).is_none() {
        return Ok(());
    }
    let spec = PomodoroStartSpec {
        raw: String::new(),
        duration_units: 5,
        offset_units: 0,
        drop: Vec::new(),
    };
    let (updated, finish) = {
        let vault = SnapshotCloseVault::from_planner(planner, bob_dir);
        apply_meet_rule(
            contents,
            &MeetStartInput {
                day_relative,
                entry_index,
                spec: &spec,
                span_rule: SpanRule::Written,
                numbers: None,
                now,
                vault: Some(&vault),
                use_event_run: false,
            },
        )?
    };
    if let Some(finish) = finish {
        let rewritten = finish.time_range.is_some();
        warnings.extend(finish.warnings);
        if rewritten {
            planner.stage(day_file, updated)?;
        }
    }
    Ok(())
}

/// [`finalize_session_clock`] that hands back the final contents, taking
/// them out of the finish so the caller keeps one owner.
pub(super) fn apply_meet_rule(
    contents: String,
    input: &MeetStartInput<'_>,
) -> Result<(String, Option<MeetFinish>), CaptureError> {
    match finalize_session_clock(&contents, input)? {
        Some(mut finish) => {
            let updated = std::mem::take(&mut finish.contents);
            Ok((updated, Some(finish)))
        }
        None => Ok((contents, None)),
    }
}

/// Apply the session clock rule to the session at `input.entry_index`.
///
/// A MEET with a non-empty event run uses the run's clock (and may rewrite
/// the calendar span). Every other session stamps `[t::]` / `[off::]` from
/// the counted-links verdict. Returns `Ok(None)` when the ledger is already
/// on the worked clock, needs no rewrite, and raises no mixed warning.
pub(super) fn finalize_session_clock(
    contents: &str,
    input: &MeetStartInput<'_>,
) -> Result<Option<MeetFinish>, CaptureError> {
    use chrono::Timelike;
    let spans = line_spans(contents);
    let Some(headline) = spans.get(input.entry_index).map(|line| line.text)
    else {
        return Ok(None);
    };
    let scan = capture_pomodoros::scan(contents);
    let Some(entry) = scan
        .entries
        .iter()
        .find(|entry| entry.line == input.entry_index + 1)
    else {
        return Ok(None);
    };
    let ledger_clock = session_clock(headline);
    let ledger_stamp = clock_stamp(headline);
    let meet = is_meet_name(entry.name.as_deref());
    let now_min = i32::try_from(input.now.hour() * 60 + input.now.minute())
        .unwrap_or_default();
    let run = if meet && input.use_event_run {
        compute_event_run_with(
            input.day_relative,
            contents,
            input.entry_index,
            input.numbers,
            now_min,
            input.vault,
        )
    } else {
        None
    };
    let Some(run) = run else {
        return finalize_lineup_clock(
            contents,
            input,
            headline,
            ledger_clock,
            ledger_stamp,
            entry,
        );
    };

    let off_clock = run.off_clock;
    let tracked = !off_clock;
    let clock_note = event_run_clock_note(&run);
    let calendar =
        input.span_rule == SpanRule::Calendar && input.spec.raw.is_empty();
    let closed_end = latest_closed_end(contents);
    let clamped_start = match closed_end {
        Some(end) if end > run.start_min && end < run.end_min => end,
        _ => run.start_min,
    };
    let source = if calendar { "calendar" } else { "explicit" };
    let event_run = Some(EventRunJson::new(&run, clamped_start, source));

    let mut warnings = event_run_clock_warnings(&run);
    if calendar {
        let duration =
            u64::try_from(run.end_min - clamped_start).unwrap_or_default();
        let range = format!("{}-{}", hhmm(clamped_start), hhmm(run.end_min));
        let time_range = if tracked {
            format!("(**{range}** [t:: {duration}m])")
        } else {
            format!("(**{range}** [off:: {duration}m])")
        };
        let updated =
            splice_session_ledger(contents, input.entry_index, &time_range)?;
        if clamped_start - now_min > FAR_RUN_MINUTES {
            let title = run
                .events
                .first()
                .map(|item| far_run_event_label(&item.event))
                .unwrap_or_default();
            warnings.push(CaptureWarning::Structured(
                StructuredCaptureWarning {
                    code: "pomodoro_event_run_later",
                    label: None,
                    count: None,
                    message: format!(
                        "MEET's next event, {title}, starts at {} ({} from now) · capture `=5#meet` to start 25 minutes now",
                        hhmm(clamped_start),
                        humanize_minutes(clamped_start - now_min),
                    ),
                },
            ));
        }
        return Ok(Some(MeetFinish {
            contents: updated,
            time_range: Some(time_range),
            start: Some(hhmm(clamped_start)),
            end: Some(hhmm(run.end_min)),
            duration_minutes: Some(duration),
            offset_units: Some(0),
            tracked,
            off_clock,
            clock_note: clock_note.clone(),
            event_run,
            warnings,
        }));
    }

    let already_on_requested_clock = match (ledger_clock, off_clock) {
        (SessionClock::Worked { .. }, false) => {
            ledger_stamp != ClockStamp::Conflict
        }
        (SessionClock::OffClock { stamped: true, .. }, true) => true,
        _ => false,
    };
    if already_on_requested_clock {
        return Ok(Some(MeetFinish {
            contents: contents.to_string(),
            time_range: None,
            start: None,
            end: None,
            duration_minutes: None,
            offset_units: None,
            tracked,
            off_clock,
            clock_note,
            event_run,
            warnings,
        }));
    }
    let ledger = read_session_ledger_bytes(headline).ok_or_else(|| {
        CaptureError::io(
            "running Pomodoro timing could not be safely identified",
        )
    })?;
    let clock = if off_clock {
        ClockKind::OffClock
    } else {
        ClockKind::Worked
    };
    let retracked = set_ledger_clock(&ledger, clock);
    let updated =
        splice_session_ledger(contents, input.entry_index, &retracked)?;
    Ok(Some(MeetFinish {
        contents: updated,
        time_range: Some(retracked),
        start: None,
        end: None,
        duration_minutes: None,
        offset_units: None,
        tracked,
        off_clock,
        clock_note,
        event_run,
        warnings,
    }))
}

fn event_run_clock_warnings(run: &EventRun) -> Vec<CaptureWarning> {
    let links: Vec<_> = run
        .events
        .iter()
        .map(|item| CountedLinkFact {
            fact: crate::native::event_task::LinkFact {
                off_clock: Some(item.event.off_clock),
            },
            block_link: item
                .event
                .block_id
                .as_deref()
                .map(|id| format!("[[#^{id}]]"))
                .unwrap_or_default(),
            text: Some(item.event.title.clone()),
            resolution: Some(ClockResolution {
                off_clock: item.event.off_clock,
                source: item.event.clock_source,
                note: item.event.clock_note.clone(),
                conflict: matches!(
                    item.event.clock_tag,
                    crate::native::clock_tags::ClockTag::Conflict { .. }
                ),
            }),
        })
        .collect();
    clock_link_warnings(&links)
}

fn event_run_clock_note(run: &EventRun) -> Option<String> {
    let resolutions: Vec<_> = run
        .events
        .iter()
        .map(|item| ClockResolution {
            off_clock: item.event.off_clock,
            source: item.event.clock_source,
            note: item.event.clock_note.clone(),
            conflict: matches!(
                item.event.clock_tag,
                crate::native::clock_tags::ClockTag::Conflict { .. }
            ),
        })
        .collect();
    via_clock_note(run.off_clock, resolutions.iter())
}

fn finalize_lineup_clock(
    contents: &str,
    input: &MeetStartInput<'_>,
    headline: &str,
    ledger_clock: SessionClock,
    ledger_stamp: ClockStamp,
    entry: &capture_pomodoros::PomodoroEntry,
) -> Result<Option<MeetFinish>, CaptureError> {
    let links = capture_pomodoro_start::resolve_session_clock_links(
        input.vault,
        input.day_relative,
        contents,
        input.entry_index,
    );
    let facts: Vec<_> = links.iter().map(|link| link.fact).collect();
    let verdict = clock_verdict(&facts);
    let off_clock = matches!(verdict, ClockVerdict::OffClock);
    let clock_note = via_clock_note(
        off_clock,
        links.iter().filter_map(|link| link.resolution.as_ref()),
    );
    let mut warnings =
        mixed_clock_warning(entry.name.as_deref(), &links, verdict)
            .into_iter()
            .collect::<Vec<_>>();
    warnings.extend(clock_link_warnings(&links));
    let already_on_requested_clock = match (ledger_clock, off_clock) {
        (SessionClock::Worked { .. }, false) => {
            ledger_stamp != ClockStamp::Conflict
        }
        (SessionClock::OffClock { stamped: true, .. }, true) => true,
        _ => false,
    };
    if already_on_requested_clock && warnings.is_empty() && !off_clock {
        return Ok(None);
    }
    if already_on_requested_clock {
        return Ok(Some(MeetFinish {
            contents: contents.to_string(),
            time_range: None,
            start: None,
            end: None,
            duration_minutes: None,
            offset_units: None,
            tracked: !off_clock,
            off_clock,
            clock_note,
            event_run: None,
            warnings,
        }));
    }
    let ledger = read_session_ledger_bytes(headline).ok_or_else(|| {
        CaptureError::io(
            "running Pomodoro timing could not be safely identified",
        )
    })?;
    let clock = if off_clock {
        ClockKind::OffClock
    } else {
        ClockKind::Worked
    };
    let restamped = set_ledger_clock(&ledger, clock);
    let updated =
        splice_session_ledger(contents, input.entry_index, &restamped)?;
    Ok(Some(MeetFinish {
        contents: updated,
        time_range: Some(restamped),
        start: None,
        end: None,
        duration_minutes: None,
        offset_units: None,
        tracked: !off_clock,
        off_clock,
        clock_note,
        event_run: None,
        warnings,
    }))
}

fn clock_link_warnings(links: &[CountedLinkFact]) -> Vec<CaptureWarning> {
    let mut warnings = Vec::new();
    let mut seen = HashSet::new();
    for link in links {
        let Some(resolution) = link.resolution.as_ref() else {
            continue;
        };
        let Some(code) = resolution.capture_warning_code() else {
            continue;
        };
        if !seen.insert((code, link.block_link.clone())) {
            continue;
        }
        let message = match code {
            "clock_tag_conflict" => {
                let label = link
                    .text
                    .as_deref()
                    .map(clock_link_label)
                    .filter(|label| !label.is_empty());
                match label {
                    Some(label) => {
                        format!("◌ {label} has both #on and #off; #on wins")
                    }
                    None => format!(
                        "◌ {} has both #on and #off; #on wins",
                        link.block_link
                    ),
                }
            }
            "note_offclock_invalid" => {
                let note = resolution.note.as_deref().unwrap_or("a note");
                format!(
                    "◌ {note} has an invalid offclock: value; that task stays on the clock"
                )
            }
            _ => continue,
        };
        warnings.push(CaptureWarning::Structured(StructuredCaptureWarning {
            code,
            label: None,
            count: None,
            message,
        }));
    }
    warnings
}

fn mixed_clock_warning(
    session_name: Option<&str>,
    links: &[CountedLinkFact],
    verdict: ClockVerdict,
) -> Option<CaptureWarning> {
    let has_off_clock =
        links.iter().any(|link| link.fact.off_clock == Some(true));
    if !has_off_clock || matches!(verdict, ClockVerdict::OffClock) {
        return None;
    }
    let name = session_name
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .unwrap_or("the session");
    let detail = match verdict {
        ClockVerdict::Worked {
            reason: ClockReason::Unresolved { first },
        } => {
            let link = links
                .get(first)
                .map(|link| link.block_link.as_str())
                .unwrap_or("a Task Link");
            format!("{link} does not resolve")
        }
        ClockVerdict::Worked {
            reason: ClockReason::Mixed { first_on_clock },
        } => {
            let label = links
                .get(first_on_clock)
                .and_then(|link| link.text.as_deref())
                .map(clock_link_label)
                .filter(|label| !label.is_empty());
            match label {
                Some(label) => format!("it also links {label}"),
                None => {
                    let link = links
                        .get(first_on_clock)
                        .map(|link| link.block_link.as_str())
                        .unwrap_or("another task");
                    format!("it also links {link}")
                }
            }
        }
        _ => return None,
    };
    Some(CaptureWarning::Structured(StructuredCaptureWarning {
        code: "pomodoro_clock_mixed",
        label: None,
        count: None,
        message: format!("◌ {name} stays on the clock: {detail}"),
    }))
}

fn clock_link_label(text: &str) -> String {
    text.split_whitespace()
        .filter(|token| {
            !token.starts_with('#')
                && !token.starts_with('[')
                && !token.starts_with('^')
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDateTime;

    fn rel() -> &'static Path {
        Path::new("2026/20261101.md")
    }

    fn day(events: &str, pomodoros: &str) -> String {
        format!("## Events\n{events}## Pomodoros\n{pomodoros}")
    }

    fn meet_index(note: &str) -> usize {
        note.lines()
            .position(|line| line.contains("— MEET"))
            .expect("MEET entry")
    }

    #[test]
    fn run_extends_over_touching_events_with_the_same_clock_classification() {
        let note = day(
            concat!(
                "- [ ] #task **1100-1130** One ^one\n",
                "- [ ] #task **1130-1200** Two ^two\n",
                "- [ ] #task **1200-1230** Lunch #untracked ^lunch\n",
                "- [ ] #task **1300-1330** Later ^later\n",
            ),
            concat!(
                "- [ ] () — MEET\n",
                "  - [[#^one]]\n",
                "  - [[#^two]]\n",
                "  - [[#^lunch]]\n",
                "  - [[#^later]]\n",
            ),
        );
        let run =
            compute_event_run(rel(), &note, meet_index(&note), None, 10 * 60)
                .unwrap();
        assert_eq!((run.start_min, run.end_min), (11 * 60, 12 * 60));
        assert!(!run.off_clock);
        let numbers: Vec<u32> =
            run.events.iter().map(|item| item.number).collect();
        assert_eq!(numbers, vec![1, 2]);
    }

    #[test]
    fn ended_done_cancelled_and_foreign_links_are_skipped() {
        let note = day(
            concat!(
                "- [ ] #task **0800-0830** Ended ^ended\n",
                "- [x] #task **1100-1130** Done ^done\n",
                "- [-] #task **1130-1200** Cancelled ^cancelled\n",
                "- [ ] #task **1200-1230** Live ^live\n",
            ),
            concat!(
                "- [ ] () — MEET\n",
                "  - [[#^ended]]\n",
                "  - [[#^done]]\n",
                "  - [[#^cancelled]]\n",
                "  - [[#^live]]\n",
                "  - [[sase#^live]]\n",
            ),
        );
        let run =
            compute_event_run(rel(), &note, meet_index(&note), None, 10 * 60)
                .unwrap();
        assert_eq!((run.start_min, run.end_min), (12 * 60, 12 * 60 + 30));
        assert_eq!(run.events.len(), 1);
        assert_eq!(run.events[0].number, 4);
    }

    #[test]
    fn dropped_numbers_keep_pre_image_numbering() {
        let note = day(
            "- [ ] #task **1100-1130** Two ^two\n",
            "- [ ] () — MEET\n  - [[#^two]]\n",
        );
        let run = compute_event_run(
            rel(),
            &note,
            meet_index(&note),
            Some(&[2]),
            10 * 60,
        )
        .unwrap();
        assert_eq!(run.events[0].number, 2);
    }

    #[test]
    fn untracked_run_breaks_at_a_contiguous_tracked_event() {
        let note = day(
            concat!(
                "- [ ] #task **1200-1300** Lunch #untracked ^lunch\n",
                "- [ ] #task **1300-1330** Sync ^sync\n",
            ),
            "- [ ] () — MEET\n  - [[#^lunch]]\n  - [[#^sync]]\n",
        );
        let run =
            compute_event_run(rel(), &note, meet_index(&note), None, 10 * 60)
                .unwrap();
        assert!(run.off_clock);
        assert_eq!((run.start_min, run.end_min), (12 * 60, 13 * 60));
        assert_eq!(run.events.len(), 1);
    }

    #[test]
    fn finalize_writes_the_calendar_span_and_clamps_late_joins() {
        let note = day(
            "- [ ] #task **1300-1400** Review ^review\n",
            concat!(
                "- [x] (**1250-1310** [t:: 20m]) — BOB\n",
                "- [ ] (**1315-1340** [t:: 25m]) — MEET\n",
                "  - [[#^review]]\n",
            ),
        );
        let spec = PomodoroStartSpec {
            raw: String::new(),
            duration_units: 0,
            offset_units: 0,
            drop: Vec::new(),
        };
        let now = NaiveDate::from_ymd_opt(2026, 11, 1)
            .unwrap()
            .and_hms_opt(13, 15, 0)
            .unwrap();
        let finish = finalize_session_clock(
            &note,
            &MeetStartInput {
                day_relative: rel(),
                entry_index: meet_index(&note),
                spec: &spec,
                span_rule: SpanRule::Calendar,
                numbers: None,
                now,
                vault: None,
                use_event_run: true,
            },
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            finish.time_range.as_deref(),
            Some("(**1310-1400** [t:: 50m])")
        );
        assert!(finish.tracked);
        assert!(finish.warnings.is_empty());
        let run = finish.event_run.unwrap();
        assert_eq!(run.time_range, "1310-1400");
        assert_eq!(run.source, "calendar");
    }

    #[test]
    fn buffered_spans_drive_candidates_sort_and_clustering() {
        let note = day(
            concat!(
                "- [ ] #task **1045-1115** One on one ^one\n",
                "- [ ] #task **1000-1030** Standup [buffer:: +15m] ^standup\n",
                "- [ ] #task **1200-1230** Lunch [buffer:: -30m] #untracked ^lunch\n",
            ),
            concat!(
                "- [ ] () — MEET\n",
                "  - [[#^one]]\n",
                "  - [[#^standup]]\n",
                "  - [[#^lunch]]\n",
            ),
        );
        // The +15 lead-out bridges Standup to the 1:1; the untracked lunch
        // (buffered 1130-1230) never joins a tracked run.
        let run =
            compute_event_run(rel(), &note, meet_index(&note), None, 9 * 60)
                .unwrap();
        assert_eq!((run.start_min, run.end_min), (10 * 60, 11 * 60 + 15));
        let numbers: Vec<u32> =
            run.events.iter().map(|item| item.number).collect();
        assert_eq!(numbers, vec![2, 1]);
        // 10:35 is inside Standup's lead-out: it still counts as the run.
        let in_lead_out = compute_event_run(
            rel(),
            &note,
            meet_index(&note),
            None,
            10 * 60 + 35,
        )
        .unwrap();
        assert_eq!(in_lead_out.start_min, 10 * 60);
        // 11:20 is past the 1:1 (1115): the run moves to the lunch lead-in.
        let later = compute_event_run(
            rel(),
            &note,
            meet_index(&note),
            None,
            11 * 60 + 20,
        )
        .unwrap();
        assert_eq!(
            (later.start_min, later.end_min),
            (11 * 60 + 30, 12 * 60 + 30)
        );
        assert!(later.off_clock);
    }

    #[test]
    fn far_run_label_adds_the_span_and_buffer_only_when_buffered() {
        let note = day(
            concat!(
                "- [ ] #task **1200-1300** Lunch [buffer:: -30m] ^lunch\n",
                "- [ ] #task **1400-1430** Plain ^plain\n",
            ),
            "- [ ] () — MEET\n  - [[#^lunch]]\n  - [[#^plain]]\n",
        );
        let buffered = event_task_by_id(rel(), &note, "lunch").unwrap();
        assert_eq!(
            far_run_event_label(&buffered),
            "Lunch (1200-1300 \u{2212}30m)"
        );
        let plain = event_task_by_id(rel(), &note, "plain").unwrap();
        assert_eq!(far_run_event_label(&plain), "Plain");
    }

    fn at(hour: u32, minute: u32) -> NaiveDateTime {
        NaiveDate::from_ymd_opt(2026, 11, 1)
            .unwrap()
            .and_hms_opt(hour, minute, 0)
            .unwrap()
    }

    fn bare_spec() -> PomodoroStartSpec {
        PomodoroStartSpec {
            raw: String::new(),
            duration_units: 5,
            offset_units: 0,
            drop: Vec::new(),
        }
    }

    fn target_name(note: &str, hour: u32, minute: u32) -> Option<String> {
        let scan = capture_pomodoros::scan(note);
        unnamed_start_target(rel(), note, &scan, at(hour, minute), &bare_spec())
            .and_then(|entry| entry.name.clone())
    }

    fn swings_day() -> String {
        day(
            "- [ ] #task **0800-0815** Swings! #offclock ^swings\n",
            concat!(
                "- [x] (**0755-0800** [t:: 5m]) — BUILD\n",
                "- [ ] () — BUILD\n",
                "- [ ] () — MEET\n",
                "  - [[#^swings]]\n",
                "- [ ] () — PLAN\n",
            ),
        )
    }

    #[test]
    fn unnamed_start_picks_meet_when_the_start_block_overlaps_the_run() {
        let note = swings_day();
        assert_eq!(
            start_block_minute(at(7, 54), &bare_spec()).unwrap(),
            7 * 60 + 55
        );
        assert_eq!(
            start_block_minute(at(7, 57), &bare_spec()).unwrap(),
            8 * 60
        );
        assert_eq!(
            start_block_minute(at(8, 4), &bare_spec()).unwrap(),
            8 * 60 + 5
        );
        assert_eq!(
            start_block_minute(at(8, 12), &bare_spec()).unwrap(),
            8 * 60 + 15
        );
        assert_eq!(target_name(&note, 7, 54).as_deref(), Some("BUILD"));
        assert_eq!(target_name(&note, 7, 57).as_deref(), Some("MEET"));
        assert_eq!(target_name(&note, 8, 4).as_deref(), Some("MEET"));
        assert_eq!(target_name(&note, 8, 12).as_deref(), Some("BUILD"));
    }

    #[test]
    fn unnamed_start_uses_buffered_lead_in_and_lead_out() {
        let note = day(
            "- [ ] #task **1200-1300** Lunch [buffer:: -30m +15m] ^lunch\n",
            concat!(
                "- [ ] () — BUILD\n",
                "- [ ] () — MEET\n",
                "  - [[#^lunch]]\n",
            ),
        );
        assert_eq!(target_name(&note, 11, 24).as_deref(), Some("BUILD"));
        assert_eq!(target_name(&note, 11, 27).as_deref(), Some("MEET"));
        assert_eq!(target_name(&note, 13, 10).as_deref(), Some("MEET"));
        assert_eq!(target_name(&note, 13, 14).as_deref(), Some("BUILD"));
    }

    #[test]
    fn unnamed_start_skips_ended_and_done_events() {
        let ended = day(
            "- [ ] #task **0800-0815** Swings! ^swings\n",
            concat!(
                "- [ ] () — BUILD\n",
                "- [ ] () — MEET\n",
                "  - [[#^swings]]\n",
            ),
        );
        assert_eq!(target_name(&ended, 8, 20).as_deref(), Some("BUILD"));
        let done = day(
            "- [x] #task **0800-0815** Swings! ^swings\n",
            concat!(
                "- [ ] () — BUILD\n",
                "- [ ] () — MEET\n",
                "  - [[#^swings]]\n",
            ),
        );
        assert_eq!(target_name(&done, 8, 4).as_deref(), Some("BUILD"));
    }

    #[test]
    fn unnamed_start_picks_the_earliest_overlapping_meet() {
        let note = day(
            concat!(
                "- [ ] #task **0805-0900** Later ^later\n",
                "- [ ] #task **0800-0815** Swings! ^swings\n",
            ),
            concat!(
                "- [ ] () — BUILD\n",
                "- [ ] () — MEET\n",
                "  - [[#^later]]\n",
                "- [ ] () — MEET\n",
                "  - [[#^swings]]\n",
            ),
        );
        let scan = capture_pomodoros::scan(&note);
        let meet_lines: Vec<usize> = note
            .lines()
            .enumerate()
            .filter(|(_, line)| line.contains("— MEET"))
            .map(|(index, _)| index + 1)
            .collect();
        assert_eq!(meet_lines.len(), 2);
        let at_0804 =
            unnamed_start_target(rel(), &note, &scan, at(8, 4), &bare_spec())
                .expect("MEET");
        // Both runs overlap [0805, 0810); the 0800 run wins over 0805.
        assert_eq!(at_0804.line, meet_lines[1]);
        let at_0820 =
            unnamed_start_target(rel(), &note, &scan, at(8, 20), &bare_spec())
                .expect("later MEET");
        assert_eq!(at_0820.line, meet_lines[0]);
    }

    #[test]
    fn unnamed_start_without_a_meet_keeps_the_first_future() {
        let note = day(
            "- [ ] #task **0800-0815** Swings! ^swings\n",
            "- [ ] () — BUILD\n- [ ] () — PLAN\n",
        );
        assert_eq!(target_name(&note, 8, 4).as_deref(), Some("BUILD"));
    }
}
