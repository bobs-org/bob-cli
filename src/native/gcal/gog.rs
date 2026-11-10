//! Calendar API methods layered on the shared Google gog transport.

use std::ffi::OsString;

use serde::Deserialize;

use super::GcalError;
pub(crate) use crate::native::google::Gog;

use crate::native::google::parse_json;

pub(crate) const CLIENT: &str = crate::native::google::GCAL.client_bucket;
pub(crate) const API_CONSOLE_URL: &str =
    crate::native::google::GCAL.api_console_url;
pub(crate) use crate::native::google::REVOKE_URL;

/// One calendar from `gog calendar calendars --json`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub(crate) struct GogCalendar {
    pub id: String,
    #[serde(default)]
    pub summary: Option<String>,
    #[serde(default)]
    pub primary: bool,
    #[serde(default, rename = "accessRole")]
    pub access_role: Option<String>,
    #[serde(default, rename = "backgroundColor")]
    pub background_color: Option<String>,
    #[serde(default, rename = "colorId")]
    pub color_id: Option<String>,
}

impl GogCalendar {
    pub(crate) fn name(&self) -> &str {
        self.summary
            .as_deref()
            .filter(|name| !name.is_empty())
            .unwrap_or(&self.id)
    }

    pub(crate) fn colour(&self) -> Option<&str> {
        self.background_color
            .as_deref()
            .filter(|value| !value.is_empty())
            .or(self.color_id.as_deref())
    }

    /// `primary` in config matches this calendar's primary flag or id.
    pub(crate) fn matches_configured_id(&self, configured: &str) -> bool {
        self.id == configured
            || (configured == "primary"
                && (self.primary || self.id == "primary"))
    }
}

impl Gog {
    pub(crate) fn calendars(
        &self,
        account: &str,
    ) -> Result<Vec<GogCalendar>, GcalError> {
        let output = self.run(
            self.common_flags(Some(account))
                .into_iter()
                .chain([
                    OsString::from("calendar"),
                    OsString::from("calendars"),
                    OsString::from("--all"),
                ])
                .collect(),
        )?;
        let value = parse_json(&output.stdout)?;
        let calendars = value
            .get("calendars")
            .cloned()
            .unwrap_or(serde_json::Value::Array(Vec::new()));
        serde_json::from_value(calendars).map_err(|error| {
            GcalError::runtime(
                "bad_json",
                format!("parse gog calendar list: {error}"),
            )
        })
    }

    /// Fetch a complete event snapshot for one calendar and local-day window.
    pub(crate) fn events(
        &self,
        calendar_id: &str,
        from: &str,
        to: &str,
        account: &str,
    ) -> Result<Vec<serde_json::Value>, GcalError> {
        const FIELDS: &str = "items(id,iCalUID,recurringEventId,originalStartTime,status,summary,description,location,htmlLink,hangoutLink,conferenceData(entryPoints(entryPointType,uri)),start,end,transparency,eventType,colorId,attendees(self,responseStatus),organizer(self,displayName,email)),nextPageToken";
        let output = self.run(
            self.common_flags(Some(account))
                .into_iter()
                .chain([
                    OsString::from("calendar"),
                    OsString::from("events"),
                    OsString::from(calendar_id),
                    OsString::from("--from"),
                    OsString::from(from),
                    OsString::from("--to"),
                    OsString::from(to),
                    OsString::from("--all-pages"),
                    OsString::from("--max"),
                    OsString::from("250"),
                    OsString::from("--fields"),
                    OsString::from(FIELDS),
                ])
                .collect(),
        )?;
        let value = parse_json(&output.stdout)?;
        if value
            .get("nextPageToken")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|token| !token.is_empty())
        {
            return Err(GcalError::runtime(
                "incomplete_snapshot",
                format!("gog left another page for calendar {calendar_id}"),
            ));
        }
        let events = value
            .get("events")
            .or_else(|| value.get("items"))
            .cloned()
            .unwrap_or(serde_json::Value::Null);
        serde_json::from_value(events).map_err(|error| {
            GcalError::runtime(
                "bad_json",
                format!("parse gog events for {calendar_id}: {error}"),
            )
        })
    }

    /// Look up one event for lifecycle reconciliation.
    pub(crate) fn event(
        &self,
        calendar_id: &str,
        event_id: &str,
        account: &str,
    ) -> Result<serde_json::Value, GcalError> {
        let output = self.run(
            self.common_flags(Some(account))
                .into_iter()
                .chain([
                    OsString::from("calendar"),
                    OsString::from("event"),
                    OsString::from(calendar_id),
                    OsString::from(event_id),
                ])
                .collect(),
        )?;
        let value = parse_json(&output.stdout)?;
        value.get("event").cloned().ok_or_else(|| {
            GcalError::runtime(
                "bad_json",
                format!("gog returned no event for {calendar_id}/{event_id}"),
            )
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn calendar_primary_matches_configured_id() {
        let calendar = GogCalendar {
            id: "you@example.com".to_string(),
            summary: Some("Personal".to_string()),
            primary: true,
            access_role: Some("owner".to_string()),
            background_color: None,
            color_id: None,
        };
        assert!(calendar.matches_configured_id("primary"));
        assert!(calendar.matches_configured_id("you@example.com"));
        assert!(!calendar.matches_configured_id("work"));
    }
}
