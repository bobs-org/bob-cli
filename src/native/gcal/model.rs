//! Normalized Google Calendar event data. Provider descriptions are parsed
//! into structured lines (and directive verdicts) at event load; attendee
//! lists yield RSVP. Occupied-time buffers come from those directives.

use chrono::{
    DateTime, FixedOffset, Local, NaiveDate, NaiveDateTime, TimeZone,
};
use serde::Deserialize;

use crate::native::{
    clock_tags::{google_clock_token, merge_clock_tags, ClockKind, ClockTag},
    event_task::{BufferPart, EventBuffer},
};

use super::{
    description::{
        parse_description, parse_note, BufferReason, DescriptionContext,
        EventDescription,
    },
    journal::{self, NoteScope},
    GcalError,
};
use serde_json::{json, Value};

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
    pub clock_tag: ClockTag,
    pub deprecated_title_tags: Vec<String>,
    pub deprecated_description_tags: Vec<String>,
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
    pub organizer_name: Option<String>,
    pub organizer_is_self: bool,
    pub description: EventDescription,
    /// Layered private-note summary, when a matching note was applied.
    pub note: Option<AppliedNote>,
}

/// Which private-note scopes were present when notes were applied.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AppliedNoteScope {
    Event,
    Series,
    Both,
}

impl AppliedNoteScope {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Event => "event",
            Self::Series => "series",
            Self::Both => "both",
        }
    }
}

/// One field a private note contributed to the layered event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NoteField {
    Root,
    Clock,
    Before,
    After,
    Bullets,
}

impl NoteField {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Root => "root",
            Self::Clock => "clock",
            Self::Before => "before",
            Self::After => "after",
            Self::Bullets => "bullets",
        }
    }
}

/// Parsed private note ready to layer over a Google event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ParsedNote {
    pub scope: NoteScope,
    pub text: String,
    pub description: EventDescription,
}

impl ParsedNote {
    pub(crate) fn parse(scope: NoteScope, text: &str) -> Self {
        Self {
            scope,
            text: text.to_string(),
            description: parse_note(
                text,
                &DescriptionContext {
                    join_link: None,
                    location: None,
                },
            ),
        }
    }
}

/// What a private note contributed after per-field layering.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AppliedNote {
    pub scope: AppliedNoteScope,
    pub event_route: Option<String>,
    pub series_route: Option<String>,
    pub fields: Vec<NoteField>,
    #[allow(dead_code)] // Provenance records this hash on `gcal.row` later.
    pub hash: String,
}

impl AppliedNote {
    pub(crate) fn provenance(&self) -> journal::NoteProvenance {
        journal::NoteProvenance {
            hash: self.hash.clone(),
            fields: self
                .fields
                .iter()
                .map(|field| field.as_str().to_string())
                .collect(),
        }
    }

    fn retain_won_fields(&mut self, root_won: bool, bullets_won: bool) {
        self.fields.retain(|field| match field {
            NoteField::Root => root_won,
            NoteField::Bullets => bullets_won,
            _ => true,
        });
    }

    pub(crate) fn json(&self, bullet_lines: usize) -> Value {
        json!({
            "scope": self.scope.as_str(),
            "fields": self.fields.iter().map(|field| field.as_str()).collect::<Vec<_>>(),
            "bullet_lines": bullet_lines,
        })
    }
}

impl GcalEvent {
    /// External events are those organized by someone else or created from
    /// a Gmail reservation, which can list the account owner as organizer.
    #[allow(dead_code)] // The tag and review phases classify external events.
    pub(crate) fn is_external(&self) -> bool {
        !self.organizer_is_self || self.event_type == "fromGmail"
    }

    /// Drop `root` unless a note route won, and `bullets` unless a note
    /// bullet is actually rendered.
    pub(crate) fn finalize_note_fields(
        &mut self,
        root_won: bool,
        bullets_won: bool,
    ) {
        if let Some(note) = &mut self.note {
            note.retain_won_fields(root_won, bullets_won);
        }
    }
}

