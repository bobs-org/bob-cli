//! `bob gcal` UI helpers: glyphs, hue dots, type swatches, errors.

use std::io::Write;

use serde_json::json;

use super::super::style::Styler;
use super::GcalError;
use crate::native::pomodoro_homes::Hue;
use crate::native::pomodoro_themes::ThemeRef;

/// Print `bob gcal <cmd>: <message>` to stderr (a red `error` prefix on
/// a TTY) plus an optional dim `  hint: …` line. In JSON mode, print
/// the error document to stdout instead. Returns the error's exit code.
pub(crate) fn report_error(cmd: &str, error: &GcalError, format: &str) -> i32 {
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
            "{}: bob gcal {cmd}: {}",
            styler.red("error"),
            error.message()
        );
    } else {
        eprintln!("bob gcal {cmd}: {}", error.message());
    }
    if let Some(hint) = error.hint() {
        eprintln!("{}", styler.dim(&format!("  hint: {hint}")));
    }
    error.exit_code()
}

/// Print a `bob gcal: warning: <message>` line to stderr.
#[allow(dead_code)]
pub(crate) fn warn(message: &str) {
    let styler = Styler::detect();
    let prefix = if styler.is_color() {
        styler.yellow("warning")
    } else {
        "warning".to_string()
    };
    eprintln!("bob gcal: {prefix}: {message}");
}

/// A status glyph: `✓ ! ✗ ·`.
pub(crate) fn glyph(styler: &Styler, status: &str) -> String {
    match status {
        "ok" => styler.green("✓"),
        "warn" => styler.yellow("!"),
        "fail" => styler.red("✗"),
        _ => styler.dim("·"),
    }
}

/// R8 area hue as a coloured `●` plus the note label.
pub(crate) fn hue_dot(
    styler: &Styler,
    hue: Option<Hue>,
    label: &str,
) -> String {
    match hue {
        Some(hue) if styler.is_color() => {
            format!(
                "{} {label}",
                styler.paint(&format!("38;5;{}", hue.xterm()), "●")
            )
        }
        Some(_) | None => format!("● {label}"),
    }
}

/// R7 type swatch plus the type name.
pub(crate) fn type_swatch(styler: &Styler, theme: &ThemeRef) -> String {
    if !styler.is_color() {
        return theme.name.clone();
    }
    match theme.hue {
        Some(hue) => format!(
            "{} {}",
            styler.paint(&format!("38;5;{}", hue.xterm()), "■"),
            theme.name
        ),
        None => format!("{} {}", styler.dim("□"), theme.name),
    }
}

/// Flush stdout; ignore a broken pipe.
pub(crate) fn flush_stdout() {
    let _ = std::io::stdout().flush();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native::pomodoro_themes::HueSource;

    #[test]
    fn glyphs_are_status_marks() {
        let styler = Styler::plain();
        assert_eq!(glyph(&styler, "ok"), "✓");
        assert_eq!(glyph(&styler, "warn"), "!");
        assert_eq!(glyph(&styler, "fail"), "✗");
        assert_eq!(glyph(&styler, "skip"), "·");
    }

    #[test]
    fn hue_dot_stays_plain_without_color() {
        let styler = Styler::plain();
        assert_eq!(hue_dot(&styler, Some(Hue::Violet), "meet"), "● meet");
    }

    #[test]
    fn type_swatch_uses_name_without_color() {
        let styler = Styler::plain();
        let theme = ThemeRef {
            key: "meet".to_string(),
            name: "MEET".to_string(),
            hue: Some(Hue::Violet),
            hue_source: HueSource::Area,
            known: true,
            configured: None,
            start: None,
            end: None,
        };
        assert_eq!(type_swatch(&styler, &theme), "MEET");
    }
}
