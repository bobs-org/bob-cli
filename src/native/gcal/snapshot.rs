//! One bounded, complete local-day calendar snapshot, shared by
//! `bob gcal list` and `bob gcal sync`.
//!
//! Fetch one calendar per gog call, all pages, with the field mask that
//! requests only `attendees(self,responseStatus)`. Any failure means the
//! snapshot is incomplete: callers must write nothing and exit 1.

use std::{collections::BTreeMap, path::PathBuf};

use chrono::{DateTime, Datelike, FixedOffset, NaiveDate};
use chrono_tz::Tz;
use serde_json::json;

use super::{
    account::resolve_command,
    classify::{self, DayWindow, Inclusion},
    clock,
    config::GcalConfig,
    gog::{Gog, GogCalendar},
    journal::{self, FoldedNote},
    model::{EventDateTime, GcalEvent, ParsedNote},
    root::{self, EventRoot, NoteRootCandidate},
    GcalError,
};
use crate::native::{
    env as bob_env,
    pomodoro_homes::{ClockResolution, HomeResolver, HomeSource},
};

/// One classified event with its valid root and display fields.
pub(crate) struct SnapshotEvent {
    pub event: GcalEvent,
    pub inclusion: Inclusion,
    pub root: Option<EventRoot>,
    #[allow(dead_code)]
    pub calendar_name: String,
    pub time: String,
    pub sort_at: DateTime<FixedOffset>,
    /// Effective clock from Google's explicit tag plus the chosen root.
    pub clock: ClockResolution,
    /// Untagged chosen-root walk, for a later phase's drop advice.
    #[allow(dead_code)]
    pub root_only_clock: ClockResolution,
}

/// A complete snapshot for one local day.
pub(crate) struct DaySnapshot {
    pub day: NaiveDate,
    pub window: DayWindow,
    pub vault: PathBuf,
    pub daily_rel: PathBuf,
    pub events: Vec<SnapshotEvent>,
    /// Stable gog account for this snapshot and later absent-event lookups.
    pub account: String,
    /// Vault-learned series roots, for root source 1 and to keep the
    /// planner from re-learning what the journal already teaches.
    pub series_roots: BTreeMap<(String, String), journal::LearnedRoot>,
}

/// Google transport and account setup shared by every day in a sync run.
pub(crate) struct FetchContext<'a> {
    config: &'a GcalConfig,
    gog: Gog,
    account: String,
    calendars: Vec<GogCalendar>,
    timezone: Option<Tz>,
}

impl<'a> FetchContext<'a> {
    pub(crate) fn new(config: &'a GcalConfig) -> Result<Self, GcalError> {
        let gog = Gog::resolve(
            crate::native::google::GCAL,
            config.gog(),
            config.timeout(),
        )?;
        let _ = gog.require_version()?;
        let account = resolve_command(&gog, config.account())?;
        let calendars = gog.calendars(&account)?;
        Ok(Self {
            config,
            gog,
            account,
            calendars,
            timezone: bob_env::configured_timezone(),
        })
    }

    /// Fetch Google events once for each configured calendar and contiguous
    /// run of days, then partition the raw events by the day's query window.
    pub(crate) fn fetch_days(
        &self,
        days: &[NaiveDate],
    ) -> Result<BTreeMap<NaiveDate, Vec<(String, serde_json::Value)>>, GcalError>
    {
        let mut by_day: BTreeMap<_, Vec<_>> =
            days.iter().copied().map(|day| (day, Vec::new())).collect();
        let windows: BTreeMap<_, _> = days
            .iter()
            .copied()
            .map(|day| Ok((day, DayWindow::new(day, self.timezone)?)))
            .collect::<Result<_, GcalError>>()?;

        for run in contiguous_runs(days) {
            let first = windows.get(&run[0]).expect("run day has window");
            let last = windows
                .get(run.last().expect("non-empty day run"))
                .expect("run day has window");
            for configured in self.config.calendars() {
                let raw_events = self.gog.events(
                    &configured.id,
                    &first.from_rfc3339(),
                    &last.to_rfc3339(),
                    &self.account,
                )?;
                for raw_event in raw_events {
                    for day in &run {
                        let window =
                            windows.get(day).expect("run day has window");
                        if raw_overlaps_day(&raw_event, window) {
                            by_day
                                .get_mut(day)
                                .expect("requested day is present")
                                .push((
                                    configured.id.clone(),
                                    raw_event.clone(),
                                ));
                        }
                    }
                }
            }
        }
        Ok(by_day)
    }

    pub(crate) fn assemble_day<S: HomeSource>(
        &self,
        vault: PathBuf,
        day: NaiveDate,
        raw_events: Vec<(String, serde_json::Value)>,
        resolver: &mut HomeResolver<S>,
    ) -> Result<DaySnapshot, GcalError> {
        assemble_day(self, vault, day, raw_events, resolver)
    }
}

