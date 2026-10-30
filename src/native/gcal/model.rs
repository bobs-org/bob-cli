//! Normalized Google Calendar event data. Provider descriptions and attendee
//! lists are parsed only long enough to derive Bob's route and RSVP.

use chrono::{
    DateTime, FixedOffset, Local, NaiveDate, NaiveDateTime, TimeZone,
};
use serde::Deserialize;

use super::GcalError;

#[allow(dead_code)] // Lifecycle and sync phases consume the preserved fields.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GcalEvent {
    pub key: OccurrenceKey,
    pub ical_uid: Option<String>,
    pub series_id: Option<String>,
    pub original_start: Option<EventDateTime>,
    pub status: String,
    pub title: String,
    pub title_route_hint: Option<String>,
    pub route_hint: Option<String>,
    pub location: Option<String>,
    pub html_link: Option<String>,
    pub join_link: Option<String>,
    pub start: EventDateTime,
    pub end: EventDateTime,
    pub transparency: String,
    pub event_type: String,
    pub color_id: Option<String>,
    pub my_answer: MyAnswer,
    pub organizer_or_no_guests: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OccurrenceKey {
    pub calendar_id: String,
    pub event_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum EventDateTime {
    Timed(DateTime<FixedOffset>),
    AllDay(NaiveDate),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MyAnswer {
    Yes,
    Maybe,
    No,
    NoReply,
}

impl MyAnswer {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Yes => "yes",
            Self::Maybe => "maybe",
            Self::No => "no",
            Self::NoReply => "no_reply",
        }
    }
}

impl GcalEvent {
    /// Parse one raw `calendar.events` item. An invalid item makes the entire
    /// calendar snapshot unusable; callers must never treat a partial parse as
    /// a complete day.
    pub(crate) fn from_google(
        calendar_id: &str,
        value: serde_json::Value,
    ) -> Result<Self, GcalError> {
        let raw: RawEvent = serde_json::from_value(value).map_err(|error| {
            GcalError::runtime(
                "bad_json",
                format!("parse Google Calendar event: {error}"),
            )
        })?;
        let start = parse_event_time(raw.start, "start")?;
        let end = parse_event_time(raw.end, "end")?;
        if !matches!(
            (&start, &end),
            (EventDateTime::Timed(_), EventDateTime::Timed(_))
                | (EventDateTime::AllDay(_), EventDateTime::AllDay(_))
        ) {
            return Err(GcalError::runtime(
                "bad_json",
                format!("event {:?} mixes timed and all-day endpoints", raw.id),
            ));
        }
        let (my_answer, organizer_or_no_guests) =
            my_answer(raw.attendees.as_deref(), raw.organizer.as_ref());
        let join_link = raw
            .hangout_link
            .filter(|link| !link.trim().is_empty())
            .or_else(|| {
                raw.conference_data.and_then(|conference| {
                    conference
                        .entry_points
                        .into_iter()
                        .find(|point| {
                            point.entry_point_type.as_deref() == Some("video")
                        })
                        .and_then(|point| point.uri)
                        .filter(|uri| !uri.trim().is_empty())
                })
            })
            .or_else(|| raw.location.as_deref().and_then(location_url));
        let title = raw
            .summary
            .filter(|summary| !summary.trim().is_empty())
            .unwrap_or_else(|| "(untitled event)".to_string());
        let title_route_hint =
            title.split_whitespace().last().and_then(parse_route_token);
        Ok(Self {
            key: OccurrenceKey {
                calendar_id: calendar_id.to_string(),
                event_id: raw.id,
            },
            ical_uid: raw.ical_uid,
            series_id: raw.recurring_event_id,
            original_start: raw
                .original_start_time
                .map(|time| parse_event_time(Some(time), "originalStartTime"))
                .transpose()?,
            status: raw.status.unwrap_or_else(|| "confirmed".to_string()),
            title,
            title_route_hint,
            route_hint: raw.description.as_deref().and_then(description_route),
            location: raw.location,
            html_link: raw.html_link,
            join_link,
            start,
            end,
            transparency: raw
                .transparency
                .unwrap_or_else(|| "opaque".to_string()),
            event_type: raw.event_type.unwrap_or_else(|| "default".to_string()),
            color_id: raw.color_id,
            my_answer,
            organizer_or_no_guests,
        })
    }
}

fn parse_event_time(
    raw: Option<RawEventTime>,
    name: &str,
) -> Result<EventDateTime, GcalError> {
    let Some(raw) = raw else {
        return Err(GcalError::runtime(
            "bad_json",
            format!("Google Calendar event is missing {name}"),
        ));
    };
    match (raw.date_time, raw.date) {
        (Some(value), None) => parse_instant(&value, raw.time_zone.as_deref())
            .map(EventDateTime::Timed)
            .ok_or_else(|| {
                GcalError::runtime(
                    "bad_json",
                    format!("Google Calendar event has invalid {name} dateTime {value:?}"),
                )
            }),
        (None, Some(value)) => NaiveDate::parse_from_str(&value, "%Y-%m-%d")
            .map(EventDateTime::AllDay)
            .map_err(|error| {
                GcalError::runtime(
                    "bad_json",
                    format!("Google Calendar event has invalid {name} date {value:?}: {error}"),
                )
            }),
        _ => Err(GcalError::runtime(
            "bad_json",
            format!("Google Calendar event {name} must have exactly one of dateTime or date"),
        )),
    }
}

