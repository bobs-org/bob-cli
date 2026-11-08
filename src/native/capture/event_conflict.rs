//! Run-past-event and mid-meeting warnings for non-MEET starts, extends,
//! and shifts.
//!
//! After an operator writes a new session span, open Event Tasks whose
//! occupied span (the meeting plus its `[buffer::]` lead-in and lead-out)
//! newly conflicts with the new `[S, E)` raise one structured
//! `pomodoro_runs_past_event` warning naming the earliest conflict, plus an
//! additive `event_conflict` object on the start, adjust, or shift JSON. A
//! start-family write whose written start is strictly inside an open Event
//! Task's meeting span raises `pomodoro_starts_during_event` plus an
//! additive `event_in_progress` object. MEET sessions and events already
//! in the session's own lineup are exempt. `-N` and closes never warn.
//! Untracked Event Tasks warn like any other open Event Task. Detection,
//! overlap, and fit all come from the shared [`event_span_conflicts`]
//! predicate; mid-meeting uses [`events_in_progress_at`].

use std::collections::HashSet;

use super::*;
use crate::native::capture_pomodoro_close::{clock_resolver, CloseVault};
use crate::native::event_task::{
    enrich_event_clocks, event_span_conflicts, events_in_note,
    events_in_progress_at, BufferPart, EventBuffer, EventNoteRow, EventSpan,
    EventSpanConflict,
};

/// How to spell the largest same-operator count that still ends at or
/// before the crossed event's start (`=2`, `=2#bugs`, `+K`, `++K`,
/// `--K`). `None` when no positive count fits.
#[derive(Debug, Clone)]
pub(super) enum FitKind {
    Start {
        spec: PomodoroStartSpec,
        name: Option<String>,
    },
    AdjustPlus {
        old_end: u16,
    },
    ShiftLater {
        old_end: u16,
    },
    ShiftEarlier {
        old_start: u16,
    },
}

/// `buffer` object on capture payloads: an Event Task's pre/post-event
/// reserve and the buffered span it occupies. Absent when the event has no
/// buffer, so unbuffered payloads stay byte-identical.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct EventBufferJson {
    pub(super) before_minutes: u16,
    pub(super) after_minutes: u16,
    pub(super) time_range: String,
}

impl EventBufferJson {
    pub(super) fn new(span: EventSpan, buffer: EventBuffer) -> Option<Self> {
        if buffer.is_empty() {
            return None;
        }
        Some(Self {
            before_minutes: buffer.before_min,
            after_minutes: buffer.after_min,
            time_range: span.buffered(buffer).text(),
        })
    }

    pub(super) fn buffer(&self) -> EventBuffer {
        EventBuffer {
            before_min: self.before_minutes,
            after_min: self.after_minutes,
        }
    }

    /// Human label, `−30m +15m`, with a typographic minus.
    pub(super) fn label(&self) -> String {
        self.buffer().label().unwrap_or_default()
    }

    /// Label of just one side, `−30m` or `+15m`.
    fn part_label(&self, part: BufferPart) -> String {
        let buffer = self.buffer();
        match part {
            BufferPart::Before => EventBuffer {
                after_min: 0,
                ..buffer
            },
            BufferPart::After => EventBuffer {
                before_min: 0,
                ..buffer
            },
        }
        .label()
        .unwrap_or_default()
    }
}

/// The human spelling of an event inside a message: the title, then its
/// meeting span, then the buffer label when it has one.
/// `Lunch (1200-1300 −30m +15m)`.
pub(super) fn event_with_buffer_label(
    title: &str,
    time_range: &str,
    buffer: Option<&EventBufferJson>,
) -> String {
    match buffer {
        Some(buffer) => format!("{title} ({time_range} {})", buffer.label()),
        None => format!("{title} ({time_range})"),
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct EventConflictEventJson {
    pub(super) title: String,
    pub(super) time_range: String,
    pub(super) line: usize,
    pub(super) off_clock: bool,
    /// Deprecated f6 field, retained for one release train.
    pub(super) untracked: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) buffer: Option<EventBufferJson>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct EventConflictJson {
    pub(super) event: EventConflictEventJson,
    pub(super) overlap_minutes: u64,
    /// `before` or `after` when the overlap lies wholly inside one buffer
    /// of the event; omitted otherwise.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) buffer_part: Option<BufferPart>,
    #[serde(skip_serializing_if = "is_zero_more")]
    pub(super) more: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) fit: Option<String>,
}

fn is_zero_more(value: &u64) -> bool {
    *value == 0
}

/// One open Event Task whose meeting is in progress at the written start.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct EventInProgressEventJson {
    pub(super) title: String,
    pub(super) time_range: String,
    pub(super) line: usize,
    pub(super) off_clock: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) buffer: Option<EventBufferJson>,
}

