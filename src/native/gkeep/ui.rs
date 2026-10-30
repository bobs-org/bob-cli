//! `bob gkeep` UI helpers: ages, glyphs, status colors, errors.
//!
//! Status glyphs (`✓ ! ✗ ·`) are always shown: the epic plan pins those
//! literal outputs, piped-output tests expect them, and a doctor row
//! without a glyph would lose its status.

use serde_json::json;

use super::super::style::Styler;
use crate::native::gkeep::GkeepError;

pub(crate) use crate::native::spinner::Spinner;

/// Convert local wall-clock `NaiveDateTime` (from
/// `bob_env::current_datetime()`) to UTC.
///
/// Outside UTC, `.and_utc()` treats local time as UTC and shifts every
/// Keep age by the UTC offset. `Local.from_local_datetime` interprets it
/// in the local zone first.
pub(crate) fn local_naive_to_utc(
    naive: &chrono::NaiveDateTime,
) -> chrono::DateTime<chrono::Utc> {
    use chrono::{Local, TimeZone};
    if let Some(local) = Local.from_local_datetime(naive).earliest() {
        return local.with_timezone(&chrono::Utc);
    }
    // A wall-clock time that never existed (spring-forward DST gap):
    // retry one hour later before falling back to treating it as UTC.
    let shifted = *naive + chrono::Duration::hours(1);
    if let Some(local) = Local.from_local_datetime(&shifted).earliest() {
        return local.with_timezone(&chrono::Utc);
    }
    naive.and_utc()
}

/// The current moment in UTC, converted from local wall-clock time.
pub(crate) fn now_utc() -> chrono::DateTime<chrono::Utc> {
    local_naive_to_utc(&crate::native::env::current_datetime())
}

/// Format the age between two unix timestamps: `now`, `12m`, `3h`, `2d`,
/// `5w`, `4mo`, `2y` (minutes `m`, months `mo`).
pub(crate) fn format_age(now: i64, then: i64) -> String {
    let secs = now.saturating_sub(then).max(0);
    let minutes = secs / 60;
    if minutes == 0 {
        return "now".to_string();
    }
    if minutes < 60 {
        return format!("{minutes}m");
    }
    let hours = minutes / 60;
    if hours < 24 {
        return format!("{hours}h");
    }
    let days = hours / 24;
    if days < 7 {
        return format!("{days}d");
    }
    if days < 30 {
        return format!("{}w", days / 7);
    }
    if days < 365 {
        return format!("{}mo", days / 30);
    }
    format!("{}y", days / 365)
}

/// Paint a task status symbol: `[x]` green, `[/]` and `[?]` yellow,
/// `[-]` red, everything else plain.
pub(crate) fn paint_status_symbol(styler: &Styler, symbol: char) -> String {
    let text = format!("[{symbol}]");
    match symbol {
        'x' => styler.green(&text),
        '/' | '?' => styler.yellow(&text),
        '-' => styler.red(&text),
        _ => text,
    }
}

/// Print `bob gkeep <cmd>: <message>` to stderr (a red `error` prefix on
/// a TTY) plus an optional dim `  hint: …` line. In JSON mode, print
/// `{"schema_version":1,"ok":false,"error":{"kind":…,"message":…,
/// "hint":…}}` to stdout instead. Returns the error's exit code.
pub(crate) fn report_error(cmd: &str, error: &GkeepError, format: &str) -> i32 {
    if format == "json" {
        println!(
            "{}",
            json!({
                "schema_version": 1,
                "ok": false,
                "error": {
                    "kind": error.kind(),
                    "message": error.message(),
                    "hint": error.hint(),
                }
            })
        );
        return error.exit_code();
    }
    let styler = Styler::detect();
    if styler.is_color() {
        eprintln!(
            "{}: bob gkeep {cmd}: {}",
            styler.red("error"),
            error.message()
        );
    } else {
        eprintln!("bob gkeep {cmd}: {}", error.message());
    }
    if let Some(hint) = error.hint() {
        eprintln!("{}", styler.dim(&format!("  hint: {hint}")));
    }
    error.exit_code()
}

/// Print a `bob gkeep: warning: <message>` line to stderr.
pub(crate) fn warn(message: &str) {
    let styler = Styler::detect();
    let prefix = if styler.is_color() {
        styler.yellow("warning")
    } else {
        "warning".to_string()
    };
    eprintln!("bob gkeep: {prefix}: {message}");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn age_buckets() {
        assert_eq!(format_age(1_000, 1_000), "now");
        assert_eq!(format_age(1_000, 941), "now");
        assert_eq!(format_age(1_000_000, 1_000_000 - 12 * 60), "12m");
        assert_eq!(format_age(1_000_000, 1_000_000 - 3 * 3600), "3h");
        assert_eq!(format_age(10_000_000, 10_000_000 - 2 * 86400), "2d");
        assert_eq!(format_age(100_000_000, 100_000_000 - 21 * 86400), "3w");
        assert_eq!(format_age(100_000_000, 100_000_000 - 120 * 86400), "4mo");
        assert_eq!(format_age(200_000_000, 200_000_000 - 800 * 86400), "2y");
    }
}
