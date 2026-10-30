//! DST-safe local day windows and the calendar-event inclusion rule.

use chrono::{
    DateTime, Duration, FixedOffset, Local, NaiveDate, NaiveDateTime, TimeZone,
};
use chrono_tz::Tz;

use super::{
    config::GcalConfig,
    model::{EventDateTime, GcalEvent, MyAnswer},
    GcalError,
};

#[derive(Debug, Clone)]
pub(crate) struct DayWindow {
    pub day: NaiveDate,
    pub from: DateTime<FixedOffset>,
    pub to: DateTime<FixedOffset>,
    timezone: Option<Tz>,
}

impl DayWindow {
    pub(crate) fn new(
        day: NaiveDate,
        timezone: Option<Tz>,
    ) -> Result<Self, GcalError> {
        let next = day.succ_opt().ok_or_else(|| {
            GcalError::setup("bad_day", format!("date {day} is out of range"))
        })?;
        let from = local_midnight(day, timezone).ok_or_else(|| {
            GcalError::setup(
                "bad_day",
                format!("cannot resolve local midnight for {day}"),
            )
        })?;
        let to = local_midnight(next, timezone).ok_or_else(|| {
            GcalError::setup(
                "bad_day",
                format!("cannot resolve local midnight for {next}"),
            )
        })?;
        Ok(Self {
            day,
            from,
            to,
            timezone,
        })
    }

    pub(crate) fn from_rfc3339(&self) -> String {
        self.from.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
    }

    pub(crate) fn to_rfc3339(&self) -> String {
        self.to.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
    }

    pub(crate) fn local(&self, value: &DateTime<FixedOffset>) -> NaiveDateTime {
        match self.timezone {
            Some(timezone) => value.with_timezone(&timezone).naive_local(),
            None => value.with_timezone(&Local).naive_local(),
        }
    }

    pub(crate) fn format_time(&self, value: &DateTime<FixedOffset>) -> String {
        self.local(value).format("%H:%M").to_string()
    }