/// `pomodoro_start.event_in_progress`: a non-MEET session whose written
/// start sits strictly inside an open Event Task's meeting span.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct EventInProgressJson {
    pub(super) event: EventInProgressEventJson,
    #[serde(skip_serializing_if = "is_zero_more")]
    pub(super) more: u64,
}

/// Parse an `HHMM` clock into minutes past midnight.
pub(super) fn parse_clock(value: &str) -> Option<u16> {
    if value.len() != 4 || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let hour = value[..2].parse::<u16>().ok()?;
    let minute = value[2..].parse::<u16>().ok()?;
    (hour <= 23 && minute <= 59).then_some(hour * 60 + minute)
}

pub(super) fn clock_span(start: &str, end: &str) -> Option<(u16, u16)> {
    Some((parse_clock(start)?, parse_clock(end)?))
}

fn minutes_ahead(from: u16, to: u16) -> i32 {
    let mut delta = i32::from(to) - i32::from(from);
    if delta < 0 {
        delta += 24 * 60;
    }
    delta
}

fn fit_token(
    kind: &FitKind,
    session_start: u16,
    event_start: u16,
) -> Option<String> {
    match kind {
        FitKind::Start { spec, name } => {
            let count = minutes_ahead(session_start, event_start) / 5;
            if count <= 0 {
                return None;
            }
            Some(start_fit_token(
                u64::try_from(count).ok()?,
                spec,
                name.as_deref(),
            ))
        }
        FitKind::AdjustPlus { old_end } | FitKind::ShiftLater { old_end } => {
            let count = minutes_ahead(*old_end, event_start) / 5;
            if count <= 0 {
                return None;
            }
            let count = u64::try_from(count).ok()?;
            Some(match kind {
                FitKind::AdjustPlus { .. } => format!("+{count}"),
                _ => format!("++{count}"),
            })
        }
        FitKind::ShiftEarlier { old_start } => {
            let gap = minutes_ahead(event_start, *old_start);
            if gap <= 0 {
                return None;
            }
            let count = (gap - 1) / 5;
            if count <= 0 {
                return None;
            }
            Some(format!("--{}", u64::try_from(count).ok()?))
        }
    }
}

fn start_fit_token(
    units: u64,
    spec: &PomodoroStartSpec,
    name: Option<&str>,
) -> String {
    let timing = if spec.offset_units == 0 && spec.later_units == 0 {
        units.to_string()
    } else if spec.later_units > 0 {
        format!("{units}+{}", spec.later_units)
    } else {
        format!("{units}-{}", spec.offset_units)
    };
    match name.map(str::trim).filter(|name| !name.is_empty()) {
        Some(name) => capture_language::glued_named_start("", name, &timing),
        None => format!("={timing}"),
    }
}

fn lineup_exclude_ids(contents: &str, entry_index: usize) -> HashSet<String> {
    capture_pomodoro_start::list_queued_links(contents, entry_index)
        .into_iter()
        .map(|link| link.block_id)
        .collect()
}

/// The start of the span a fit draft must end at, as an `HHMM` clock
/// minute. Unbuffered events fit against the meeting start, as always. A
/// buffered event fits against its buffered start, and only when the
/// session starts before it; the predicate withholds the target otherwise.
/// `--N` is the exception: it keeps the session's start mid-meeting
/// (`start < S < end`, which never warns), so it measures from the meeting
/// start.
fn events_for_clock<V: CloseVault + ?Sized>(
    contents: &str,
    date: NaiveDate,
    vault: Option<&V>,
    day_relative: Option<&Path>,
) -> Vec<EventNoteRow> {
    let mut events = events_in_note(contents, date);
    if let (Some(vault), Some(day_relative)) = (vault, day_relative) {
        let mut resolver = clock_resolver(vault);
        enrich_event_clocks(day_relative, contents, &mut events, &mut resolver);
    }
    events
}

