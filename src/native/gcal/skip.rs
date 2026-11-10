//! One vocabulary for why `bob gcal` leaves an event out of a day's note.
//!
//! `sync` and `list` share it. A [`SkipReason`] has a stable JSON `code`, a
//! human `detail`, and an optional `hint` naming the fix when you can act.
//! Precedence: an omission (the Google side) wins over a tombstone.
//! The classifier's own reason strings stay unchanged; this module only
//! adds the friendlier words beside them.

use std::collections::{BTreeMap, HashSet};

use chrono::NaiveDate;
use serde_json::{json, Value};

use super::{
    classify::{DayWindow, Inclusion},
    journal::{occurrence_key, OccurrenceState},
    model::{EventDateTime, GcalEvent},
    plan::SyncPlan,
    snapshot::SnapshotEvent,
};
use crate::native::style::Styler;

/// What to do about a skip: plain words, then an optional command that
/// renders cyan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SkipHint {
    text: String,
    command: Option<String>,
}

impl SkipHint {
    fn words(text: &str) -> Self {
        Self {
            text: text.to_string(),
            command: None,
        }
    }

    fn with_command(text: &str, command: String) -> Self {
        Self {
            text: text.to_string(),
            command: Some(command),
        }
    }

    /// Plain text, as JSON and plain terminals show it.
    pub(crate) fn plain(&self) -> String {
        match &self.command {
            Some(command) => format!("{} {command}", self.text),
            None => self.text.clone(),
        }
    }

    pub(crate) fn styled(&self, styler: &Styler) -> String {
        match &self.command {
            Some(command) => {
                format!("{} {}", self.text, styler.cyan(command))
            }
            None => self.text.clone(),
        }
    }
}

/// Why one event is left out of the day's note.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SkipReason {
    /// Stable JSON `reason` code.
    pub code: &'static str,
    pub detail: String,
    pub hint: Option<SkipHint>,
    /// `HH:MM` the tombstone was written, for `removed_by_you`.
    at: Option<String>,
}

impl SkipReason {
    fn new(code: &'static str, detail: String, hint: Option<SkipHint>) -> Self {
        Self {
            code,
            detail,
            hint,
            at: None,
        }
    }

    /// Actionable skips are ones you can undo (yellow); the rest are
    /// by-design omissions (dim).
    pub(crate) fn actionable(&self) -> bool {
        matches!(
            self.code,
            "removed_by_you" | "awaiting_reply" | "events_replaced"
        )
    }

    pub(crate) fn hint_text(&self) -> Option<String> {
        self.hint.as_ref().map(SkipHint::plain)
    }

    pub(crate) fn json(&self) -> Value {
        json!({
            "reason": self.code,
            "detail": self.detail,
            "hint": self.hint_text(),
        })
    }

    /// The classifier's omission reason, in friendlier words. `window`
    /// names the note an out-of-day timed event belongs to.
    pub(crate) fn omitted(
        reason: &str,
        event: &GcalEvent,
        window: &DayWindow,
    ) -> Self {
        match reason {
            "declined" => Self::new(
                "declined",
                "declined in Google Calendar".to_string(),
                None,
            ),
            "cancelled" => Self::new(
                "cancelled",
                "cancelled in Google Calendar".to_string(),
                None,
            ),
            "reply in Calendar" => Self::new(
                "awaiting_reply",
                "awaiting your reply".to_string(),
                Some(SkipHint::words("RSVP in Google Calendar to add it")),
            ),
            "does not cover this day"
            | "does not overlap this day"
            | "timed events belong to their local start day" => {
                Self::outside_day(event, window)
            }
            "invalid all-day range"
            | "end is before start"
            | "invalid timed/all-day range" => Self::new(
                "invalid_time",
                "Google sent an invalid time range".to_string(),
                None,
            ),
            other => match other.strip_prefix("event type: ") {
                Some(kind) => Self::new(
                    "event_type",
                    format!("Google {kind} events are not synced"),
                    None,
                ),
                None => Self::new("omitted", other.to_string(), None),
            },
        }
    }

    fn outside_day(event: &GcalEvent, window: &DayWindow) -> Self {
        let owner = match &event.start {
            EventDateTime::Timed(start) => {
                Some(window.local(start).date()).filter(|d| *d != window.day)
            }
            EventDateTime::AllDay(_) => None,
        };
        match owner {
            Some(owner) => Self::new(
                "outside_day",
                format!("belongs to {}'s note", owner.format("%a %Y-%m-%d")),
                Some(SkipHint::with_command(
                    "sync that day with",
                    format!("bob gcal sync -D {owner}"),
                )),
            ),
            None => Self::new(
                "outside_day",
                "does not cover this day".to_string(),
                None,
            ),
        }
    }