/// Fetch, classify, and root every event for `day`. A failed calendar
/// fails the whole snapshot.
pub(crate) fn fetch_day<S: HomeSource>(
    vault: PathBuf,
    config: &GcalConfig,
    day: NaiveDate,
    resolver: &mut HomeResolver<S>,
) -> Result<DaySnapshot, GcalError> {
    let context = FetchContext::new(config)?;
    let mut days = context.fetch_days(&[day])?;
    context.assemble_day(
        vault,
        day,
        days.remove(&day).unwrap_or_default(),
        resolver,
    )
}

fn assemble_day<S: HomeSource>(
    context: &FetchContext<'_>,
    vault: PathBuf,
    day: NaiveDate,
    raw_events: Vec<(String, serde_json::Value)>,
    resolver: &mut HomeResolver<S>,
) -> Result<DaySnapshot, GcalError> {
    let config = context.config;
    let account = &context.account;
    let window = DayWindow::new(day, context.timezone)?;
    let daily_rel = daily_relative_path(day);
    // Series roots and notes are vault-local memory. An unreadable
    // journal degrades to none here; sync still fails loudly on its
    // own strict journal read.
    let series_roots = journal::load_series_roots(&vault).unwrap_or_default();
    let notes = journal::load_notes(&vault)
        .map(|(book, _)| book)
        .unwrap_or_default();

    let mut events = Vec::new();
    for (calendar_id, raw_event) in raw_events {
        let calendar = context
            .calendars
            .iter()
            .find(|calendar| calendar.matches_configured_id(&calendar_id));
        let calendar_name = calendar
            .map(|calendar| calendar.name().to_string())
            .unwrap_or_else(|| calendar_id.clone());
        let mut event = GcalEvent::from_google(&calendar_id, raw_event)?;
        let occurrence_note = matching_note(
            notes.event(account, &event.key.calendar_id, &event.key.event_id),
            account,
        );
        let series_note = event.series_id.as_ref().and_then(|series| {
            matching_note(
                notes.series(account, &event.key.calendar_id, series),
                account,
            )
        });
        event.apply_notes(occurrence_note.as_ref(), series_note.as_ref());
        let inclusion = classify::classify(&event, &window, config);
        let root = if inclusion == Inclusion::Task {
            let learned = event.series_id.as_ref().and_then(|series| {
                series_roots
                    .get(&(event.key.calendar_id.clone(), series.clone()))
            });
            let note_roots =
                event.note.as_ref().map(|note| NoteRootCandidate {
                    event: note.event_route.clone(),
                    series: note.series_route.clone(),
                });
            Some(root::choose_event_root(
                resolver,
                &vault,
                &daily_rel,
                &event,
                &calendar_name,
                config,
                learned,
                note_roots.as_ref(),
            ))
        } else {
            None
        };
        event.finalize_note_fields(
            root.as_ref().is_some_and(root::EventRoot::is_from_note),
            inclusion == Inclusion::Task && config.description_bullets(),
        );
        let title = root
            .as_ref()
            .map(|chosen| chosen.title.clone())
            .unwrap_or_else(|| event.title.clone());
        let time = event_time(&event, &window);
        let sort_at = window.effective_start(&event);
        let chosen_root =
            root.as_ref().and_then(|chosen| chosen.value.as_deref());
        let (clock, root_only_clock) = clock::resolve_listed_clock(
            resolver,
            &daily_rel,
            chosen_root,
            &event.clock_tag,
            inclusion == Inclusion::Task,
        );
        event.title = title;
        events.push(SnapshotEvent {
            event,
            inclusion,
            root,
            calendar_name,
            time,
            sort_at,
            clock,
            root_only_clock,
        });
    }
    events.sort_by(compare_events);
    Ok(DaySnapshot {
        day,
        window,
        vault,
        daily_rel,
        events,
        account: account.clone(),
        series_roots,
    })
}

fn contiguous_runs(days: &[NaiveDate]) -> Vec<Vec<NaiveDate>> {
    let mut runs = Vec::<Vec<NaiveDate>>::new();
    for day in days.iter().copied() {
        match runs.last_mut() {
            Some(run)
                if run.last().and_then(|last| last.succ_opt()) == Some(day) =>
            {
                run.push(day);
            }
            _ => runs.push(vec![day]),
        }
    }
    runs
}