fn fit_target(
    kind: &FitKind,
    event: &EventNoteRow,
    conflict: &EventSpanConflict,
) -> Option<u16> {
    let span = event.span?;
    if event.buffer.is_empty() || matches!(kind, FitKind::ShiftEarlier { .. }) {
        return Some(span.start_min);
    }
    conflict.fit_start_min
}

/// Conflict for a non-MEET session whose new span newly conflicts with an
/// open Event Task's occupied span. `None` when the session is MEET or
/// nothing new conflicts.
#[allow(dead_code)]
pub(super) fn detect_event_conflict(
    contents: &str,
    date: NaiveDate,
    session_name: Option<&str>,
    entry_index: usize,
    new_start: u16,
    new_end: u16,
    old: Option<(u16, u16)>,
    fit: &FitKind,
) -> Option<EventConflictJson> {
    detect_event_conflict_with(
        contents,
        date,
        session_name,
        entry_index,
        new_start,
        new_end,
        old,
        fit,
        None::<&dyn CloseVault>,
        None,
    )
}

pub(super) fn detect_event_conflict_with<V: CloseVault + ?Sized>(
    contents: &str,
    date: NaiveDate,
    session_name: Option<&str>,
    entry_index: usize,
    new_start: u16,
    new_end: u16,
    old: Option<(u16, u16)>,
    fit: &FitKind,
    vault: Option<&V>,
    day_relative: Option<&Path>,
) -> Option<EventConflictJson> {
    if is_meet_name(session_name) {
        return None;
    }
    let events = events_for_clock(contents, date, vault, day_relative);
    let exclude = lineup_exclude_ids(contents, entry_index);
    let crossed =
        event_span_conflicts(new_start, new_end, old, &events, &exclude);
    let &(first, conflict) = crossed.first()?;
    let span = first.span?;
    let more = u64::try_from(crossed.len().saturating_sub(1)).unwrap_or(0);
    Some(EventConflictJson {
        event: EventConflictEventJson {
            title: first.title.clone(),
            time_range: span.text(),
            line: first.line_index + 1,
            off_clock: first.off_clock,
            untracked: first.off_clock,
            buffer: EventBufferJson::new(span, first.buffer),
        },
        overlap_minutes: conflict.overlap_minutes,
        buffer_part: conflict.buffer_part,
        more,
        fit: fit_target(fit, first, &conflict)
            .and_then(|target| fit_token(fit, new_start, target)),
    })
}

/// A non-MEET start whose written start sits strictly inside an open
/// Event Task's meeting span. `None` when the session is MEET or no
/// open meeting contains `S`. Does not apply an "only new" rule.
#[allow(dead_code)]
pub(super) fn detect_event_in_progress(
    contents: &str,
    date: NaiveDate,
    session_name: Option<&str>,
    entry_index: usize,
    start: u16,
) -> Option<EventInProgressJson> {
    detect_event_in_progress_with(
        contents,
        date,
        session_name,
        entry_index,
        start,
        None::<&dyn CloseVault>,
        None,
    )
}

pub(super) fn detect_event_in_progress_with<V: CloseVault + ?Sized>(
    contents: &str,
    date: NaiveDate,
    session_name: Option<&str>,
    entry_index: usize,
    start: u16,
    vault: Option<&V>,
    day_relative: Option<&Path>,
) -> Option<EventInProgressJson> {
    if is_meet_name(session_name) {
        return None;
    }
    let events = events_for_clock(contents, date, vault, day_relative);
    let exclude = lineup_exclude_ids(contents, entry_index);
    let in_progress = events_in_progress_at(start, &events, &exclude);
    let first = in_progress.first().copied()?;
    let span = first.span?;
    let more = u64::try_from(in_progress.len().saturating_sub(1)).unwrap_or(0);
    Some(EventInProgressJson {
        event: EventInProgressEventJson {
            title: first.title.clone(),
            time_range: span.text(),
            line: first.line_index + 1,
            off_clock: first.off_clock,
            buffer: EventBufferJson::new(span, first.buffer),
        },
        more,
    })
}