fn parse_instant(
    value: &str,
    timezone: Option<&str>,
) -> Option<DateTime<FixedOffset>> {
    if let Ok(value) = DateTime::parse_from_rfc3339(value) {
        return Some(value);
    }
    let naive = NaiveDateTime::parse_from_str(value, "%Y-%m-%dT%H:%M:%S%.f")
        .or_else(|_| NaiveDateTime::parse_from_str(value, "%Y-%m-%dT%H:%M:%S"))
        .ok()?;
    if let Some(timezone) =
        timezone.and_then(|name| name.parse::<chrono_tz::Tz>().ok())
    {
        return local_result(timezone.from_local_datetime(&naive));
    }
    local_result(Local.from_local_datetime(&naive))
}

fn local_result<Tz: TimeZone>(
    result: chrono::LocalResult<DateTime<Tz>>,
) -> Option<DateTime<FixedOffset>> {
    match result {
        chrono::LocalResult::Single(value) => Some(value.fixed_offset()),
        chrono::LocalResult::Ambiguous(first, second) => {
            let (earlier, _) = if first.timestamp() <= second.timestamp() {
                (first, second)
            } else {
                (second, first)
            };
            Some(earlier.fixed_offset())
        }
        chrono::LocalResult::None => None,
    }
}

fn my_answer(
    attendees: Option<&[RawAttendee]>,
    organizer: Option<&RawOrganizer>,
) -> (MyAnswer, bool) {
    if organizer.is_some_and(|organizer| organizer.is_self) {
        return (MyAnswer::Yes, true);
    }
    let Some(attendees) = attendees else {
        return (MyAnswer::Yes, true);
    };
    if attendees.is_empty() {
        return (MyAnswer::Yes, true);
    }
    let Some(me) = attendees.iter().find(|attendee| attendee.is_self) else {
        return (MyAnswer::NoReply, false);
    };
    let answer = match me.response_status.as_deref() {
        Some("accepted") => MyAnswer::Yes,
        Some("tentative") => MyAnswer::Maybe,
        Some("declined") => MyAnswer::No,
        _ => MyAnswer::NoReply,
    };
    (answer, false)
}

fn parse_route_token(token: &str) -> Option<String> {
    let route = token.strip_prefix('@')?;
    if route.is_empty() || route.chars().any(char::is_whitespace) {
        return None;
    }
    Some(route.to_string())
}

fn description_route(description: &str) -> Option<String> {
    description.lines().find_map(|line| {
        let line = line.trim();
        if !line.starts_with('@') || line.split_whitespace().count() != 1 {
            return None;
        }
        parse_route_token(line)
    })
}

fn location_url(location: &str) -> Option<String> {
    let url = location.trim();
    (url.starts_with("https://") || url.starts_with("http://"))
        .then(|| url.to_string())
}

#[derive(Debug, Deserialize)]
struct RawEvent {
    id: String,
    #[serde(rename = "iCalUID")]
    ical_uid: Option<String>,
    #[serde(rename = "recurringEventId")]
    recurring_event_id: Option<String>,
    #[serde(rename = "originalStartTime")]
    original_start_time: Option<RawEventTime>,
    status: Option<String>,
    summary: Option<String>,
    description: Option<String>,
    location: Option<String>,
    #[serde(rename = "htmlLink")]
    html_link: Option<String>,
    #[serde(rename = "hangoutLink")]
    hangout_link: Option<String>,
    #[serde(rename = "conferenceData")]
    conference_data: Option<RawConferenceData>,
    start: Option<RawEventTime>,
    end: Option<RawEventTime>,
    transparency: Option<String>,
    #[serde(rename = "eventType")]
    event_type: Option<String>,
    #[serde(rename = "colorId")]
    color_id: Option<String>,
    attendees: Option<Vec<RawAttendee>>,
    organizer: Option<RawOrganizer>,
}

#[derive(Debug, Deserialize)]
struct RawEventTime {
    #[serde(rename = "dateTime")]
    date_time: Option<String>,
    date: Option<String>,
    #[serde(rename = "timeZone")]
    time_zone: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RawAttendee {
    #[serde(rename = "self")]
    #[serde(default)]
    is_self: bool,
    #[serde(rename = "responseStatus")]
    response_status: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RawOrganizer {
    #[serde(rename = "self")]
    #[serde(default)]
    is_self: bool,
}

#[derive(Debug, Deserialize)]
struct RawConferenceData {
    #[serde(rename = "entryPoints", default)]
    entry_points: Vec<RawEntryPoint>,
}

#[derive(Debug, Deserialize)]
struct RawEntryPoint {
    #[serde(rename = "entryPointType")]
    entry_point_type: Option<String>,
    uri: Option<String>,
}
