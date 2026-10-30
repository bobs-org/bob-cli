//! One bounded, complete local-day calendar snapshot, shared by
//! `bob gcal list` and `bob gcal sync`.
//!
//! Fetch one calendar per gog call, all pages, with the field mask that
//! requests only `attendees(self,responseStatus)`. Any failure means the
//! snapshot is incomplete: callers must write nothing and exit 1.

use std::path::PathBuf;

use chrono::{DateTime, Datelike, FixedOffset, NaiveDate};
use serde_json::json;

use super::{
    calendars::resolve_account,
    classify::{self, DayWindow, Inclusion},
    config::GcalConfig,
    gog::Gog,
    model::{EventDateTime, GcalEvent},
    root::{self, EventRoot},
    GcalError,
};
use crate::native::env as bob_env;

/// One classified event with its valid root and display fields.
pub(crate) struct SnapshotEvent {
    pub event: GcalEvent,
    pub inclusion: Inclusion,
    pub root: Option<EventRoot>,
    #[allow(dead_code)]
    pub calendar_name: String,
    pub time: String,
    pub sort_at: DateTime<FixedOffset>,
}

/// A complete snapshot for one local day.
pub(crate) struct DaySnapshot {
    pub day: NaiveDate,
    pub window: DayWindow,
    pub vault: PathBuf,
    pub daily_rel: PathBuf,
    pub events: Vec<SnapshotEvent>,
}

/// Fetch, classify, and root every event for `day`. A failed calendar
/// fails the whole snapshot.
pub(crate) fn fetch_day(
    vault: PathBuf,
    config: &GcalConfig,
    day: NaiveDate,
) -> Result<DaySnapshot, GcalError> {
    let timezone = bob_env::configured_timezone();
    let window = DayWindow::new(day, timezone)?;
    let daily_rel = daily_relative_path(day);
    let gog = Gog::resolve(config.timeout())?;
    let _ = gog.require_version()?;
    let account = resolve_account(&gog, config)?;
    let calendars = gog.calendars(&account)?;

    let mut events = Vec::new();
    for configured in config.calendars() {
        let calendar = calendars
            .iter()
            .find(|calendar| calendar.matches_configured_id(&configured.id));
        let calendar_name = calendar
            .map(|calendar| calendar.name().to_string())
            .unwrap_or_else(|| configured.id.clone());
        let raw_events = gog.events(
            &configured.id,
            &window.from_rfc3339(),
            &window.to_rfc3339(),
        )?;
        for raw_event in raw_events {
            let event = GcalEvent::from_google(&configured.id, raw_event)?;
            let inclusion = classify::classify(&event, &window, config);
            let root = if inclusion == Inclusion::Task {
                Some(root::choose_event_root(
                    &vault,
                    &daily_rel,
                    &event,
                    &calendar_name,
                    config,
                ))
            } else {
                None
            };
            let title = root
                .as_ref()
                .map(|chosen| chosen.title.clone())
                .unwrap_or_else(|| event.title.clone());
            let time = event_time(&event, &window);
            let sort_at = window.effective_start(&event);
            let mut event = event;
            event.title = title;
            events.push(SnapshotEvent {
                event,
                inclusion,
                root,
                calendar_name: calendar_name.clone(),
                time,
                sort_at,
            });
        }
    }
    events.sort_by(compare_events);
    Ok(DaySnapshot {
        day,
        window,
        vault,
        daily_rel,
        events,
    })
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