/// Attach a start-family conflict after the MEET rule has written the
/// final span. Restart and swap reuse the previous session's clock as
/// the old span so already-crossed events are not re-warned. Mid-meeting
/// starts attach `event_in_progress` without that "only new" filter.
#[allow(dead_code)]
pub(super) fn attach_start_event_conflict(
    summary: &mut PomodoroStartSummary,
    contents: &str,
    date: NaiveDate,
    spec: &PomodoroStartSpec,
    typed_name: Option<&str>,
) {
    attach_start_event_conflict_with(
        summary,
        contents,
        date,
        spec,
        typed_name,
        None::<&dyn CloseVault>,
        None,
    );
}

pub(super) fn attach_start_event_conflict_with<V: CloseVault + ?Sized>(
    summary: &mut PomodoroStartSummary,
    contents: &str,
    date: NaiveDate,
    spec: &PomodoroStartSpec,
    typed_name: Option<&str>,
    vault: Option<&V>,
    day_relative: Option<&Path>,
) {
    let Some((new_start, new_end)) = clock_span(&summary.start, &summary.end)
    else {
        return;
    };
    let old = summary.r#override.as_ref().and_then(|item| {
        item.previous
            .as_ref()
            .and_then(|prev| clock_span(&prev.start, &prev.end))
    });
    let name = typed_name
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_string);
    summary.event_conflict = detect_event_conflict_with(
        contents,
        date,
        summary.pomodoro_name.as_deref(),
        summary.pomodoro_line.saturating_sub(1),
        new_start,
        new_end,
        old,
        &FitKind::Start {
            spec: spec.clone(),
            name,
        },
        vault,
        day_relative,
    );
    summary.event_in_progress = detect_event_in_progress_with(
        contents,
        date,
        summary.pomodoro_name.as_deref(),
        summary.pomodoro_line.saturating_sub(1),
        new_start,
        vault,
        day_relative,
    );
}

pub(super) fn event_conflict_message(
    session_name: Option<&str>,
    start: &str,
    end: &str,
    conflict: &EventConflictJson,
) -> String {
    let name = session_name
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .unwrap_or("session");
    let event = &conflict.event;
    let buffer = event.buffer.as_ref();
    // The overlap sits wholly inside one buffer: name that buffer. Any
    // other overlap on a buffered event runs into the meeting, so the
    // event carries its whole buffer label.
    let target = match (buffer, conflict.buffer_part) {
        (Some(buffer), Some(part)) => {
            format!(
                "{}'s {} buffer ({})",
                event.title,
                buffer.part_label(part),
                event.time_range
            )
        }
        _ => event_with_buffer_label(&event.title, &event.time_range, buffer),
    };
    let mut message = format!(
        "⚠ {name} {start}-{end} runs {}m into {target}",
        conflict.overlap_minutes,
    );
    if conflict.more > 0 {
        message.push_str(&format!(" and {} more", conflict.more));
    }
    if let Some(fit) = conflict.fit.as_ref() {
        // A fit draft ends where the event starts occupying time. `--N`
        // keeps the session inside the meeting instead (see
        // [`fit_target`]), so it names the meeting start.
        let occupied = match buffer {
            Some(buffer) if !fit.starts_with("--") => {
                buffer.time_range.as_str()
            }
            _ => event.time_range.as_str(),
        };
        let ends = occupied
            .split_once('-')
            .map(|(start, _)| start)
            .unwrap_or(occupied);
        message.push_str(&format!(" · {fit} ends at {ends}"));
    }
    message
}

pub(super) fn event_conflict_warning(
    session_name: Option<&str>,
    start: &str,
    end: &str,
    conflict: &EventConflictJson,
) -> CaptureWarning {
    CaptureWarning::Structured(StructuredCaptureWarning {
        code: "pomodoro_runs_past_event",
        label: None,
        count: None,
        message: event_conflict_message(session_name, start, end, conflict),
    })
}

