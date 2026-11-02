//! Normalized Google Calendar event data. Provider descriptions and attendee
//! lists are parsed only long enough to derive Bob's route, RSVP, and
//! occupied-time buffer.

use chrono::{
    DateTime, FixedOffset, Local, NaiveDate, NaiveDateTime, TimeZone,
};
use serde::Deserialize;

use crate::native::event_task::{parse_buffer_token, BufferPart, EventBuffer};

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
    pub untracked: bool,
    pub buffer: EventBuffer,
    pub off_clock: bool,
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
        let raw_title = raw
            .summary
            .filter(|summary| !summary.trim().is_empty())
            .unwrap_or_else(|| "(untitled event)".to_string());
        let (title, title_off_clock) = strip_off_clock_tokens(&raw_title);
        let directives = raw
            .description
            .as_deref()
            .map(description_directives)
            .unwrap_or_default();
        let off_clock = title_off_clock || directives.untracked;
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
            untracked: off_clock,
            buffer: directives.buffer,
            route_hint: directives.route,
            off_clock,
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

/// Combined route, untracked, and buffer from HTML-normalized directive
/// lines. The first route and the first value per buffer side win.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct DescriptionDirectives {
    pub route: Option<String>,
    pub untracked: bool,
    pub buffer: EventBuffer,
}

pub(crate) fn description_directives(
    description: &str,
) -> DescriptionDirectives {
    let normalized = normalize_description_html(description);
    let mut out = DescriptionDirectives::default();
    let mut before_set = false;
    let mut after_set = false;
    for line in normalized.lines() {
        let tokens: Vec<&str> = line
            .split(|ch: char| ch.is_whitespace() || ch == ',')
            .filter(|token| !token.is_empty())
            .collect();
        if tokens.is_empty() || !tokens.iter().copied().all(is_directive_token)
        {
            continue;
        }
        for token in tokens {
            if let Some(route) = parse_route_token(token) {
                if out.route.is_none() {
                    out.route = Some(route);
                }
                continue;
            }
            if is_off_clock_token(token) {
                out.untracked = true;
                continue;
            }
            let Some((part, minutes)) = parse_buffer_token(token) else {
                continue;
            };
            match part {
                BufferPart::Before if !before_set => {
                    out.buffer.before_min = minutes;
                    before_set = true;
                }
                BufferPart::After if !after_set => {
                    out.buffer.after_min = minutes;
                    after_set = true;
                }
                _ => {}
            }
        }
    }
    out
}

fn is_directive_token(token: &str) -> bool {
    parse_route_token(token).is_some()
        || is_off_clock_token(token)
        || parse_buffer_token(token).is_some()
}

/// Turn Google HTML descriptions into plain directive text: `br`/`p`/
/// `div`/`li` boundaries become line breaks, other tags are stripped,
/// then common entities are decoded.
fn normalize_description_html(input: &str) -> String {
    decode_html_entities(&replace_html_tags(input))
}

fn replace_html_tags(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut rest = input;
    while let Some(start) = rest.find('<') {
        out.push_str(&rest[..start]);
        rest = &rest[start..];
        let Some(end) = rest.find('>') else {
            out.push_str(rest);
            return out;
        };
        let tag = &rest[1..end];
        rest = &rest[end + 1..];
        if is_break_tag(tag) {
            out.push('\n');
        }
    }
    out.push_str(rest);
    out
}

fn is_break_tag(tag: &str) -> bool {
    let tag = tag.trim();
    let tag = tag.strip_prefix('/').unwrap_or(tag);
    let name = tag
        .split(|ch: char| ch.is_whitespace() || ch == '/')
        .next()
        .unwrap_or("");
    name.eq_ignore_ascii_case("br")
        || name.eq_ignore_ascii_case("p")
        || name.eq_ignore_ascii_case("div")
        || name.eq_ignore_ascii_case("li")
}

