//! `bob gkeep` UI helpers: ages, glyphs, status colors, errors.
//!
//! Glyphs show only when `Styler::is_color()`, the same rule as
//! `bob plugins`: piped output stays plain.

use serde_json::json;

use super::super::style::Styler;
use crate::native::gkeep::GkeepError;

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

/// A status glyph shown only on a color terminal, else `None`.
pub(crate) fn status_glyph(
    ok: Option<bool>,
    styler: Styler,
) -> Option<&'static str> {
    if !styler.is_color() {
        return None;
    }
    match ok {
        Some(true) => Some("✓"),
        Some(false) => Some("✗"),
        None => Some("·"),
    }
}

/// A warning glyph shown only on a color terminal, else `None`.
pub(crate) fn warning_glyph(styler: Styler) -> Option<&'static str> {
    styler.is_color().then_some("!")
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

/// Print a plain `bob gkeep <cmd>: warning: <message>` line to stderr.
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

    #[test]
    fn glyphs_hide_without_color() {
        let plain = Styler::plain();
        assert_eq!(status_glyph(Some(true), plain), None);
        assert_eq!(warning_glyph(plain), None);
    }
}
