//! Stderr step renderer for `bob gcal login`.
//!
//! Layout: `N/3`, two spaces, a 15-character label, then content (column
//! 20). Continuation lines indent to that column. Long URLs print
//! flush-left with a blank line before and after. In JSON mode, only
//! interactive pieces (instructions, the link, prompts) are shown.

use std::io::{self, BufRead, IsTerminal, Write};

use super::super::spinner::Spinner;
use super::super::style::Styler;
use super::{config::assume_tty, GcalError};

const CONTENT_COL: usize = 20;

/// Stderr transcript for the login ceremony.
pub(crate) struct LoginUi {
    styler: Styler,
    json: bool,
}

impl LoginUi {
    pub(crate) fn new(json: bool) -> Self {
        Self {
            styler: Styler::detect_stderr(),
            json,
        }
    }

    pub(crate) fn json(&self) -> bool {
        self.json
    }

    pub(crate) fn title(&self) {
        if self.json {
            return;
        }
        eprintln!(
            "{}",
            self.styler
                .cyan("Google Calendar · read-only · one-time setup")
        );
        eprintln!();
    }

    pub(crate) fn step_ok(&self, n: u8, label: &str, detail: &str) {
        self.step_line(n, label, &self.styler.green("✓"), detail);
    }

    #[allow(dead_code)]
    pub(crate) fn step_warn(&self, n: u8, label: &str, detail: &str) {
        self.step_line(n, label, &self.styler.yellow("!"), detail);
    }

    #[allow(dead_code)]
    pub(crate) fn step_fail(&self, n: u8, label: &str, detail: &str) {
        self.step_line(n, label, &self.styler.red("✗"), detail);
    }

    pub(crate) fn step_open(&self, n: u8, label: &str, detail: &str) {
        if self.json {
            return;
        }
        eprintln!("{}{detail}", step_prefix(n, label));
    }

    fn step_line(&self, n: u8, label: &str, glyph: &str, detail: &str) {
        if self.json {
            return;
        }
        eprintln!("{}{glyph} {detail}", step_prefix(n, label));
    }

    /// A continuation line, indented to the content column.
    pub(crate) fn note(&self, text: &str) {
        eprintln!("{:content$}{text}", "", content = CONTENT_COL);
    }

    /// A dim continuation line.
    pub(crate) fn dim_note(&self, text: &str) {
        eprintln!(
            "{:content$}{}",
            "",
            self.styler.dim(text),
            content = CONTENT_COL
        );
    }

    /// A flush-left URL with blank lines around it, so a wrapped copy
    /// stays intact.
    pub(crate) fn link(&self, url: &str) {
        eprintln!();
        eprintln!("{url}");
        eprintln!();
    }

    /// Spinner while waiting for the browser callback. No-op in JSON
    /// mode or when `NO_COLOR` is set (no `\r` redraws).
    pub(crate) fn wait(&self, label: &str) -> Option<Spinner> {
        if self.json || crate::native::env::var_os("NO_COLOR").is_some() {
            return None;
        }
        Some(Spinner::start(label))
    }

    /// Prompt on stderr. `Ok(None)` on EOF.
    pub(crate) fn prompt(
        &self,
        text: &str,
    ) -> Result<Option<String>, GcalError> {
        eprint!("{:content$}{text}", "", content = CONTENT_COL);
        let _ = io::stderr().flush();
        let echo_on_stderr = io::stdin().is_terminal();
        let mut line = String::new();
        match io::stdin().lock().read_line(&mut line) {
            Ok(0) => {
                if !echo_on_stderr {
                    eprintln!();
                }
                Ok(None)
            }
            Ok(_) => {
                if !echo_on_stderr {
                    eprintln!();
                }
                Ok(Some(line))
            }
            Err(error) => Err(GcalError::setup(
                "needs_tty",
                format!("read a prompt: {error}"),
            )
            .with_hint("run `bob gcal login` in a terminal")),
        }
    }

    pub(crate) fn summary(&self, account: &str) {
        if self.json {
            return;
        }
        eprintln!();
        eprintln!(
            "{} Connected. Bob can read, never change, {account}'s calendars.",
            self.styler.green("✓")
        );
        eprintln!();
        eprintln!("  Next  bob gcal sync -d     preview today's Events");
        eprintln!(
            "        bob gcal calendars   choose which calendars to sync"
        );
        eprintln!(
            "  Undo  bob gcal logout      removes only Bob's Calendar grant"
        );
    }
}

fn step_prefix(n: u8, label: &str) -> String {
    format!("{n}/3  {label:<15}")
}

/// Stdin is a terminal, or tests set `BOB_GCAL_ASSUME_TTY`.
pub(crate) fn is_tty() -> bool {
    assume_tty() || io::stdin().is_terminal()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn step_prefix_pads_label_to_column_twenty() {
        let prefix = step_prefix(1, "Google client");
        assert_eq!(prefix.len(), CONTENT_COL);
        assert!(prefix.starts_with("1/3  Google client"));
        let prefix = step_prefix(2, "Sign in");
        assert_eq!(prefix.len(), CONTENT_COL);
    }
}