fn decode_html_entities(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find('&') {
        out.push_str(&rest[..start]);
        rest = &rest[start..];
        let end = rest.find(';').unwrap_or(rest.len());
        if end == rest.len() {
            out.push_str(rest);
            return out;
        }
        let entity = &rest[1..end];
        let decoded = match entity {
            "nbsp" => Some(' '),
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "#39" | "apos" => Some('\''),
            _ => decode_numeric_entity(entity),
        };
        match decoded {
            Some(ch) => {
                out.push(ch);
                rest = &rest[end + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

fn decode_numeric_entity(entity: &str) -> Option<char> {
    let digits = if let Some(hex) = entity
        .strip_prefix("#x")
        .or_else(|| entity.strip_prefix("#X"))
    {
        u32::from_str_radix(hex, 16).ok()?
    } else {
        entity.strip_prefix('#')?.parse::<u32>().ok()?
    };
    char::from_u32(digits)
}

/// A standalone off-clock spelling in a Google title (case-insensitive,
/// whitespace-bounded, trailing `.,;:!?` allowed).
fn is_off_clock_token(token: &str) -> bool {
    let stripped = token.trim_end_matches(['.', ',', ';', ':', '!', '?']);
    ["#offclock", "#off-clock", "#untracked"]
        .iter()
        .any(|tag| stripped.eq_ignore_ascii_case(tag))
}

/// Strip off-clock aliases from a Google title. Returns the cleaned
/// title and whether any token was removed. When no token is present the
/// title is returned unchanged so journal hashes stay stable.
fn strip_off_clock_tokens(title: &str) -> (String, bool) {
    let lower = title.to_ascii_lowercase();
    if !["#offclock", "#off-clock", "#untracked"]
        .iter()
        .any(|tag| lower.contains(tag))
    {
        return (title.to_string(), false);
    }
    let kept: Vec<&str> = title
        .split_whitespace()
        .filter(|token| !is_off_clock_token(token))
        .collect();
    if kept.len() == title.split_whitespace().count() {
        return (title.to_string(), false);
    }
    let cleaned = kept.join(" ");
    if cleaned.trim().is_empty() {
        ("(untitled event)".to_string(), true)
    } else {
        (cleaned, true)
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};

    fn event(title: &str, description: Option<&str>) -> GcalEvent {
        let mut value = json!({
            "id": "event-1",
            "summary": title,
            "status": "confirmed",
            "start": {"dateTime": "2026-11-01T12:00:00-04:00"},
            "end": {"dateTime": "2026-11-01T13:00:00-04:00"},
            "attendees": [{"self": true, "responseStatus": "accepted"}]
        });
        if let Some(description) = description {
            value["description"] = json!(description);
        }
        GcalEvent::from_google("primary", value).expect("event")
    }

    #[test]
    fn description_directive_vectors() {
        let vectors: Value = serde_json::from_str(include_str!(
            "../../../tests/fixtures/event_buffer/vectors.json"
        ))
        .expect("shared vectors JSON");
        for case in vectors["description"].as_array().expect("description") {
            let parsed =
                description_directives(case["input"].as_str().expect("input"));
            assert_eq!(
                parsed.route.as_deref(),
                case["route"].as_str(),
                "{}",
                case["id"]
            );
            assert_eq!(
                parsed.untracked,
                case["untracked"].as_bool().expect("untracked"),
                "{}",
                case["id"]
            );
            let before =
                u16::try_from(case["before"].as_u64().expect("before"))
                    .expect("before fits");
            let after = u16::try_from(case["after"].as_u64().expect("after"))
                .expect("after fits");
            assert_eq!(
                parsed.buffer,
                EventBuffer {
                    before_min: before,
                    after_min: after,
                },
                "{}",
                case["id"]
            );
        }
    }

    #[test]
    fn title_aliases_are_stripped_before_route_parsing() {
        for token in ["#offclock", "#OFFCLOCK.", "#off-clock", "#untracked!"] {
            let parsed = event(&format!("Lunch @love {token}"), None);
            assert_eq!(parsed.title, "Lunch @love", "token {token}");
            assert_eq!(parsed.title_route_hint.as_deref(), Some("love"));
            assert!(parsed.off_clock, "token {token}");
            assert!(parsed.untracked, "legacy alias for {token}");
        }
    }

    #[test]
    fn description_aliases_and_buffers_combine_only_on_directive_lines() {
        for token in ["#offclock", "#OFF-CLOCK", "#untracked"] {
            let parsed =
                event("Lunch", Some(&format!("Bring shoes\n {token} \n")));
            assert!(parsed.off_clock, "token {token}");
        }
        let parsed = event("Lunch", Some("@love #off-clock -30 +15"));
        assert_eq!(parsed.route_hint.as_deref(), Some("love"));
        assert!(parsed.off_clock);
        assert_eq!(parsed.buffer.before_min, 30);
        assert_eq!(parsed.buffer.after_min, 15);
        assert!(!event("Lunch", Some("text #offclock")).off_clock);
    }
}
