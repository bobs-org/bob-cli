//! Closed-session drill-down facts for the schema-1 `sessions` list.

use chrono::NaiveDate;

use super::{
    clock::StatsClock, index::PomodoroSession, report::SessionFact,
    series::format_minutes, value::DataQuality,
};
use crate::native::pomodoro_homes::SessionHomes;

/// One closed-session fact. `attributed` is the unfiltered R9 object with
/// real closed-session minutes; off-clock rows carry `off_clock`.
pub(crate) fn session_fact(
    date: NaiveDate,
    path: &str,
    session: &PomodoroSession,
    attributed: &SessionHomes,
) -> SessionFact {
    let minutes = session.counted_minutes();
    let quality = if minutes.is_some() {
        DataQuality::Ok
    } else {
        DataQuality::Missing
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
        off_clock: session.stats_clock() == Some(StatsClock::OffClock),
        homes: attributed.homes.clone(),
    }
}
