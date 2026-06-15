use std::{
    env,
    io::{self, IsTerminal},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Styler {
    color: bool,
}

impl Styler {
    pub(crate) fn detect() -> Self {
        Self {
            color: io::stdout().is_terminal()
                && env::var_os("NO_COLOR").is_none(),
        }
    }

    #[cfg(test)]
    pub(crate) fn plain() -> Self {
        Self { color: false }
    }

    pub(crate) fn is_color(self) -> bool {
        self.color
    }

    pub(crate) fn separator(&self) -> &'static str {
        if self.color {
            "\u{b7}"
        } else {
            "-"
        }
    }

    pub(crate) fn cyan(&self, text: &str) -> String {
        self.paint("36;1", text)
    }

    pub(crate) fn green(&self, text: &str) -> String {
        self.paint("32;1", text)
    }

    pub(crate) fn yellow(&self, text: &str) -> String {
        self.paint("33;1", text)
    }

    pub(crate) fn blue(&self, text: &str) -> String {
        self.paint("34;1", text)
    }

    pub(crate) fn red(&self, text: &str) -> String {
        self.paint("31;1", text)
    }

    pub(crate) fn dim(&self, text: &str) -> String {
        self.paint("2", text)
    }

    pub(crate) fn paint(&self, code: &str, text: &str) -> String {
        if self.color {
            format!("\u{1b}[{code}m{text}\u{1b}[0m")
        } else {
            text.to_string()
        }
    }

    pub(crate) fn success_prefix(&self, dry_run: bool) -> String {
        let label = if dry_run { "[dry-run] ok" } else { "ok" };
        if self.color {
            self.green(label)
        } else {
            label.to_string()
        }
    }

    pub(crate) fn warning_prefix(&self) -> String {
        if self.color {
            self.yellow("warning")
        } else {
            "warning".to_string()
        }
    }
}

pub(crate) fn pad_right(text: &str, width: usize) -> String {
    let padding = width.saturating_sub(display_width(text));
    format!("{text}{}", " ".repeat(padding))
}

pub(crate) fn display_width(text: &str) -> usize {
    text.chars().count()
}