    /// A tombstoned row. `line_removed` (or no recorded reason) is your
    /// deletion; any other reason is Google's lifecycle text.
    pub(crate) fn removed(
        state: &OccurrenceState,
        day: NaiveDate,
        is_today: bool,
    ) -> Self {
        let at = state.gone_at.map(|ts| ts.format("%H:%M").to_string());
        match state.gone_reason.as_deref() {
            None | Some("line_removed") => {
                Self::removed_by_you(at, day, is_today)
            }
            Some(reason) => Self::removed_in_google(reason),
        }
    }

    /// Your deletion, with the time it was noticed when known.
    pub(crate) fn removed_by_you(
        at: Option<String>,
        day: NaiveDate,
        is_today: bool,
    ) -> Self {
        let detail = match &at {
            Some(at) => format!("removed by you at {at}"),
            None => "removed by you".to_string(),
        };
        let mut reason = Self::new(
            "removed_by_you",
            detail,
            Some(SkipHint::with_command(
                "restore with",
                restore_command(day, is_today),
            )),
        );
        reason.at = at;
        reason
    }

    pub(crate) fn removed_in_google(reason: &str) -> Self {
        Self::new("removed_in_google", reason.to_string(), None)
    }

    pub(crate) fn events_replaced(day: NaiveDate, is_today: bool) -> Self {
        Self::new(
            "events_replaced",
            "lost when the Events section was replaced".to_string(),
            Some(SkipHint::with_command(
                "restore with",
                restore_command(day, is_today),
            )),
        )
    }

    /// The `list` VAULT cell: `removed by you 05:28`, else the detail.
    pub(crate) fn vault_label(&self) -> String {
        match (self.code, &self.at) {
            ("removed_by_you", Some(at)) => format!("removed by you {at}"),
            ("removed_by_you", None) => "removed by you".to_string(),
            _ => self.detail.clone(),
        }
    }
}

/// `bob gcal sync -F`, with `-D` when the day is not today.
pub(crate) fn restore_command(day: NaiveDate, is_today: bool) -> String {
    if is_today {
        "bob gcal sync -F".to_string()
    } else {
        format!("bob gcal sync -D {day} -F")
    }
}

/// One event sync leaves out of the note.
#[derive(Debug, Clone)]
pub(crate) struct SkippedEvent {
    /// `HHMM-HHMM` or `all day`, like a row's time label.
    pub time: String,
    pub title: String,
    pub calendar_id: String,
    pub event_id: String,
    pub reason: SkipReason,
}

impl SkippedEvent {
    pub(crate) fn json(&self) -> Value {
        json!({
            "time": self.time,
            "title": self.title,
            "calendar_id": self.calendar_id,
            "event_id": self.event_id,
            "reason": self.reason.code,
            "detail": self.reason.detail,
            "hint": self.reason.hint_text(),
        })
    }

    /// `  ⊘ skipped 09:00–09:30  Team sync  declined in Google Calendar`.
    pub(crate) fn human_line(
        &self,
        styler: &Styler,
        human_time: &dyn Fn(&str) -> String,
    ) -> String {
        let sigil = if self.reason.actionable() {
            styler.yellow("⊘")
        } else {
            styler.dim("⊘")
        };
        let mut tail = styler.dim(&self.reason.detail);
        if let Some(hint) = &self.reason.hint {
            tail.push_str(&styler.dim(" · "));
            tail.push_str(&hint.styled(styler));
        }
        format!(
            "  {sigil} skipped {}  {}  {tail}",
            human_time(&self.time),
            self.title
        )
    }
}

/// The day's left-out events and how many included ones needed nothing.
#[derive(Debug, Clone, Default)]
pub(crate) struct DaySkips {
    pub skipped: Vec<SkippedEvent>,
    pub in_sync: usize,
}