    pub(crate) fn effective_start(
        &self,
        event: &GcalEvent,
    ) -> DateTime<FixedOffset> {
        match &event.start {
            EventDateTime::Timed(start) if *start < self.from => self.from,
            EventDateTime::Timed(start) => *start,
            EventDateTime::AllDay(date) => {
                local_midnight(*date, self.timezone).unwrap_or(self.from)
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Inclusion {
    Task,
    Context(String),
    Omitted(String),
}

impl Inclusion {
    pub(crate) fn name(&self) -> &'static str {
        match self {
            Self::Task => "task",
            Self::Context(_) => "context",
            Self::Omitted(_) => "omitted",
        }
    }

    pub(crate) fn reason(&self) -> Option<&str> {
        match self {
            Self::Task => None,
            Self::Context(reason) | Self::Omitted(reason) => Some(reason),
        }
    }
}

/// Resolve `today`, `tomorrow`, `yesterday`, or an explicit local date.
pub(crate) fn resolve_day(
    value: &str,
    today: NaiveDate,
) -> Result<NaiveDate, GcalError> {
    match value.trim().to_ascii_lowercase().as_str() {
        "today" => Ok(today),
        "tomorrow" => today.succ_opt().ok_or_else(|| {
            GcalError::setup("bad_day", "tomorrow is out of range".to_string())
        }),
        "yesterday" => today.pred_opt().ok_or_else(|| {
            GcalError::setup("bad_day", "yesterday is out of range".to_string())
        }),
        value => NaiveDate::parse_from_str(value, "%Y-%m-%d").map_err(|_| {
            GcalError::setup(
                "bad_day",
                format!("invalid day {value:?}; use today, tomorrow, yesterday, or YYYY-MM-DD"),
            )
        }),
    }
}

pub(crate) fn classify(
    event: &GcalEvent,
    window: &DayWindow,
    config: &GcalConfig,
) -> Inclusion {
    if !matches!(event.event_type.as_str(), "default" | "fromGmail") {
        return Inclusion::Omitted(format!("event type: {}", event.event_type));
    }
    if event.status == "cancelled" {
        return Inclusion::Omitted("cancelled".to_string());
    }
    match event.my_answer {
        MyAnswer::No => return Inclusion::Omitted("declined".to_string()),
        MyAnswer::NoReply => {
            return Inclusion::Omitted("reply in Calendar".to_string());
        }
        MyAnswer::Yes | MyAnswer::Maybe => {}
    }

    match (&event.start, &event.end) {
        (EventDateTime::AllDay(start), EventDateTime::AllDay(end)) => {
            if end <= start {
                return Inclusion::Omitted("invalid all-day range".to_string());
            }
            if *start <= window.day && window.day < *end {
                Inclusion::Context(all_day_reason(event, window.day))
            } else {
                Inclusion::Omitted("does not cover this day".to_string())
            }
        }
        (EventDateTime::Timed(start), EventDateTime::Timed(end)) => {
            if end < start {
                return Inclusion::Omitted("end is before start".to_string());
            }
            let duration = end.signed_duration_since(*start);
            let start_day = window.local(start).date();
            let overlaps = *start < window.to && *end > window.from;
            if !overlaps {
                return Inclusion::Omitted(
                    "does not overlap this day".to_string(),
                );
            }
            if duration < Duration::hours(24) && start_day != window.day {
                return Inclusion::Omitted(
                    "timed events belong to their local start day".to_string(),
                );
            }
            let short_and_starts_today =
                duration < Duration::hours(24) && start_day == window.day;
            let busy = event.transparency != "transparent";
            match event.my_answer {
                MyAnswer::Yes if short_and_starts_today && busy => {
                    Inclusion::Task
                }
                MyAnswer::Maybe
                    if short_and_starts_today
                        && busy
                        && config.tentative()
                            == crate::native::config::GcalTentative::Task =>
                {
                    Inclusion::Task
                }
                MyAnswer::Yes => Inclusion::Context(if busy {
                    "multi-day".to_string()
                } else {
                    "free".to_string()
                }),
                MyAnswer::Maybe => Inclusion::Context(if busy {
                    "maybe".to_string()
                } else {
                    "free, maybe".to_string()
                }),
                MyAnswer::No | MyAnswer::NoReply => {
                    unreachable!("answers were filtered above")
                }
            }
        }
        _ => Inclusion::Omitted("invalid timed/all-day range".to_string()),
    }
}

fn all_day_reason(event: &GcalEvent, day: NaiveDate) -> String {
    match (&event.start, &event.end) {
        (EventDateTime::AllDay(start), EventDateTime::AllDay(end)) => {
            let days = (*end - *start).num_days();
            if days > 1 {
                format!(
                    "all-day · day {} of {days}",
                    (day - *start).num_days() + 1
                )
            } else {
                "all-day".to_string()
            }
        }
        _ => "all-day".to_string(),
    }
}

fn local_midnight(
    day: NaiveDate,
    timezone: Option<Tz>,
) -> Option<DateTime<FixedOffset>> {
    match timezone {
        Some(timezone) => first_valid_midnight(&timezone, day),
        None => first_valid_midnight(&Local, day),
    }
}

fn first_valid_midnight<Tz: TimeZone>(
    timezone: &Tz,
    day: NaiveDate,
) -> Option<DateTime<FixedOffset>> {
    let midnight = day.and_hms_opt(0, 0, 0)?;
    // A handful of zones move their clocks at midnight. Advance to the first
    // representable minute on this date; never approximate the next boundary
    // as the previous one plus 24 hours.
    for minute in 0..=24 * 60 {
        let candidate = midnight + Duration::minutes(minute);
        if candidate.date() != day {
            break;
        }
        match timezone.from_local_datetime(&candidate) {
            chrono::LocalResult::Single(value) => {
                return Some(value.fixed_offset())
            }
            chrono::LocalResult::Ambiguous(first, second) => {
                return Some(if first.timestamp() <= second.timestamp() {
                    first.fixed_offset()
                } else {
                    second.fixed_offset()
                });
            }
            chrono::LocalResult::None => {}
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native::config::{GcalSettings, GcalTentative};
    use serde_json::Value;

    fn config(tentative: GcalTentative) -> GcalConfig {
        GcalConfig::from_settings(GcalSettings {
            tentative: Some(tentative),
            ..GcalSettings::default()
        })
        .expect("config")
    }

    fn fixture_events() -> Vec<GcalEvent> {
        let value: Value = serde_json::from_str(include_str!(
            "../../../tests/fixtures/gcal/events.json"
        ))
        .expect("fixture JSON");
        value["events"]
            .as_array()
            .expect("events array")
            .iter()
            .cloned()
            .map(|value| {
                GcalEvent::from_google("primary", value).expect("event")
            })
            .collect()
    }

    #[test]
    fn fixture_covers_inclusion_rsvp_day_ownership_and_foreign_offsets() {
        let day = NaiveDate::from_ymd_opt(2026, 10, 30).expect("day");
        let window = DayWindow::new(day, Some(chrono_tz::America::New_York))
            .expect("DST-aware day");
        let config = config(GcalTentative::Context);
        let events = fixture_events();
        let inclusion = |id: &str| {
            let event = events
                .iter()
                .find(|event| event.key.event_id == id)
                .expect("fixture event");
            classify(event, &window, &config)
        };

        assert_eq!(inclusion("interview"), Inclusion::Task);
        assert!(
            matches!(inclusion("class"), Inclusion::Context(reason) if reason == "maybe")
        );
        assert!(matches!(inclusion("holiday"), Inclusion::Context(_)));
        assert!(
            matches!(inclusion("multi-day"), Inclusion::Context(reason) if reason.contains("day 2 of 2"))
        );
        assert!(matches!(
            inclusion("timed-multi-day"),
            Inclusion::Context(_)
        ));
        assert!(
            matches!(inclusion("free"), Inclusion::Context(reason) if reason == "free")
        );
        assert!(
            matches!(inclusion("reply"), Inclusion::Omitted(reason) if reason == "reply in Calendar")
        );
        assert!(
            matches!(inclusion("declined"), Inclusion::Omitted(reason) if reason == "declined")
        );
        assert!(
            matches!(inclusion("focus"), Inclusion::Omitted(reason) if reason == "event type: focusTime")
        );
        assert_eq!(inclusion("gmail"), Inclusion::Task);
        assert_eq!(inclusion("foreign-zone"), Inclusion::Task);
        assert_eq!(inclusion("cross-midnight"), Inclusion::Task);
    }

    #[test]
    fn tentative_task_setting_promotes_only_eligible_busy_events() {
        let day = NaiveDate::from_ymd_opt(2026, 10, 30).expect("day");
        let window = DayWindow::new(day, Some(chrono_tz::America::New_York))
            .expect("day");
        let event = fixture_events()
            .into_iter()
            .find(|event| event.key.event_id == "class")
            .expect("Maybe event");
        assert_eq!(
            classify(&event, &window, &config(GcalTentative::Task)),
            Inclusion::Task
        );
    }

    #[test]
    fn local_day_windows_are_23_and_25_hours_at_dst_changes() {
        let spring = DayWindow::new(
            NaiveDate::from_ymd_opt(2026, 3, 8).expect("spring day"),
            Some(chrono_tz::America::New_York),
        )
        .expect("spring window");
        let fall = DayWindow::new(
            NaiveDate::from_ymd_opt(2026, 11, 1).expect("fall day"),
            Some(chrono_tz::America::New_York),
        )
        .expect("fall window");
        assert_eq!(
            spring.to.signed_duration_since(spring.from).num_hours(),
            23
        );
        assert_eq!(fall.to.signed_duration_since(fall.from).num_hours(), 25);
    }

    #[test]
    fn day_aliases_and_explicit_dates_are_local_calendar_days() {
        let today = NaiveDate::from_ymd_opt(2026, 10, 30).expect("today");
        assert_eq!(resolve_day("today", today).unwrap(), today);
        assert_eq!(
            resolve_day("tomorrow", today).unwrap(),
            NaiveDate::from_ymd_opt(2026, 10, 31).unwrap()
        );
        assert_eq!(
            resolve_day("yesterday", today).unwrap(),
            NaiveDate::from_ymd_opt(2026, 10, 29).unwrap()
        );
        assert_eq!(
            resolve_day("2026-11-01", today).unwrap(),
            NaiveDate::from_ymd_opt(2026, 11, 1).unwrap()
        );
        assert_eq!(resolve_day("bad", today).unwrap_err().exit_code(), 2);
    }
}