pub(super) fn event_in_progress_message(
    session_name: Option<&str>,
    start: &str,
    end: &str,
    in_progress: &EventInProgressJson,
) -> String {
    let name = session_name
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .unwrap_or("session");
    let event = &in_progress.event;
    let target = event_with_buffer_label(
        &event.title,
        &event.time_range,
        event.buffer.as_ref(),
    );
    let mut message = format!("⚠ {name} {start}-{end} starts during {target}");
    if in_progress.more > 0 {
        message.push_str(&format!(" and {} more", in_progress.more));
    }
    message
}

pub(super) fn event_in_progress_warning(
    session_name: Option<&str>,
    start: &str,
    end: &str,
    in_progress: &EventInProgressJson,
) -> CaptureWarning {
    CaptureWarning::Structured(StructuredCaptureWarning {
        code: "pomodoro_starts_during_event",
        label: None,
        count: None,
        message: event_in_progress_message(
            session_name,
            start,
            end,
            in_progress,
        ),
    })
}

/// Harvest the run-past-event warning from a planned item's start,
/// adjust, or shift JSON. One warning of this kind per item, so `= +2`
/// warns once on the start when the adjust does not newly cross anything.
pub(super) fn warning_from_item(
    item: &CaptureItemResult,
) -> Option<CaptureWarning> {
    if let Some(start) = item.pomodoro_start.as_ref()
        && let Some(conflict) = start.event_conflict.as_ref()
    {
        return Some(event_conflict_warning(
            start.pomodoro_name.as_deref(),
            &start.start,
            &start.end,
            conflict,
        ));
    }
    if let Some(adjust) = item.pomodoro_adjust.as_ref()
        && let Some(conflict) = adjust.event_conflict.as_ref()
    {
        return Some(event_conflict_warning(
            adjust.pomodoro_name.as_deref(),
            &adjust.after_start,
            &adjust.after_end,
            conflict,
        ));
    }
    if let Some(shift) = item.pomodoro_shift.as_ref()
        && let Some(conflict) = shift.event_conflict.as_ref()
    {
        return Some(event_conflict_warning(
            shift.pomodoro_name.as_deref(),
            &shift.after_start,
            &shift.after_end,
            conflict,
        ));
    }
    None
}