/// Skipped events and the in-sync count from the final plan.
///
/// Every snapshot event is accounted for once: an omission (Google side)
/// is skipped; an included event whose tombstone predates this run and
/// whose line is still missing is skipped; an included event the plan
/// changed (including a fresh tombstone, which reports as a change row)
/// is counted nowhere here; any other included event is in sync. Events
/// stay in snapshot (time) order.
pub(crate) fn summarize(
    events: &[SnapshotEvent],
    window: &DayWindow,
    journal: &BTreeMap<(String, String), OccurrenceState>,
    plan: &SyncPlan,
    is_today: bool,
    time_label: &dyn Fn(&GcalEvent) -> String,
) -> DaySkips {
    let day_string = window.day.to_string();
    let key_of =
        |calendar: &str, event: &str| (calendar.to_string(), event.to_string());
    // Rows this run puts (back) into the note, or takes over again.
    let mut written: HashSet<(String, String)> = HashSet::new();
    for row in &plan.rows_added {
        written.insert(key_of(&row.calendar_id, &row.event_id));
    }
    for row in &plan.rows_adopted {
        written.insert(key_of(&row.calendar_id, &row.event_id));
    }
    let mut changed = written.clone();
    for row in &plan.rows_updated {
        changed.insert(key_of(&row.calendar_id, &row.event_id));
    }
    for row in &plan.rows_upgraded {
        changed.insert(key_of(&row.calendar_id, &row.event_id));
    }
    for row in &plan.rows_gone {
        changed.insert(key_of(&row.calendar_id, &row.event_id));
    }
    for link in plan.links_added.iter().chain(plan.links_refiled.iter()) {
        changed.insert(key_of(&link.calendar_id, &link.event_id));
    }
    for moved in &plan.moved_rows {
        changed.insert(key_of(&moved.calendar_id, &moved.event_id));
    }

    let mut out = DaySkips::default();
    for listed in events {
        let event = &listed.event;
        let key = key_of(&event.key.calendar_id, &event.key.event_id);
        let skip = match &listed.inclusion {
            Inclusion::Omitted(reason) => {
                Some(SkipReason::omitted(reason, event, window))
            }
            Inclusion::Task | Inclusion::Context(_) => {
                let state = journal.get(&(
                    occurrence_key(&event.key.calendar_id, &event.key.event_id),
                    day_string.clone(),
                ));
                match state {
                    Some(state) if state.gone && !written.contains(&key) => {
                        Some(SkipReason::removed(state, window.day, is_today))
                    }
                    _ if changed.contains(&key) => continue,
                    _ => {
                        out.in_sync += 1;
                        continue;
                    }
                }
            }
        };
        if let Some(reason) = skip {
            out.skipped.push(SkippedEvent {
                time: time_label(event),
                title: event.title.clone(),
                calendar_id: event.key.calendar_id.clone(),
                event_id: event.key.event_id.clone(),
                reason,
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use chrono::{FixedOffset, TimeZone};

    use super::*;
    use crate::native::{
        config::{GcalSettings, GcalTentative},
        gcal::{classify::classify, config::GcalConfig},
    };

    fn day() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 10, 30).expect("day")
    }

    fn window() -> DayWindow {
        DayWindow::new(day(), Some(chrono_tz::America::New_York))
            .expect("window")
    }

    fn fixture() -> Vec<GcalEvent> {
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

    /// The skip reason the real classifier produces for one fixture event.
    fn skip_for(id: &str) -> SkipReason {
        let config = GcalConfig::from_settings(GcalSettings {
            tentative: Some(GcalTentative::Context),
            ..GcalSettings::default()
        })
        .expect("config");
        let window = window();
        let events = fixture();
        let event = events
            .iter()
            .find(|event| event.key.event_id == id)
            .expect("fixture event");
        match classify(event, &window, &config) {
            Inclusion::Omitted(reason) => {
                SkipReason::omitted(&reason, event, &window)
            }
            other => panic!("{id} was not omitted: {other:?}"),
        }
    }

    fn gone(reason: Option<&str>, hour: u32, minute: u32) -> OccurrenceState {
        OccurrenceState {
            gone: true,
            gone_at: FixedOffset::west_opt(4 * 3600)
                .expect("offset")
                .with_ymd_and_hms(2026, 10, 30, hour, minute, 0)
                .single(),
            gone_reason: reason.map(str::to_string),
            ..OccurrenceState::default()
        }
    }

    #[test]
    fn omissions_map_to_stable_codes_and_friendly_detail() {
        let declined = skip_for("declined");
        assert_eq!(declined.code, "declined");
        assert_eq!(declined.detail, "declined in Google Calendar");
        assert_eq!(declined.hint_text(), None);
        assert!(!declined.actionable());

        let reply = skip_for("reply");
        assert_eq!(reply.code, "awaiting_reply");
        assert_eq!(reply.detail, "awaiting your reply");
        assert_eq!(
            reply.hint_text().as_deref(),
            Some("RSVP in Google Calendar to add it")
        );
        assert!(reply.actionable());

        let focus = skip_for("focus");
        assert_eq!(focus.code, "event_type");
        assert_eq!(focus.detail, "Google focusTime events are not synced");
    }

    #[test]
    fn classifier_strings_without_a_dedicated_code_keep_their_words() {
        let config_event = fixture().remove(0);
        let reason = SkipReason::omitted("cancelled", &config_event, &window());
        assert_eq!(reason.code, "cancelled");
        assert_eq!(reason.detail, "cancelled in Google Calendar");
        for text in [
            "invalid all-day range",
            "end is before start",
            "invalid timed/all-day range",
        ] {
            let reason = SkipReason::omitted(text, &config_event, &window());
            assert_eq!(reason.code, "invalid_time", "{text}");
            assert_eq!(reason.detail, "Google sent an invalid time range");
        }
        let unknown =
            SkipReason::omitted("something new", &config_event, &window());
        assert_eq!(unknown.code, "omitted");
        assert_eq!(unknown.detail, "something new");
    }

    #[test]
    fn outside_day_names_the_owning_note_for_timed_events_only() {
        let timed: GcalEvent = GcalEvent::from_google(
            "primary",
            serde_json::json!({
                "id": "late",
                "summary": "Evening news",
                "status": "confirmed",
                "start": {"dateTime": "2026-10-29T23:30:00-04:00"},
                "end": {"dateTime": "2026-10-30T00:30:00-04:00"},
            }),
        )
        .expect("event");
        let reason = SkipReason::omitted(
            "timed events belong to their local start day",
            &timed,
            &window(),
        );
        assert_eq!(reason.code, "outside_day");
        assert_eq!(reason.detail, "belongs to Thu 2026-10-29's note");
        assert_eq!(
            reason.hint_text().as_deref(),
            Some("sync that day with bob gcal sync -D 2026-10-29")
        );

        let all_day = fixture()
            .into_iter()
            .find(|event| event.key.event_id == "holiday")
            .expect("holiday");
        let reason =
            SkipReason::omitted("does not cover this day", &all_day, &window());
        assert_eq!(reason.code, "outside_day");
        assert_eq!(reason.detail, "does not cover this day");
        assert_eq!(reason.hint_text(), None);
    }

    #[test]
    fn your_deletion_reads_with_its_time_and_the_restore_command() {
        let reason = SkipReason::removed(
            &gone(Some("line_removed"), 5, 28),
            day(),
            true,
        );
        assert_eq!(reason.code, "removed_by_you");
        assert_eq!(reason.detail, "removed by you at 05:28");
        assert_eq!(reason.vault_label(), "removed by you 05:28");
        assert_eq!(
            reason.hint_text().as_deref(),
            Some("restore with bob gcal sync -F")
        );
        assert!(reason.actionable());
        // A tombstone without a recorded reason is still your deletion.
        assert_eq!(
            SkipReason::removed(&gone(None, 5, 28), day(), true).code,
            "removed_by_you"
        );
        // Another day's restore command names the day.
        let other = SkipReason::removed(
            &gone(Some("line_removed"), 5, 28),
            day(),
            false,
        );
        assert_eq!(
            other.hint_text().as_deref(),
            Some("restore with bob gcal sync -D 2026-10-30 -F")
        );
    }

    #[test]
    fn lifecycle_tombstones_keep_googles_reason_text() {
        let reason = SkipReason::removed(
            &gone(Some("Declined in Google Calendar"), 5, 28),
            day(),
            true,
        );
        assert_eq!(reason.code, "removed_in_google");
        assert_eq!(reason.detail, "Declined in Google Calendar");
        assert_eq!(reason.vault_label(), "Declined in Google Calendar");
        assert_eq!(reason.hint_text(), None);
        assert!(!reason.actionable());
    }

    #[test]
    fn undated_removal_drops_the_time() {
        let reason = SkipReason::removed_by_you(None, day(), true);
        assert_eq!(reason.detail, "removed by you");
        assert_eq!(reason.vault_label(), "removed by you");
    }

    #[test]
    fn hint_renders_the_command_cyan_only_with_color() {
        let reason = SkipReason::removed_by_you(None, day(), true);
        let hint = reason.hint.as_ref().expect("hint");
        assert_eq!(
            hint.styled(&Styler::plain()),
            "restore with bob gcal sync -F"
        );
        let colored = hint.styled(&Styler::colored());
        assert!(colored.starts_with("restore with "), "{colored:?}");
        assert!(colored.contains("\u{1b}["), "{colored:?}");
        assert!(colored.contains("bob gcal sync -F"), "{colored:?}");
    }

    #[test]
    fn skipped_row_marks_actionable_skips_apart_from_by_design_ones() {
        let human = |text: &str| text.replace('\u{2013}', "-");
        let event = |reason| SkippedEvent {
            time: "1200-1230".to_string(),
            title: "Design review".to_string(),
            calendar_id: "primary".to_string(),
            event_id: "reply".to_string(),
            reason,
        };
        let plain = Styler::plain();
        let line =
            event(skip_for("reply")).human_line(&plain, &|time| human(time));
        assert_eq!(
            line,
            "  ⊘ skipped 1200-1230  Design review  awaiting your reply \
             · RSVP in Google Calendar to add it"
        );
        let colored = Styler::colored();
        let actionable =
            event(skip_for("reply")).human_line(&colored, &|t| t.to_string());
        let by_design = event(skip_for("declined"))
            .human_line(&colored, &|t| t.to_string());
        assert!(actionable.contains("\u{1b}[33;1m⊘"), "{actionable:?}");
        assert!(!by_design.contains("\u{1b}[33"), "{by_design:?}");
        assert!(by_design.contains("⊘"), "{by_design:?}");
    }
}
