//! Calendar-aware MEET starts: the event run a MEET session follows and the
//! trackedness rule every start-family write applies.
//!
//! A MEET session's lineup holds same-note links to Event Tasks. The **event
//! run** is the earliest open Event Task that has not ended plus every
//! following one that touches or overlaps it (`next.start <= run.end`, the
//! `cluster_by_trackedness` rule) and shares its trackedness. An empty-suffix
//! start writes the run's real span; every MEET start (counted, kept-ledger,
//! link) omits `[t::]` exactly when the run is non-empty and untracked. Every
//! other session is tracked. The run is computed on the staged daily note, so
//! drops (`~K`) and earlier items of the same draft are already applied.

use std::collections::HashSet;

use super::*;
use crate::native::event_task::{event_task_by_id, EventTask};

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

/// The computed run, in minutes on the day's timeline (`end_min` may pass
/// 1440 for a midnight-crossing event).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct EventRun {
    pub(super) start_min: i32,
    pub(super) end_min: i32,
    pub(super) untracked: bool,
    pub(super) events: Vec<RunEvent>,
}

/// Event run of the session headlined at `entry_index` in `contents`.
///
/// `numbers` are the pre-drop lineup numbers of the surviving links in
/// ledger order (a drop leaves gaps); `None` numbers the links 1..n.
pub(super) fn compute_event_run(
    day_relative: &Path,
    contents: &str,
    entry_index: usize,
    numbers: Option<&[u32]>,
    now_min: i32,
) -> Option<EventRun> {
    let links =
        capture_pomodoro_start::list_queued_links(contents, entry_index);
    let mut seen = HashSet::new();
    let mut candidates: Vec<(usize, RunEvent)> = Vec::new();
    for (position, link) in links.iter().enumerate() {
        if link.embedded || !link.path_part.is_empty() {
            continue;
        }
        let Some(event) =
            event_task_by_id(day_relative, contents, &link.block_id)
        else {
            continue;
        };
        let start = i32::from(event.span.start_min);
        let end = event.span.end_on_timeline();
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
        (
            item.event.span.start_min,
            item.event.span.end_on_timeline(),
            *position,
        )
    });
    let mut ordered = candidates.into_iter().map(|(_, item)| item);
    let first = ordered.next()?;
    let untracked = first.event.untracked;
    let start_min = i32::from(first.event.span.start_min);
    let mut end_min = first.event.span.end_on_timeline();
    let mut events = vec![first];
    for next in ordered {
        if i32::from(next.event.span.start_min) > end_min
            || next.event.untracked != untracked
        {
            break;
        }
        end_min = end_min.max(next.event.span.end_on_timeline());
        events.push(next);
    }
    Some(EventRun {
        start_min,
        end_min,
        untracked,
        events,
    })
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
    pub(super) untracked: bool,
}

/// `pomodoro_start.event_run`: the run a MEET start followed or overrode.
///
/// `time_range`/`start`/`end` are the run's span as the calendar gives it
/// (after the late-join clamp), plain `HHMM-HHMM`; `source` says whether
/// the written session follows it (`calendar`) or a counted suffix or kept
/// ledger set its own span (`explicit`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct EventRunJson {
    pub(super) time_range: String,
    pub(super) start: String,
    pub(super) end: String,
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
            untracked: run.untracked,
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
                    untracked: item.event.untracked,
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
    /// trackedness follows the run.
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
        summary.event_run = self.event_run;
        self.warnings
    }
}

/// [`finalize_meet_start`] that hands back the final contents, taking them
/// out of the finish so the caller keeps one owner.
pub(super) fn apply_meet_rule(
    contents: String,
    input: &MeetStartInput<'_>,
) -> Result<(String, Option<MeetFinish>), CaptureError> {
    match finalize_meet_start(&contents, input)? {
        Some(mut finish) => {
            let updated = std::mem::take(&mut finish.contents);
            Ok((updated, Some(finish)))
        }
        None => Ok((contents, None)),
    }
}

/// Apply the MEET rule to the session just started at `input.entry_index`.
///
/// Returns `Ok(None)` when the started session needs nothing: it is not a
/// MEET, or it is a tracked MEET without a run, and its ledger is tracked.
pub(super) fn finalize_meet_start(
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
    let ledger_tracked =
        !matches!(session_tracking(headline), SessionTracking::Untracked);
    let meet = is_meet_name(entry.name.as_deref());
    if !meet && ledger_tracked {
        return Ok(None);
    }
    let now_min = i32::try_from(input.now.hour() * 60 + input.now.minute())
        .unwrap_or_default();
    let run = if meet {
        compute_event_run(
            input.day_relative,
            contents,
            input.entry_index,
            input.numbers,
            now_min,
        )
    } else {
        None
    };
    let Some(run) = run else {
        if ledger_tracked {
            return Ok(None);
        }
        // A kept ledger from an untracked session lands on a session with
        // no untracked run: restore the clock.
        let ledger = read_session_ledger_bytes(headline).ok_or_else(|| {
            CaptureError::io(
                "running Pomodoro timing could not be safely identified",
            )
        })?;
        let restored = set_ledger_tracking(&ledger, true);
        let updated =
            splice_session_ledger(contents, input.entry_index, &restored)?;
        return Ok(Some(MeetFinish {
            contents: updated,
            time_range: Some(restored),
            start: None,
            end: None,
            duration_minutes: None,
            offset_units: None,
            tracked: true,
            event_run: None,
            warnings: Vec::new(),
        }));
    };

    let tracked = !run.untracked;
    let calendar =
        input.span_rule == SpanRule::Calendar && input.spec.raw.is_empty();
    let closed_end = latest_closed_end(contents);
    let clamped_start = match closed_end {
        Some(end) if end > run.start_min && end < run.end_min => end,
        _ => run.start_min,
    };
    let source = if calendar { "calendar" } else { "explicit" };
    let event_run = Some(EventRunJson::new(&run, clamped_start, source));

    let mut warnings = Vec::new();
    if calendar {
        let duration =
            u64::try_from(run.end_min - clamped_start).unwrap_or_default();
        let range = format!("{}-{}", hhmm(clamped_start), hhmm(run.end_min));
        let time_range = if tracked {
            format!("(**{range}** [t:: {duration}m])")
        } else {
            format!("(**{range}**)")
        };
        let updated =
            splice_session_ledger(contents, input.entry_index, &time_range)?;
        if clamped_start - now_min > FAR_RUN_MINUTES {
            let title = run
                .events
                .first()
                .map(|item| item.event.title.as_str())
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
            event_run,
            warnings,
        }));
    }

    if ledger_tracked == tracked {
        return Ok(Some(MeetFinish {
            contents: contents.to_string(),
            time_range: None,
            start: None,
            end: None,
            duration_minutes: None,
            offset_units: None,
            tracked,
            event_run,
            warnings,
        }));
    }
    let ledger = read_session_ledger_bytes(headline).ok_or_else(|| {
        CaptureError::io(
            "running Pomodoro timing could not be safely identified",
        )
    })?;
    let retracked = set_ledger_tracking(&ledger, tracked);
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
        event_run,
        warnings,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn run_extends_over_touching_events_of_the_same_trackedness() {
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
        assert!(!run.untracked);
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
        assert!(run.untracked);
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
        let finish = finalize_meet_start(
            &note,
            &MeetStartInput {
                day_relative: rel(),
                entry_index: meet_index(&note),
                spec: &spec,
                span_rule: SpanRule::Calendar,
                numbers: None,
                now,
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
}
