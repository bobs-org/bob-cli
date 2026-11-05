//! Stderr step renderer for service-specific Google login ceremonies.
//!
//! Layout: `N/3`, two spaces, a 15-character label, then content (column
//! 20). Continuation lines indent to that column. Long URLs print
//! flush-left with a blank line before and after. In JSON mode, only
//! interactive pieces (instructions, the link, prompts) are shown.

use std::io::{self, BufRead, IsTerminal, Write};

use super::super::spinner::Spinner;
use super::super::style::Styler;
use super::{GoogleError, GoogleService};

const CONTENT_COL: usize = 20;

/// Stderr transcript for the login ceremony.
pub(crate) struct LoginUi {
    styler: Styler,
    json: bool,
    service: GoogleService,
}

impl LoginUi {
    pub(crate) fn new(service: GoogleService, json: bool) -> Self {
        Self {
            styler: Styler::detect_stderr(),
            json,
            service,
        }
    }

    pub(crate) fn service(&self) -> GoogleService {
        self.service
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
            self.styler.cyan(&format!(
                "{} · read-only · one-time setup",
                self.service.product_name
            ))
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
        if self.json {
            return;
        }
        self.note_line(text);
    }

    /// A line needed to complete an interactive ceremony; shown in JSON mode.
    pub(crate) fn note_interactive(&self, text: &str) {
        self.note_line(text);
    }

    fn note_line(&self, text: &str) {
        eprintln!("{:content$}{text}", "", content = CONTENT_COL);
    }

    /// A dim continuation line.
    pub(crate) fn dim_note(&self, text: &str) {
        if self.json {
            return;
        }
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
    ) -> Result<Option<String>, GoogleError> {
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
            Err(error) => Err(GoogleError::setup(
                "needs_tty",
                format!("read a prompt: {error}"),
            )
            .with_hint(&format!(
                "run `{} login` in a terminal",
                self.service.command
            ))),
        }
    }

    pub(crate) fn summary(&self, account: &str) {
        if self.json {
            return;
        }
        eprintln!();
        if self.service.client_bucket == "bob-gcal" {
            eprintln!(
                "{} Connected. Bob can read, never change, {account}'s calendars.",
                self.styler.green("✓")
            );
            eprintln!();
            eprintln!("  Next  bob gcal sync -d     preview today's Events");
            eprintln!(
                "        bob gcal calendars   choose which calendars to sync"
            );
            eprintln!("  Undo  bob gcal logout      removes only Bob's Calendar grant");
        } else {
            eprintln!(
                "{} Connected. Bob can read, never change, {account}'s {}.",
                self.styler.green("✓"),
                self.service.product_name
            );
        }
    }

    pub(crate) fn writer_note(&self, writer: &str) {
        if self.json {
            return;
        }
        eprintln!(
            "  Note  sync runs on {writer} ({}.writer); run `{} login` there too.",
            self.service.command.strip_prefix("bob ").unwrap_or(self.service.command),
            self.service.command
        );
    }

    pub(crate) fn account_tip(&self, account: &str) {
        if self.json {
            return;
        }
        eprintln!();
        eprintln!("  Tip   add this to ~/.config/bob/config.yml:");
        eprintln!(
            "        {}:",
            self.service
                .command
                .strip_prefix("bob ")
                .unwrap_or(self.service.command)
        );
        eprintln!("          account: {account}");
    }

    pub(crate) fn testing_tip(&self) {
        if self.json {
            return;
        }
        eprintln!(
            "  Tip   if sign-in expires again after about 7 days, publish the consent screen at"
        );
        eprintln!("        https://console.cloud.google.com/auth/audience");
    }

    pub(crate) fn is_tty(&self) -> bool {
        is_tty(self.service)
    }
}

fn step_prefix(n: u8, label: &str) -> String {
    format!("{n}/3  {label:<15}")
}

/// Stdin is a terminal, or tests set the service-specific assume-TTY variable.
pub(crate) fn is_tty(service: GoogleService) -> bool {
    crate::native::env::var_os(service.assume_tty_env)
        .is_some_and(|value| matches!(value.to_str(), Some("1") | Some("true")))
        || io::stdin().is_terminal()
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