pub(crate) fn note_summary_json(event: &GcalEvent) -> Value {
    match &event.note {
        Some(note) => note.json(event.description.note_lines.len()),
        None => Value::Null,
    }
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
        let (title, title_clock_tag) = strip_google_clock_tokens(&raw_title);
        let description = parse_description(
            raw.description.as_deref().unwrap_or(""),
            &DescriptionContext {
                join_link: join_link.as_deref(),
                location: raw.location.as_deref(),
            },
        );
        let clock_tag = merge_clock_tags(
            &title_clock_tag,
            &description.directives.clock_tag,
        );
        let off_clock = clock_tag.explicit_off();
        let deprecated_title_tags = title_clock_tag.deprecated().to_vec();
        let deprecated_description_tags =
            description.directives.clock_tag.deprecated().to_vec();
        let title_route_hint =
            title.split_whitespace().last().and_then(parse_route_token);
        let organizer_name = raw
            .organizer
            .as_ref()
            .and_then(|organizer| organizer.display_label());
        let organizer_is_self = raw
            .organizer
            .as_ref()
            .is_some_and(|organizer| organizer.is_self);
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
            clock_tag,
            deprecated_title_tags,
            deprecated_description_tags,
            untracked: off_clock,
            buffer: description.directives.buffer,
            route_hint: description.directives.route.clone(),
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
            organizer_name,
            organizer_is_self,
            description,
            note: None,
        })
    }

    /// Layer occurrence then series notes over Google's description,
    /// field by field. Google keeps time, title, and badge links.
    pub(crate) fn apply_notes(
        &mut self,
        occurrence: Option<&ParsedNote>,
        series: Option<&ParsedNote>,
    ) {
        if occurrence.is_none() && series.is_none() {
            self.note = None;
            return;
        }

        let mut fields = Vec::new();
        let event_route = occurrence
            .and_then(|note| note.description.directives.route.clone());
        let series_route =
            series.and_then(|note| note.description.directives.route.clone());
        if event_route.is_some() || series_route.is_some() {
            fields.push(NoteField::Root);
        }

        let occ_clock =
            occurrence.map(|note| &note.description.directives.clock_tag);
        let series_clock =
            series.map(|note| &note.description.directives.clock_tag);
        if occ_clock.is_some_and(|tag| tag.kind() != ClockKind::None) {
            self.clock_tag = occ_clock.expect("set").clone();
            self.deprecated_description_tags =
                self.clock_tag.deprecated().to_vec();
            fields.push(NoteField::Clock);
        } else if series_clock.is_some_and(|tag| tag.kind() != ClockKind::None)
        {
            self.clock_tag = series_clock.expect("set").clone();
            self.deprecated_description_tags =
                self.clock_tag.deprecated().to_vec();
            fields.push(NoteField::Clock);
        }
        self.off_clock = self.clock_tag.explicit_off();
        self.untracked = self.off_clock;

        let mut note_reason = false;
        if let Some((minutes, reason)) =
            layered_side(occurrence, series, BufferPart::Before)
        {
            self.buffer.before_min = minutes;
            self.description.buffer_reasons.before = reason.clone();
            fields.push(NoteField::Before);
            note_reason |= reason.is_some();
        }
        if let Some((minutes, reason)) =
            layered_side(occurrence, series, BufferPart::After)
        {
            self.buffer.after_min = minutes;
            self.description.buffer_reasons.after = reason.clone();
            fields.push(NoteField::After);
            note_reason |= reason.is_some();
        }

        let mut note_lines = Vec::new();
        if let Some(note) = occurrence {
            note_lines.extend(note.description.lines.clone());
        }
        if let Some(note) = series {
            note_lines.extend(note.description.lines.clone());
        }
        if !note_lines.is_empty() || note_reason {
            fields.push(NoteField::Bullets);
        }
        self.description.note_lines = note_lines;

        let mut texts = Vec::new();
        if let Some(note) = occurrence {
            texts.push(note.text.as_str());
        }
        if let Some(note) = series {
            texts.push(note.text.as_str());
        }
        self.note = Some(AppliedNote {
            scope: match (occurrence.is_some(), series.is_some()) {
                (true, true) => AppliedNoteScope::Both,
                (true, false) => AppliedNoteScope::Event,
                (false, true) => AppliedNoteScope::Series,
                (false, false) => unreachable!("early return"),
            },
            event_route,
            series_route,
            fields,
            hash: journal::span_hash(&texts.join("\n")),
        });
    }
}

