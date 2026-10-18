//! Closed-session drill-down facts for the schema-1 `sessions` list.

use chrono::NaiveDate;

use super::{
    index::PomodoroSession, report::SessionFact, series::format_minutes,
    value::DataQuality,
};
use crate::native::pomodoro_homes::SessionHomes;

/// One closed-session fact. `attributed` is the unfiltered R9 object.
pub(crate) fn session_fact(
    date: NaiveDate,
    path: &str,
    session: &PomodoroSession,
    attributed: &SessionHomes,
) -> SessionFact {
    let (minutes, quality) =
        match (session.recorded_minutes, session.range_minutes) {
            (Some(minutes), _) => (Some(minutes), DataQuality::Ok),
            (None, Some(minutes)) => (Some(minutes), DataQuality::Partial),
            (None, None) => (None, DataQuality::Missing),
        };
    let minutes_display = match minutes {
        Some(minutes) => format_minutes(i64::from(minutes)),
        None => "–".to_string(),
    };
    SessionFact {
        date,
        path: path.to_string(),
        line: session.line,
        time_range: session.time_range.clone(),
        minutes,
        minutes_display,
        quality,
        name: session.name.clone(),
        homes: attributed.homes.clone(),
    }
}
