//! Stacked human reports and `status -j` JSON for `bob completion`.
//!
//! Reports are stacked per-shell blocks, never a wide table, so narrow
//! terminals never shred columns. Color flows through [`Styler`]: glyphs
//! are `✓` green, `·` dim, `→` cyan, `⚠` yellow, and `✗` red, and plain
//! text when piped or under `NO_COLOR`. Home directories render as `~`.

use std::path::Path;

use super::cli::Shell;
use crate::native::env;
use crate::native::style::Styler;

/// File state behind one shell's report row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum State {
    NotInstalled,
    Current,
    Outdated,
    Edited,
    Foreign,
    ExternallyManaged,
    Missing,
}

impl State {
    pub(crate) fn text(self) -> &'static str {
        match self {
            State::NotInstalled => "not installed",
            State::Current => "current",
            State::Outdated => "outdated",
            State::Edited => "edited",
            State::Foreign => "foreign",
            State::ExternallyManaged => "current (externally managed)",
            State::Missing => "missing",
        }
    }
}

/// Where the target directory came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TargetReason {
    ExplicitTarget,
    PreviousInstall,
    Fpath,
    OhMyZsh,
    HomeDefault,
    BashDefault,
}

impl TargetReason {
    pub(crate) fn text(self) -> &'static str {
        match self {
            TargetReason::ExplicitTarget => "--target",
            TargetReason::PreviousInstall => "previous install",
            TargetReason::Fpath => "first writable fpath entry",
            TargetReason::OhMyZsh => "oh-my-zsh completions",
            TargetReason::HomeDefault => "~/.zfunc default",
            TargetReason::BashDefault => "bash-completion user dir",
        }
    }
}

/// Row severity: which glyph it gets and whether `--quiet` keeps it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Level {
    Ok,
    Info,
    Plan,
    Warn,
    Fail,
}

impl Level {
    fn glyph(self, styler: Styler) -> String {
        match self {
            Level::Ok => styler.green("✓"),
            Level::Info => styler.dim("·"),
            Level::Plan => styler.cyan("→"),
            Level::Warn => styler.yellow("⚠"),
            Level::Fail => styler.red("✗"),
        }
    }
}

/// One shell's report block plus its JSON entry.
pub(crate) struct ShellRow {
    pub(crate) shell: Shell,
    pub(crate) level: Level,
    pub(crate) summary: String,
    pub(crate) path: Option<String>,
    pub(crate) notes: Vec<String>,
    pub(crate) entry: serde_json::Value,
    /// Counts toward a failing exit code.
    pub(crate) failed: bool,
}

/// A full command report.
pub(crate) struct Report {
    pub(crate) dry_run: bool,
    pub(crate) rows: Vec<ShellRow>,
    pub(crate) warnings: Vec<String>,
    pub(crate) closers: Vec<String>,
}

impl Report {
    fn header(&self) -> String {
        if self.dry_run {
            "🐚  Shell completion · dry run — nothing is written".to_string()
        } else {
            format!(
                "🐚  Shell completion · bob {} · {}",
                env!("CARGO_PKG_VERSION"),
                running_display()
            )
        }
    }

    pub(crate) fn render_human(&self, styler: Styler, quiet: bool) -> String {
        let mut out = String::new();
        if !(quiet && self.rows.iter().all(|row| row.level == Level::Ok)) {
            out.push_str(&self.header());
            out.push('\n');
        }
        let mut first = out.is_empty();
        for row in &self.rows {
            if quiet
                && matches!(row.level, Level::Ok | Level::Info | Level::Plan)
            {
                continue;
            }
            if !first {
                out.push('\n');
            }
            first = false;
            out.push_str(&format!(
                "  {}  {:<6} {}",
                row.level.glyph(styler),
                row.shell.name(),
                row.summary
            ));
            out.push('\n');
            if let Some(path) = &row.path {
                out.push_str(&format!("            {path}\n"));
            }
            for note in &row.notes {
                for line in note.lines() {
                    out.push_str(&format!("            {line}\n"));
                }
            }
        }
        for warning in &self.warnings {
            if !first {
                out.push('\n');
            }
            first = false;
            let mut lines = warning.lines();
            if let Some(head) = lines.next() {
                out.push_str(&format!("  {}  {head}\n", styler.yellow("⚠")));
            }
            for line in lines {
                out.push_str(&format!("       {line}\n"));
            }
        }
        if !quiet {
            for closer in &self.closers {
                if !first {
                    out.push('\n');
                }
                first = false;
                out.push_str(&format!("  {closer}\n"));
            }
        }
        out
    }

    pub(crate) fn render_json(&self) -> String {
        let on_path = super::verify::bob_on_path()
            .map(|path| tilde(&path))
            .unwrap_or_default();
        let object = serde_json::json!({
            "schema_version": 1,
            "protocol": super::protocol::PROTOCOL,
            "bob": {
                "version": env!("CARGO_PKG_VERSION"),
                "current_exe": running_display(),
                "on_path": if on_path.is_empty() { serde_json::Value::Null } else { serde_json::Value::String(on_path) },
            },
            "shells": self.rows.iter().map(|row| &row.entry).collect::<Vec<_>>(),
        });
        format!("{object}")
    }
}

fn running_display() -> String {
    std::env::current_exe()
        .map(|path| tilde(&path))
        .unwrap_or_else(|_| "bob".to_string())
}

/// Render `path` with a leading `~` for the home directory.
pub(crate) fn tilde(path: &Path) -> String {
    let home = env::home_dir();
    if path == home {
        return "~".to_string();
    }
    if let Ok(suffix) = path.strip_prefix(&home) {
        return format!("~/{}", suffix.display());
    }
    path.display().to_string()
}

/// The exact `fpath` line that registers `dir`, with `~` styling.
pub(crate) fn fpath_line(dir: &Path) -> String {
    format!("fpath=({} $fpath)", tilde(dir))
}

/// One `status -j` shell entry. The eight parameters mirror the fixed
/// JSON schema field for field.
#[allow(clippy::too_many_arguments)]
pub(crate) fn json_entry(
    shell: Shell,
    state: State,
    path: Option<&Path>,
    owned: bool,
    registration: &str,
    registration_checked: Option<&str>,
    target_reason: Option<TargetReason>,
    remedy: Option<String>,
) -> serde_json::Value {
    serde_json::json!({
        "shell": shell.name(),
        "state": state.text(),
        "path": path.map(tilde).unwrap_or_default(),
        "protocol": super::protocol::PROTOCOL,
        "owned": owned,
        "registration": registration,
        "registration_checked": registration_checked,
        "target_reason": target_reason.map(TargetReason::text),
        "remedy": remedy,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tilde_collapses_home() {
        let home = env::home_dir();
        assert_eq!(tilde(&home), "~");
        assert_eq!(tilde(&home.join(".zfunc").join("_bob")), "~/.zfunc/_bob");
        assert_eq!(tilde(Path::new("/tmp/x")), "/tmp/x");
    }
}