fn layered_side(
    occurrence: Option<&ParsedNote>,
    series: Option<&ParsedNote>,
    part: BufferPart,
) -> Option<(u16, Option<BufferReason>)> {
    side_from(occurrence, part).or_else(|| side_from(series, part))
}

fn side_from(
    note: Option<&ParsedNote>,
    part: BufferPart,
) -> Option<(u16, Option<BufferReason>)> {
    let note = note?;
    let directives = &note.description.directives;
    let reasons = &note.description.buffer_reasons;
    match part {
        BufferPart::Before if directives.before_set => {
            Some((directives.buffer.before_min, reasons.before.clone()))
        }
        BufferPart::After if directives.after_set => {
            Some((directives.buffer.after_min, reasons.after.clone()))
        }
        _ => None,
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

pub(crate) fn parse_instant(
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

pub(super) fn parse_route_token(token: &str) -> Option<String> {
    let route = token.strip_prefix('@')?;
    if route.is_empty() || route.chars().any(char::is_whitespace) {
        return None;
    }
    Some(route.to_string())
}

/// Strip all explicit Google clock tokens before looking for a trailing
/// `@route`. With no token, preserve the exact title bytes so journal hashes
/// remain stable.
fn strip_google_clock_tokens(title: &str) -> (String, ClockTag) {
    let mut kept = Vec::new();
    let mut clock_tag = ClockTag::None;
    let mut removed = false;
    for token in title.split_whitespace() {
        if let Some(found) = google_clock_token(token) {
            clock_tag = merge_clock_tags(&clock_tag, &found);
            removed = true;
        } else {
            kept.push(token);
        }
    }
    if !removed {
        return (title.to_string(), ClockTag::None);
    }
    let cleaned = kept.join(" ");
    if cleaned.trim().is_empty() {
        ("(untitled event)".to_string(), clock_tag)
    } else {
        (cleaned, clock_tag)
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
    #[serde(rename = "displayName")]
    display_name: Option<String>,
    #[serde(default)]
    email: Option<String>,
}

impl RawOrganizer {
    fn display_label(&self) -> Option<String> {
        self.display_name
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .or_else(|| {
                self.email
                    .as_deref()
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(str::to_string)
            })
    }
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
    use crate::native::clock_tags::ClockKind;
    use serde_json::json;

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
    fn event_description_debug_redacts_text() {
        let parsed = event(
            "Lunch",
            Some("SECRET_PHRASE https://secret.example/x\n@love"),
        );
        let debug = format!("{parsed:?}");
        assert!(!debug.contains("SECRET_PHRASE"), "{debug}");
        assert!(!debug.contains("secret.example"), "{debug}");
        assert_eq!(parsed.route_hint.as_deref(), Some("love"));
        assert_eq!(parsed.description.lines.len(), 1);
    }

    #[test]
    fn external_includes_missing_organizer_and_gmail_events() {
        assert!(event("No organizer", None).is_external());

        let owned_gmail = GcalEvent::from_google(
            "primary",
            json!({
                "id": "flight",
                "summary": "Flight",
                "eventType": "fromGmail",
                "organizer": {"self": true},
                "start": {"dateTime": "2026-11-01T12:00:00-04:00"},
                "end": {"dateTime": "2026-11-01T13:00:00-04:00"}
            }),
        )
        .expect("Gmail event");
        assert!(owned_gmail.organizer_is_self);
        assert!(owned_gmail.is_external());

        let owned = GcalEvent::from_google(
            "primary",
            json!({
                "id": "owned",
                "summary": "Owned",
                "organizer": {"self": true},
                "start": {"dateTime": "2026-11-01T12:00:00-04:00"},
                "end": {"dateTime": "2026-11-01T13:00:00-04:00"}
            }),
        )
        .expect("owned event");
        assert!(!owned.is_external());
    }

    #[test]
    fn google_title_clock_tokens_strip_before_route_parsing() {
        let spellings =
            ["#off", "#on", "#offclock", "#off-clock", "#untracked"];
        let punctuation = ["", ".", ",", ";", ":", "!", "?"];
        for spelling in spellings {
            for punctuation in punctuation {
                let token =
                    format!("{}{punctuation}", spelling.to_ascii_uppercase());
                let parsed = event(&format!("Lunch @love {token}"), None);
                assert_eq!(parsed.title, "Lunch @love", "token {token}");
                assert_eq!(parsed.title_route_hint.as_deref(), Some("love"));
                let expected = if spelling.eq_ignore_ascii_case("#off")
                    || spelling.starts_with("#off")
                    || spelling.eq_ignore_ascii_case("#untracked")
                {
                    ClockKind::Off
                } else {
                    ClockKind::On
                };
                assert_eq!(parsed.clock_tag.kind(), expected, "token {token}");
                assert_eq!(
                    parsed.off_clock,
                    expected == ClockKind::Off,
                    "{token}"
                );
                assert_eq!(parsed.untracked, parsed.off_clock, "{token}");
                if spelling.starts_with("#offclock")
                    || spelling.starts_with("#off-clock")
                    || spelling.starts_with("#untracked")
                {
                    assert_eq!(
                        parsed.deprecated_title_tags,
                        [spelling.to_ascii_lowercase()]
                    );
                } else {
                    assert!(parsed.deprecated_title_tags.is_empty(), "{token}");
                }
            }
        }
        let parsed = event("Lunch @love #on", None);
        assert_eq!(parsed.title, "Lunch @love");
        assert_eq!(parsed.title_route_hint.as_deref(), Some("love"));
        assert_eq!(parsed.clock_tag.kind(), ClockKind::On);
    }

    #[test]
    fn description_aliases_and_buffers_combine_only_on_directive_lines() {
        for token in ["#offclock", "#OFF-CLOCK", "#untracked"] {
            let parsed =
                event("Lunch", Some(&format!("Bring shoes\n {token} \n")));
            assert!(parsed.off_clock, "token {token}");
        }
        let parsed = event("Lunch", Some("@love #on -30 +15"));
        assert_eq!(parsed.route_hint.as_deref(), Some("love"));
        assert!(!parsed.off_clock);
        assert_eq!(parsed.clock_tag.kind(), ClockKind::On);
        assert_eq!(parsed.buffer.before_min, 30);
        assert_eq!(parsed.buffer.after_min, 15);
        assert!(!event("Lunch", Some("text #offclock")).off_clock);
    }

    #[test]
    fn google_clock_conflicts_are_order_independent_and_keep_alias_evidence() {
        for (title, description) in [
            ("Lunch @love #off", Some("#on")),
            ("Lunch @love #on", Some("#offclock")),
            ("Lunch @love #offclock", Some("#ON")),
        ] {
            let parsed = event(title, description);
            assert_eq!(parsed.clock_tag.kind(), ClockKind::On);
            assert!(!parsed.off_clock);
            assert!(!parsed.untracked);
        }
        let alias_in_title = event("Lunch #untracked @love", Some("#on"));
        assert_eq!(alias_in_title.deprecated_title_tags, ["#untracked"]);
        assert!(alias_in_title.deprecated_description_tags.is_empty());
        let alias_in_description = event("Lunch @love #on", Some("#off-clock"));
        assert!(alias_in_description.deprecated_title_tags.is_empty());
        assert_eq!(
            alias_in_description.deprecated_description_tags,
            ["#off-clock"]
        );
        assert_eq!(
            alias_in_description.description.directives.clock_tag.kind(),
            ClockKind::Off,
            "the description retains its own verdict"
        );

        let duplicates =
            event("Lunch #offclock #OFFCLOCK", Some("#untracked #UNTRACKED"));
        assert_eq!(duplicates.deprecated_title_tags, ["#offclock"]);
        assert_eq!(duplicates.deprecated_description_tags, ["#untracked"]);
    }

    #[test]
    fn google_title_clock_matching_requires_a_whole_token() {
        let untouched =
            "Lunch   @love #office #one #online #off/x x#off https://x/#off";
        let parsed = event(untouched, None);
        assert_eq!(parsed.title, untouched, "no-token titles keep exact bytes");
        assert_eq!(parsed.clock_tag.kind(), ClockKind::None);
        assert!(parsed.title_route_hint.is_none());
    }
}