/// Harvest the mid-meeting start warning. Sibling of [`warning_from_item`]
/// so one item can report both `pomodoro_runs_past_event` and
/// `pomodoro_starts_during_event` when they name different events.
pub(super) fn in_progress_warning_from_item(
    item: &CaptureItemResult,
) -> Option<CaptureWarning> {
    let start = item.pomodoro_start.as_ref()?;
    let in_progress = start.event_in_progress.as_ref()?;
    Some(event_in_progress_warning(
        start.pomodoro_name.as_deref(),
        &start.start,
        &start.end,
        in_progress,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(events: &str, pomodoros: &str) -> String {
        format!("## Events\n{events}## Pomodoros\n{pomodoros}")
    }

    fn date() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 11, 1).unwrap()
    }

    fn empty_start() -> PomodoroStartSpec {
        PomodoroStartSpec {
            raw: String::new(),
            duration_units: 5,
            offset_units: 0,
            later_units: 0,
            drop: Vec::new(),
        }
    }

    #[test]
    fn start_fit_drafts_keep_the_later_sign() {
        let spec = PomodoroStartSpec {
            raw: "+2".to_string(),
            duration_units: 5,
            offset_units: 0,
            later_units: 2,
            drop: Vec::new(),
        };
        assert_eq!(start_fit_token(3, &spec, None), "=3+2");
        assert_eq!(
            start_fit_token(3, &spec, Some("deep-work")),
            "=3+2#deep-work"
        );
        assert_eq!(start_fit_token(2, &spec, Some("fix")), "=fix2+2");
        assert_eq!(start_fit_token(2, &empty_start(), Some("bugs")), "=bugs2");
    }

    fn session_index(note: &str) -> usize {
        note.lines()
            .position(|line| line.contains("—"))
            .expect("session headline")
    }

    #[test]
    fn start_fit_is_largest_count_ending_at_the_event() {
        let contents = note(
            "- [ ] #task **1200-1230** Standup ^standup\n",
            "- [ ] (**1150-1215** [t:: 25m]) — BOB\n",
        );
        let conflict = detect_event_conflict(
            &contents,
            date(),
            Some("BOB"),
            session_index(&contents),
            11 * 60 + 50,
            12 * 60 + 15,
            None,
            &FitKind::Start {
                spec: empty_start(),
                name: None,
            },
        )
        .expect("conflict");
        assert_eq!(conflict.event.title, "Standup");
        assert_eq!(conflict.event.time_range, "1200-1230");
        assert_eq!(conflict.event.line, 2);
        assert!(!conflict.event.untracked);
        assert_eq!(conflict.overlap_minutes, 15);
        assert_eq!(conflict.more, 0);
        assert_eq!(conflict.fit.as_deref(), Some("=2"));
        assert_eq!(
            event_conflict_message(
                Some("BOB"),
                "1150",
                "1215",
                &conflict,
            ),
            "⚠ BOB 1150-1215 runs 15m into Standup (1200-1230) · =2 ends at 1200"
        );
    }

    #[test]
    fn untracked_events_warn_and_named_starts_keep_the_hash() {
        let contents = note(
            "- [ ] #task **1200-1300** Lunch with Kelly #untracked ^lunch\n",
            "- [ ] (**1150-1215** [t:: 25m]) — BUGS\n",
        );
        let spec = PomodoroStartSpec {
            raw: String::new(),
            duration_units: 5,
            offset_units: 0,
            later_units: 0,
            drop: Vec::new(),
        };
        let conflict = detect_event_conflict(
            &contents,
            date(),
            Some("BUGS"),
            session_index(&contents),
            11 * 60 + 50,
            12 * 60 + 15,
            None,
            &FitKind::Start {
                spec,
                name: Some("bugs".to_string()),
            },
        )
        .expect("conflict");
        assert!(conflict.event.off_clock);
        assert_eq!(conflict.fit.as_deref(), Some("=bugs2"));
    }

    #[test]
    fn meet_sessions_and_own_lineup_and_old_span_are_exempt() {
        let contents = note(
            concat!(
                "- [ ] #task **1200-1230** Standup ^standup\n",
                "- [x] #task **1215-1245** Done ^done\n",
                "- [-] #task **1220-1250** Cancelled ^cancelled\n",
            ),
            concat!(
                "- [ ] (**1150-1215** [t:: 25m]) — MEET\n",
                "  - [[#^standup]]\n",
            ),
        );
        assert!(detect_event_conflict(
            &contents,
            date(),
            Some("MEET"),
            session_index(&contents),
            11 * 60 + 50,
            12 * 60 + 15,
            None,
            &FitKind::Start {
                spec: empty_start(),
                name: None,
            },
        )
        .is_none());
        let bob = note(
            "- [ ] #task **1200-1230** Standup ^standup\n",
            concat!(
                "- [ ] (**1150-1215** [t:: 25m]) — BOB\n",
                "  - [[#^standup]]\n",
            ),
        );
        assert!(detect_event_conflict(
            &bob,
            date(),
            Some("BOB"),
            session_index(&bob),
            11 * 60 + 50,
            12 * 60 + 15,
            None,
            &FitKind::Start {
                spec: empty_start(),
                name: None,
            },
        )
        .is_none());
        assert!(detect_event_conflict(
            &contents,
            date(),
            Some("BOB"),
            session_index(&contents),
            11 * 60 + 50,
            12 * 60 + 15,
            Some((11 * 60 + 50, 12 * 60 + 15)),
            &FitKind::Start {
                spec: empty_start(),
                name: None,
            },
        )
        .is_none());
    }

    #[test]
    fn plus_and_shift_fits_and_no_fit_when_the_start_is_the_event() {
        let contents = note(
            "- [ ] #task **1200-1230** Standup ^standup\n",
            "- [ ] (**1150-1155** [t:: 5m]) — BOB\n",
        );
        let plus = detect_event_conflict(
            &contents,
            date(),
            Some("BOB"),
            session_index(&contents),
            11 * 60 + 50,
            12 * 60 + 15,
            Some((11 * 60 + 50, 11 * 60 + 55)),
            &FitKind::AdjustPlus {
                old_end: 11 * 60 + 55,
            },
        )
        .expect("plus");
        assert_eq!(plus.fit.as_deref(), Some("+1"));
        let later_note = note(
            "- [ ] #task **1210-1230** Review ^review\n",
            "- [ ] (**1140-1205** [t:: 25m]) — BOB\n",
        );
        let later = detect_event_conflict(
            &later_note,
            date(),
            Some("BOB"),
            session_index(&later_note),
            11 * 60 + 55,
            12 * 60 + 20,
            Some((11 * 60 + 40, 12 * 60 + 5)),
            &FitKind::ShiftLater {
                old_end: 12 * 60 + 5,
            },
        )
        .expect("later");
        assert_eq!(later.fit.as_deref(), Some("++1"));
        let earlier = detect_event_conflict(
            &contents,
            date(),
            Some("BOB"),
            session_index(&contents),
            11 * 60 + 55,
            12 * 60 + 20,
            Some((12 * 60 + 10, 12 * 60 + 35)),
            &FitKind::ShiftEarlier {
                old_start: 12 * 60 + 10,
            },
        )
        .expect("earlier");
        assert_eq!(earlier.fit.as_deref(), Some("--1"));
        let no_fit = detect_event_conflict(
            &contents,
            date(),
            Some("BOB"),
            session_index(&contents),
            12 * 60,
            12 * 60 + 25,
            None,
            &FitKind::Start {
                spec: empty_start(),
                name: None,
            },
        )
        .expect("no-fit");
        assert!(no_fit.fit.is_none(), "{no_fit:?}");
        assert_eq!(no_fit.overlap_minutes, 25);
    }

    #[test]
    fn more_counts_later_crossings() {
        let contents = note(
            concat!(
                "- [ ] #task **1200-1215** Standup ^standup\n",
                "- [ ] #task **1205-1230** 1:1 ^one-on-one\n",
            ),
            "- [ ] (**1150-1215** [t:: 25m]) — BOB\n",
        );
        let conflict = detect_event_conflict(
            &contents,
            date(),
            Some("BOB"),
            session_index(&contents),
            11 * 60 + 50,
            12 * 60 + 15,
            None,
            &FitKind::Start {
                spec: empty_start(),
                name: None,
            },
        )
        .expect("conflict");
        assert_eq!(conflict.event.title, "Standup");
        assert_eq!(conflict.more, 1);
        assert!(
            event_conflict_message(Some("BOB"), "1150", "1215", &conflict)
                .contains(" and 1 more")
        );
    }

    fn lunch_buffer() -> EventBufferJson {
        EventBufferJson {
            before_minutes: 30,
            after_minutes: 15,
            time_range: "1130-1315".to_string(),
        }
    }

    fn lunch_conflict(
        buffer_part: Option<BufferPart>,
        fit: Option<&str>,
    ) -> EventConflictJson {
        EventConflictJson {
            event: EventConflictEventJson {
                title: "Lunch".to_string(),
                time_range: "1200-1300".to_string(),
                line: 2,
                off_clock: false,
                untracked: false,
                buffer: Some(lunch_buffer()),
            },
            overlap_minutes: 5,
            buffer_part,
            more: 0,
            fit: fit.map(str::to_string),
        }
    }

    #[test]
    fn messages_name_the_buffer_the_overlap_sits_in() {
        assert_eq!(
            event_conflict_message(
                Some("BOB"),
                "1110",
                "1135",
                &lunch_conflict(Some(BufferPart::Before), Some("=4")),
            ),
            "⚠ BOB 1110-1135 runs 5m into Lunch's \u{2212}30m buffer (1200-1300) · =4 ends at 1130"
        );
        assert_eq!(
            event_conflict_message(
                Some("BOB"),
                "1305",
                "1330",
                &lunch_conflict(Some(BufferPart::After), None),
            ),
            "⚠ BOB 1305-1330 runs 5m into Lunch's +15m buffer (1200-1300)"
        );
        assert_eq!(
            event_conflict_message(
                Some("BOB"),
                "1110",
                "1215",
                &lunch_conflict(None, Some("=4")),
            ),
            "⚠ BOB 1110-1215 runs 5m into Lunch (1200-1300 \u{2212}30m +15m) · =4 ends at 1130"
        );
        // `--N` keeps the session inside the meeting, so it names the
        // meeting start, as it does without a buffer.
        assert_eq!(
            event_conflict_message(
                Some("BOB"),
                "1155",
                "1220",
                &lunch_conflict(None, Some("--1")),
            ),
            "⚠ BOB 1155-1220 runs 5m into Lunch (1200-1300 \u{2212}30m +15m) · --1 ends at 1200"
        );
    }

    #[test]
    fn buffered_fit_ends_at_the_buffered_start_and_only_before_it() {
        let contents = note(
            "- [ ] #task **1200-1300** Lunch [buffer:: -30m +15m] ^lunch\n",
            "- [ ] (**1110-1135** [t:: 25m]) — BOB\n",
        );
        let conflict = detect_event_conflict(
            &contents,
            date(),
            Some("BOB"),
            session_index(&contents),
            11 * 60 + 10,
            11 * 60 + 35,
            None,
            &FitKind::Start {
                spec: empty_start(),
                name: None,
            },
        )
        .expect("conflict");
        assert_eq!(conflict.fit.as_deref(), Some("=4"));
        assert_eq!(conflict.buffer_part, Some(BufferPart::Before));
        assert_eq!(conflict.overlap_minutes, 5);
        assert_eq!(conflict.event.buffer, Some(lunch_buffer()));
        // Starting inside the lead-in offers no fit: there is no room
        // before the buffered start.
        let inside = detect_event_conflict(
            &contents,
            date(),
            Some("BOB"),
            session_index(&contents),
            11 * 60 + 40,
            12 * 60 + 5,
            None,
            &FitKind::Start {
                spec: empty_start(),
                name: None,
            },
        )
        .expect("inside");
        assert!(inside.fit.is_none(), "{inside:?}");
        assert!(inside.buffer_part.is_none());
    }

    #[test]
    fn unbuffered_payloads_omit_the_buffer_keys() {
        let contents = note(
            "- [ ] #task **1200-1230** Standup ^standup\n",
            "- [ ] (**1150-1215** [t:: 25m]) — BOB\n",
        );
        let conflict = detect_event_conflict(
            &contents,
            date(),
            Some("BOB"),
            session_index(&contents),
            11 * 60 + 50,
            12 * 60 + 15,
            None,
            &FitKind::Start {
                spec: empty_start(),
                name: None,
            },
        )
        .expect("conflict");
        let json = serde_json::to_value(&conflict).unwrap();
        assert!(json.get("buffer_part").is_none(), "{json}");
        assert!(json["event"].get("buffer").is_none(), "{json}");
    }

    #[test]
    fn mid_meeting_start_reports_event_in_progress_without_untracked() {
        let contents = note(
            concat!(
                "- [ ] #task **0800-0815** Swings! #offclock ^swings\n",
                "- [ ] #task **0700-0900** Overlap ^overlap\n",
            ),
            "- [ ] (**0805-0830** [t:: 25m]) — BUILD\n",
        );
        let in_progress = detect_event_in_progress(
            &contents,
            date(),
            Some("BUILD"),
            session_index(&contents),
            8 * 60 + 5,
        )
        .expect("in progress");
        assert_eq!(in_progress.event.title, "Overlap");
        assert_eq!(in_progress.event.time_range, "0700-0900");
        assert!(!in_progress.event.off_clock);
        assert_eq!(in_progress.more, 1);
        let json = serde_json::to_value(&in_progress).unwrap();
        assert!(json["event"].get("untracked").is_none(), "{json}");
        assert_eq!(
            event_in_progress_message(
                Some("BUILD"),
                "0805",
                "0830",
                &in_progress,
            ),
            "⚠ BUILD 0805-0830 starts during Overlap (0700-0900) and 1 more"
        );
        assert!(detect_event_in_progress(
            &contents,
            date(),
            Some("MEET"),
            session_index(&contents),
            8 * 60 + 5,
        )
        .is_none());
        let own = note(
            "- [ ] #task **0800-0815** Swings! ^swings\n",
            concat!(
                "- [ ] (**0805-0830** [t:: 25m]) — BUILD\n",
                "  - [[#^swings]]\n",
            ),
        );
        assert!(detect_event_in_progress(
            &own,
            date(),
            Some("BUILD"),
            session_index(&own),
            8 * 60 + 5,
        )
        .is_none());
    }
}
