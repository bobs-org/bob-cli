//! `bob gkeep` UI helpers: ages, glyphs, status colors, errors.
//!
//! Status glyphs (`✓ ! ✗ ·`) are always shown: the epic plan pins those
//! literal outputs, piped-output tests expect them, and a doctor row
//! without a glyph would lose its status.

use std::{
    io::{IsTerminal, Write},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread::{self, JoinHandle},
    time::Duration,
};

use serde_json::json;

use super::super::style::Styler;
use crate::native::gkeep::GkeepError;

/// Braille progress frames drawn by [`Spinner`].
const SPINNER_FRAMES: &[&str] =
    &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

/// A stderr progress spinner, stopped by clearing the line on drop.
///
/// `Spinner::start` is a no-op returning an idle spinner when stderr is
/// not a TTY. Callers pass no label at all in JSON mode or with
/// `--quiet`, so construction itself stays unconditional here.
pub(crate) struct Spinner {
    running: Option<RunningSpinner>,
}

struct RunningSpinner {
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl Spinner {
    /// Start spinning `label` on stderr, unless stderr is not a TTY.
    pub(crate) fn start(label: &str) -> Self {
        if !std::io::stderr().is_terminal() {
            return Self { running: None };
        }
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = Arc::clone(&stop);
        let label = label.to_string();
        let handle = thread::spawn(move || {
            let mut frame = 0;
            while !thread_stop.load(Ordering::Relaxed) {
                let glyph = SPINNER_FRAMES[frame % SPINNER_FRAMES.len()];
                eprint!("\r{glyph} {label}…");
                let _ = std::io::stderr().flush();
                frame += 1;
                thread::sleep(Duration::from_millis(80));
            }
        });
        Self {
            running: Some(RunningSpinner {
                stop,
                handle: Some(handle),
            }),
        }
    }
}

impl Drop for Spinner {
    fn drop(&mut self) {
        if let Some(running) = self.running.as_mut() {
            running.stop.store(true, Ordering::Relaxed);
            if let Some(handle) = running.handle.take() {
                let _ = handle.join();
            }
            eprint!("\r\x1b[K");
            let _ = std::io::stderr().flush();
        }
    }
}

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
    Local
        .from_local_datetime(naive)
        .earliest()
        .map(|local| local.with_timezone(&chrono::Utc))
        .unwrap_or_else(|| naive.and_utc())
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
    fn spinner_frames_are_braille() {
        assert_eq!(SPINNER_FRAMES.len(), 10);
        assert_eq!(SPINNER_FRAMES[0], "⠋");
    }

    #[test]
    fn spinner_start_and_drop_never_panics() {
        let _spinner = Spinner::start("Syncing Google Keep");
    }
}
