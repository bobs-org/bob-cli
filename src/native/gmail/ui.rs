//! `bob gmail` UI helpers: errors, glyphs, and flushing.

use std::io::Write;

use serde_json::json;

use super::super::style::Styler;
use super::GmailError;

/// Print `bob gmail <cmd>: <message>` to stderr (a red `error` prefix on
/// a TTY) plus an optional dim `  hint: …` line. In JSON mode, print
/// the error document to stdout instead. Returns the error's exit code.
pub(crate) fn report_error(cmd: &str, error: &GmailError, format: &str) -> i32 {
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
            "{}: bob gmail {cmd}: {}",
            styler.red("error"),
            error.message()
        );
    } else {
        eprintln!("bob gmail {cmd}: {}", error.message());
    }
    if let Some(hint) = error.hint() {
        eprintln!("{}", styler.dim(&format!("  hint: {hint}")));
    }
    error.exit_code()
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

/// Flush stdout; ignore a broken pipe.
pub(crate) fn flush_stdout() {
    let _ = std::io::stdout().flush();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glyphs_are_status_marks() {
        let styler = Styler::plain();
        assert_eq!(glyph(&styler, "ok"), "✓");
        assert_eq!(glyph(&styler, "warn"), "!");
        assert_eq!(glyph(&styler, "fail"), "✗");
        assert_eq!(glyph(&styler, "skip"), "·");
    }
}