/// Match Google's half-open `timeMin`/`timeMax` event overlap rule.
/// Unknown or malformed time shapes are kept so the normal event parser can
/// fail the complete snapshot instead of silently dropping an event.
fn raw_overlaps_day(value: &serde_json::Value, window: &DayWindow) -> bool {
    let Some(raw_start) = value.get("start") else {
        return true;
    };
    let Some(raw_end) = value.get("end") else {
        return true;
    };
    if let (Some(start), Some(end)) = (
        raw_start.get("date").and_then(serde_json::Value::as_str),
        raw_end.get("date").and_then(serde_json::Value::as_str),
    ) {
        return match (
            NaiveDate::parse_from_str(start, "%Y-%m-%d"),
            NaiveDate::parse_from_str(end, "%Y-%m-%d"),
        ) {
            (Ok(start), Ok(end)) => start <= window.day && window.day < end,
            _ => true,
        };
    }
    if let (Some(start), Some(end)) = (
        raw_start
            .get("dateTime")
            .and_then(serde_json::Value::as_str),
        raw_end.get("dateTime").and_then(serde_json::Value::as_str),
    ) {
        let start_tz = raw_start
            .get("timeZone")
            .and_then(serde_json::Value::as_str);
        let end_tz =
            raw_end.get("timeZone").and_then(serde_json::Value::as_str);
        return match (
            super::model::parse_instant(start, start_tz),
            super::model::parse_instant(end, end_tz),
        ) {
            (Some(start), Some(end)) => end > window.from && start < window.to,
            _ => true,
        };
    }
    true
}

pub(crate) fn daily_relative_path(day: NaiveDate) -> PathBuf {
    PathBuf::from(format!(
        "{}/{:04}{:02}{:02}.md",
        day.year(),
        day.year(),
        day.month(),
        day.day()
    ))
}

fn event_time(event: &GcalEvent, window: &DayWindow) -> String {
    match (&event.start, &event.end) {
        (EventDateTime::AllDay(_), EventDateTime::AllDay(_)) => {
            "all day".to_string()
        }
        (EventDateTime::Timed(start), EventDateTime::Timed(end)) => {
            format!("{}-{}", window.format_time(start), window.format_time(end))
        }
        _ => "all day".to_string(),
    }
}

fn matching_note(
    record: Option<&FoldedNote>,
    account: &str,
) -> Option<ParsedNote> {
    let record = record?;
    if record.account != account || record.cleared {
        return None;
    }
    Some(ParsedNote::parse(record.scope, &record.text))
}

fn compare_events(
    left: &SnapshotEvent,
    right: &SnapshotEvent,
) -> std::cmp::Ordering {
    let left_all_day = matches!(left.event.start, EventDateTime::AllDay(_));
    let right_all_day = matches!(right.event.start, EventDateTime::AllDay(_));
    left_all_day
        .cmp(&right_all_day)
        .reverse()
        .then_with(|| left.sort_at.cmp(&right.sort_at))
        .then_with(|| {
            left.event.key.calendar_id.cmp(&right.event.key.calendar_id)
        })
        .then_with(|| left.event.key.event_id.cmp(&right.event.key.event_id))
}

#[allow(dead_code)]
pub(crate) fn snapshot_counts_json(
    events: &[SnapshotEvent],
) -> serde_json::Value {
    let mut tasks = 0;
    let mut context = 0;
    let mut omitted = 0;
    let mut awaiting_reply = 0;
    for event in events {
        match &event.inclusion {
            Inclusion::Task => tasks += 1,
            Inclusion::Context(_) => context += 1,
            Inclusion::Omitted(_) => omitted += 1,
        }
        if event.inclusion.reason() == Some("reply in Calendar") {
            awaiting_reply += 1;
        }
    }
    json!({
        "tasks": tasks,
        "context": context,
        "omitted": omitted,
        "awaiting_reply": awaiting_reply,
    })
}

#[cfg(test)]
mod tests {
    use chrono::NaiveDate;
    use chrono_tz::Tz;
    use serde_json::json;

    use super::{raw_overlaps_day, DayWindow};

    fn window(day: &str) -> DayWindow {
        DayWindow::new(
            NaiveDate::parse_from_str(day, "%Y-%m-%d").expect("date"),
            Some("America/New_York".parse::<Tz>().expect("timezone")),
        )
        .expect("day window")
    }

    #[test]
    fn timed_window_overlap_uses_strict_boundaries() {
        let day = window("2026-10-31");
        let ending_at_start = json!({
            "start": {"dateTime": "2026-10-30T23:00:00-04:00"},
            "end": {"dateTime": "2026-10-31T00:00:00-04:00"},
        });
        let starting_at_end = json!({
            "start": {"dateTime": "2026-11-01T00:00:00-04:00"},
            "end": {"dateTime": "2026-11-01T01:00:00-04:00"},
        });
        let spanning_midnight = json!({
            "start": {"dateTime": "2026-10-31T23:00:00-04:00"},
            "end": {"dateTime": "2026-11-01T01:00:00-04:00"},
        });
        assert!(!raw_overlaps_day(&ending_at_start, &day));
        assert!(!raw_overlaps_day(&starting_at_end, &day));
        assert!(raw_overlaps_day(&spanning_midnight, &day));
        assert!(raw_overlaps_day(&spanning_midnight, &window("2026-11-01")));
    }

    #[test]
    fn all_day_window_uses_an_exclusive_end_date() {
        let event = json!({
            "start": {"date": "2026-10-30"},
            "end": {"date": "2026-10-31"},
        });
        assert!(raw_overlaps_day(&event, &window("2026-10-30")));
        assert!(!raw_overlaps_day(&event, &window("2026-10-31")));
    }
}
