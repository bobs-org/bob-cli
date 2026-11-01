//! Run-past-event warnings for non-MEET starts, extends, and shifts.
//!
//! After an operator writes a new session span, open Event Tasks whose
//! start is in the new `[S, E)` but not the previous one raise one
//! structured `pomodoro_runs_past_event` warning naming the earliest
//! crossing, plus an additive `event_conflict` object on the start,
//! adjust, or shift JSON. MEET sessions and events already in the
//! session's own lineup are exempt. `-N` and closes never warn.
//! Untracked Event Tasks warn like any other open Event Task.

use std::collections::HashSet;

use super::*;
use crate::native::event_task::{
    event_starts_crossed, events_in_note, EventNoteRow, EventSpan,
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct EventConflictEventJson {
    pub(super) title: String,
    pub(super) time_range: String,
    pub(super) line: usize,
    pub(super) untracked: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct EventConflictJson {
    pub(super) event: EventConflictEventJson,
    pub(super) overlap_minutes: u64,
    #[serde(skip_serializing_if = "is_zero_more")]
    pub(super) more: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) fit: Option<String>,
}

fn is_zero_more(value: &u64) -> bool {
    *value == 0
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

fn overlap_minutes(
    session_start: u16,
    session_end: u16,
    event: EventSpan,
) -> u64 {
    let start = i32::from(session_start);
    let mut end = i32::from(session_end);
    if end <= start {
        end += 24 * 60;
    }
    let mut event_start = i32::from(event.start_min);
    if event_start < start {
        event_start += 24 * 60;
    }
    let mut event_end = event.end_on_timeline();
    if event_end <= event_start {
        event_end += 24 * 60;
    }
    let overlap_start = event_start.max(start);
    let overlap_end = event_end.min(end);
    u64::try_from((overlap_end - overlap_start).max(0)).unwrap_or(0)
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
    let duration = if spec.offset_units == 0 {
        units.to_string()
    } else {
        format!("{units}-{}", spec.offset_units)
    };
    match name.map(str::trim).filter(|name| !name.is_empty()) {
        Some(name) => format!("={duration}#{name}"),
        None => format!("={duration}"),
    }
}

fn lineup_exclude_ids(contents: &str, entry_index: usize) -> HashSet<String> {
    capture_pomodoro_start::list_queued_links(contents, entry_index)
        .into_iter()
        .map(|link| link.block_id)
        .collect()
}

fn newly_crossed_from<'a>(
    events: &'a [EventNoteRow],
    entry_index: usize,
    contents: &str,
    new_start: u16,
    new_end: u16,
    old: Option<(u16, u16)>,
) -> Vec<&'a EventNoteRow> {
    let exclude = lineup_exclude_ids(contents, entry_index);
    event_starts_crossed(new_start, new_end, events, &exclude)
        .into_iter()
        .filter(|event| {
            let Some(span) = event.span else {
                return false;
            };
            match old {
                Some((old_start, old_end)) => {
                    !start_in_old(span.start_min, old_start, old_end)
                }
                None => true,
            }
        })
        .collect()
}

fn start_in_old(event_start: u16, old_start: u16, old_end: u16) -> bool {
    if old_start == old_end {
        return false;
    }
    if old_end > old_start {
        event_start >= old_start && event_start < old_end
    } else {
        event_start >= old_start || event_start < old_end
    }
}

/// Conflict for a non-MEET session whose new span newly crosses an
/// open Event Task. `None` when the session is MEET or nothing new
/// is crossed.
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
    if is_meet_name(session_name) {
        return None;
    }
    let events = events_in_note(contents, date);
    let crossed = newly_crossed_from(
        &events,
        entry_index,
        contents,
        new_start,
        new_end,
        old,
    );
    let first = *crossed.first()?;
    let span = first.span?;
    let more = u64::try_from(crossed.len().saturating_sub(1)).unwrap_or(0);
    Some(EventConflictJson {
        event: EventConflictEventJson {
            title: first.title.clone(),
            time_range: span.text(),
            line: first.line_index + 1,
            untracked: first.untracked,
        },
        overlap_minutes: overlap_minutes(new_start, new_end, span),
        more,
        fit: fit_token(fit, new_start, span.start_min),
    })
}

/// Attach a start-family conflict after the MEET rule has written the
/// final span. Restart and swap reuse the previous session's clock as
/// the old span so already-crossed events are not re-warned.
pub(super) fn attach_start_event_conflict(
    summary: &mut PomodoroStartSummary,
    contents: &str,
    date: NaiveDate,
    spec: &PomodoroStartSpec,
    typed_name: Option<&str>,
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
    summary.event_conflict = detect_event_conflict(
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
    let mut message = format!(
        "⚠ {name} {start}-{end} runs {}m into {} ({})",
        conflict.overlap_minutes,
        conflict.event.title,
        conflict.event.time_range,
    );
    if conflict.more > 0 {
        message.push_str(&format!(" and {} more", conflict.more));
    }
    if let Some(fit) = conflict.fit.as_ref() {
        let ends = conflict
            .event
            .time_range
            .split_once('-')
            .map(|(start, _)| start)
            .unwrap_or(conflict.event.time_range.as_str());
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

/// Harvest the structured warning from a planned item's start, adjust,
/// or shift JSON. One warning per item, so `= +2` warns once on the
/// start when the adjust does not newly cross anything.
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
            drop: Vec::new(),
        }
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
        assert!(conflict.event.untracked);
        assert_eq!(conflict.fit.as_deref(), Some("=2#bugs"));
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
}
