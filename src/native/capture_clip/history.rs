//! Clipboard history provider boundary.
//!
//! A history provider returns newest-first [`HistoryEntry`] values. The
//! agreement read (`clipboard::read_agreed_history`) polls the ordered
//! candidates selected here until one agrees with the live clipboard.
//! Selection itself is a pure, platform-neutral function so the full
//! precedence matrix is unit-tested on Linux; only the dispatch into the
//! SQLite adapters is cfg-gated.

use std::{
    path::{Path, PathBuf},
    time::SystemTime,
};

/// One clipboard history row, newest first.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct HistoryEntry {
    /// Decoded text for a text row; `None` for [`EntryKind::NotText`].
    pub(crate) text: Option<String>,
    pub(crate) kind: EntryKind,
    /// When the manager recorded the row, when known.
    pub(crate) at: Option<SystemTime>,
    /// Source application name reported by the manager, when known.
    pub(crate) app: Option<String>,
}

/// Whether a history row holds text or something counted capture refuses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum EntryKind {
    Text,
    /// A file, image, or other non-text row. The detail names the row, for
    /// example `"file or image"`, and is surfaced in the indexed error.
    // Built by the SQLite adapters; read by the agreement phase.
    #[cfg_attr(not(any(target_os = "macos", test)), allow(dead_code))]
    NotText(String),
}

impl HistoryEntry {
    pub(crate) fn text(
        text: String,
        at: Option<SystemTime>,
        app: Option<String>,
    ) -> Self {
        Self {
            text: Some(text),
            kind: EntryKind::Text,
            at,
            app,
        }
    }

    // Built by the SQLite adapters; read by the agreement phase.
    #[cfg_attr(not(any(target_os = "macos", test)), allow(dead_code))]
    pub(crate) fn not_text(
        detail: String,
        at: Option<SystemTime>,
        app: Option<String>,
    ) -> Self {
        Self {
            text: None,
            kind: EntryKind::NotText(detail),
            at,
            app,
        }
    }
}

/// Why a provider read failed. `Missing` and `Unsupported` are permanent for
/// this invocation; `Busy` is retryable on the next poll.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ProviderError {
    /// The store file is absent.
    // Built by the SQLite adapters; retried by the agreement phase.
    #[cfg_attr(not(any(target_os = "macos", test)), allow(dead_code))]
    Missing(String),
    /// The store is locked or has a hot journal; retry on the next poll.
    // Built by the SQLite adapters; retried by the agreement phase.
    #[cfg_attr(not(any(target_os = "macos", test)), allow(dead_code))]
    Busy(String),
    /// The store schema is not one bob understands.
    // Built by the SQLite adapters; read by the agreement phase.
    #[cfg_attr(not(any(target_os = "macos", test)), allow(dead_code))]
    Unsupported(String),
    /// Any other failure: command errors, decode errors, platform refusals.
    Failed(String),
}

impl ProviderError {
    pub(crate) fn message(&self) -> &str {
        match self {
            Self::Missing(message)
            | Self::Busy(message)
            | Self::Unsupported(message)
            | Self::Failed(message) => message,
        }
    }
}

/// Where a command provider's argv came from, so errors name the right
/// setting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CommandOrigin {
    Env,
    Config,
}

/// One clipboard history source, newest first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum HistoryProvider {
    Command {
        argv: Vec<String>,
        origin: CommandOrigin,
    },
    Alfred(PathBuf),
    Clipy(PathBuf),
}

impl HistoryProvider {
    /// Read at most `count` entries, newest first.
    pub(crate) fn read(
        &self,
        count: usize,
    ) -> Result<Vec<HistoryEntry>, ProviderError> {
        match self {
            Self::Command { argv, origin } => {
                read_command_history(argv, *origin, count)
            }
            Self::Alfred(database) => {
                #[cfg(any(target_os = "macos", test))]
                {
                    super::alfred::read_alfred_history(database, count)
                }
                #[cfg(not(any(target_os = "macos", test)))]
                {
                    let _ = database;
                    Err(ProviderError::Failed(
                        "the Alfred history provider is available only on \
                         macOS"
                            .to_string(),
                    ))
                }
            }
            Self::Clipy(database) => {
                #[cfg(any(target_os = "macos", test))]
                {
                    super::clipy::read_clipy_history(database, count)
                }
                #[cfg(not(any(target_os = "macos", test)))]
                {
                    let _ = database;
                    Err(ProviderError::Failed(
                        "the Clipy history provider is available only on \
                         macOS"
                            .to_string(),
                    ))
                }
            }
        }
    }
}

/// Where the candidate list came from, for later diagnostics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SelectionSource {
    /// `BOB_CLIPBOARD_HISTORY_CMD` was set.
    EnvCommand,
    /// `capture.clipboard_history` in the config file selected the list.
    Config { setting: String },
    /// No env command and no config value: the `auto` default.
    DefaultAuto,
}

/// Ordered history candidates plus where the choice came from.
///
/// Precedence: a non-blank `BOB_CLIPBOARD_HISTORY_CMD` (today's
/// whitespace-split command) wins, then `capture.clipboard_history` in
/// bob's config file, then `auto`. `auto` prefers Alfred, then Clipy,
/// skipping any whose store file is missing. `command` without a config
/// argv list is a config error raised at parse time, so reaching here with
/// an empty list is an internal error.
pub(crate) fn select_history_providers(
    env_command: Option<&str>,
    setting: &crate::native::config::ClipboardHistorySetting,
    command_argv: &[String],
    is_macos: bool,
    home: &Path,
) -> Result<(Vec<HistoryProvider>, SelectionSource), String> {
    use crate::native::config::ClipboardHistorySetting;

    if let Some(command) = env_command
        && !command.trim().is_empty()
    {
        let argv = command
            .split_whitespace()
            .map(str::to_string)
            .collect::<Vec<_>>();
        return Ok((
            vec![HistoryProvider::Command {
                argv,
                origin: CommandOrigin::Env,
            }],
            SelectionSource::EnvCommand,
        ));
    }

    let source = |setting: ClipboardHistorySetting| SelectionSource::Config {
        setting: setting.as_str().to_string(),
    };
    match setting {
        ClipboardHistorySetting::Auto => {
            if !is_macos {
                return Err(auto_unavailable_error());
            }
            let mut candidates = Vec::new();
            let alfred = alfred_database(home);
            if alfred.is_file() {
                candidates.push(HistoryProvider::Alfred(alfred.clone()));
            }
            let clipy = clipy_database(home);
            if clipy.is_file() {
                candidates.push(HistoryProvider::Clipy(clipy.clone()));
            }
            if candidates.is_empty() {
                return Err(format!(
                    "no clipboard history provider is available: Alfred history was not found at {} and Clipy history was not found at {}; install and run Alfred or Clipy, set BOB_CLIPBOARD_HISTORY_CMD, or set capture.clipboard_history in the config file",
                    alfred.display(),
                    clipy.display()
                ));
            }
            Ok((candidates, SelectionSource::DefaultAuto))
        }
        ClipboardHistorySetting::Alfred => {
            if !is_macos {
                return Err(macos_only_error("alfred"));
            }
            Ok((
                vec![HistoryProvider::Alfred(alfred_database(home))],
                source(ClipboardHistorySetting::Alfred),
            ))
        }
        ClipboardHistorySetting::Clipy => {
            if !is_macos {
                return Err(macos_only_error("clipy"));
            }
            Ok((
                vec![HistoryProvider::Clipy(clipy_database(home))],
                source(ClipboardHistorySetting::Clipy),
            ))
        }
        ClipboardHistorySetting::Command => {
            if command_argv.is_empty() {
                return Err("capture.clipboard_history is `command` but \
                     capture.clipboard_history_command is missing; set it to \
                     a non-empty YAML list of non-empty argv strings"
                    .to_string());
            }
            Ok((
                vec![HistoryProvider::Command {
                    argv: command_argv.to_vec(),
                    origin: CommandOrigin::Config,
                }],
                source(ClipboardHistorySetting::Command),
            ))
        }
        ClipboardHistorySetting::Off => Err(
            "clipboard history is disabled (capture.clipboard_history is \
             `off`); `%c<N>` with N >= 2 refuses while `%c` still reads the \
             live clipboard"
                .to_string(),
        ),
    }
}

/// Clipboard history is unavailable without a provider. This message keeps
/// the existing contract: it names `BOB_CLIPBOARD_HISTORY_CMD` and the
/// JSON array, and now also points at `capture.clipboard_history_command`.
fn auto_unavailable_error() -> String {
    "clipboard history is unavailable on this platform; set \
     BOB_CLIPBOARD_HISTORY_CMD to a command that accepts the requested count \
     and prints a newest-first JSON array of clipboard strings, or set \
     capture.clipboard_history_command to a JSON array of argv strings with \
     the same contract"
        .to_string()
}

fn macos_only_error(provider: &str) -> String {
    format!(
        "the {provider} clipboard history provider is available only on \
         macOS; set capture.clipboard_history to another value or run on a \
         Mac"
    )
}

pub(crate) fn alfred_database(home: &Path) -> PathBuf {
    home.join("Library/Application Support/Alfred/Databases/clipboard.alfdb")
}

pub(crate) fn clipy_database(home: &Path) -> PathBuf {
    home.join("Library/Application Support/com.clipy-app.Clipy/sqlite.db")
}

fn read_command_history(
    argv: &[String],
    origin: CommandOrigin,
    count: usize,
) -> Result<Vec<HistoryEntry>, ProviderError> {
    let label = match origin {
        CommandOrigin::Env => "BOB_CLIPBOARD_HISTORY_CMD",
        CommandOrigin::Config => "capture.clipboard_history_command",
    };
    let (program, args) = argv.split_first().ok_or_else(|| {
        ProviderError::Failed(format!("{label} must name a command"))
    })?;
    let mut full_args = args
        .iter()
        .map(std::ffi::OsString::from)
        .collect::<Vec<_>>();
    full_args.push(std::ffi::OsString::from(count.to_string()));
    let output = super::clipboard::run_bounded_argv(program, full_args)
        .map_err(|error| {
            ProviderError::Failed(format!("run {label}: {error}"))
        })?;
    let stdout = super::clipboard::require_clipboard_success(output, label)
        .map_err(ProviderError::Failed)?;
    let values =
        serde_json::from_slice::<Vec<String>>(&stdout).map_err(|error| {
            ProviderError::Failed(format!(
                "{label} must print a UTF-8 JSON array of strings ordered \
                 newest first: {error}"
            ))
        })?;
    Ok(values
        .into_iter()
        .map(|text| HistoryEntry::text(text, None, None))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native::config::ClipboardHistorySetting;

    static SEQUENCE: std::sync::atomic::AtomicU64 =
        std::sync::atomic::AtomicU64::new(0);

    fn home_with(files: &[&str]) -> PathBuf {
        let sequence =
            SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "bob-history-select-{}-{sequence}",
            std::process::id(),
        ));
        std::fs::create_dir_all(&root).expect("home root");
        for file in files {
            let path = root.join(file);
            std::fs::create_dir_all(path.parent().expect("parent"))
                .expect("dirs");
            std::fs::write(&path, b"").expect("store");
        }
        root
    }

    #[test]
    fn env_command_beats_everything() {
        let home = home_with(&[
            "Library/Application Support/Alfred/Databases/clipboard.alfdb",
        ]);
        let (candidates, source) = select_history_providers(
            Some("  my-hist --flag "),
            &ClipboardHistorySetting::Alfred,
            &[],
            true,
            &home,
        )
        .expect("env wins");
        assert_eq!(source, SelectionSource::EnvCommand);
        assert_eq!(
            candidates,
            [HistoryProvider::Command {
                argv: vec!["my-hist".to_string(), "--flag".to_string()],
                origin: CommandOrigin::Env,
            }]
        );
        assert!(matches!(
            candidates[0],
            HistoryProvider::Command {
                origin: CommandOrigin::Env,
                ..
            }
        ));
        std::fs::remove_dir_all(&home).expect("cleanup");
    }

    #[test]
    fn blank_env_falls_through_to_config() {
        let home = home_with(&[]);
        let error = select_history_providers(
            Some("   "),
            &ClipboardHistorySetting::Off,
            &[],
            false,
            &home,
        )
        .expect_err("blank env is ignored");
        assert!(error.contains("`off`"), "{error}");
        std::fs::remove_dir_all(&home).expect("cleanup");
    }

    #[test]
    fn auto_prefers_alfred_then_clipy_and_skips_missing_stores() {
        let both = home_with(&[
            "Library/Application Support/Alfred/Databases/clipboard.alfdb",
            "Library/Application Support/com.clipy-app.Clipy/sqlite.db",
        ]);
        let (candidates, source) = select_history_providers(
            None,
            &ClipboardHistorySetting::Auto,
            &[],
            true,
            &both,
        )
        .expect("both present");
        assert_eq!(source, SelectionSource::DefaultAuto);
        assert_eq!(candidates.len(), 2);
        assert!(matches!(candidates[0], HistoryProvider::Alfred(_)));
        assert!(matches!(candidates[1], HistoryProvider::Clipy(_)));

        let clipy_only = home_with(&[
            "Library/Application Support/com.clipy-app.Clipy/sqlite.db",
        ]);
        let (candidates, _) = select_history_providers(
            None,
            &ClipboardHistorySetting::Auto,
            &[],
            true,
            &clipy_only,
        )
        .expect("clipy only");
        assert_eq!(candidates.len(), 1);
        assert!(matches!(candidates[0], HistoryProvider::Clipy(_)));

        let empty = home_with(&[]);
        let error = select_history_providers(
            None,
            &ClipboardHistorySetting::Auto,
            &[],
            true,
            &empty,
        )
        .expect_err("no store present");
        assert!(error.contains("clipboard.alfdb"), "{error}");
        assert!(error.contains("sqlite.db"), "{error}");
        assert!(error.contains("capture.clipboard_history"), "{error}");

        for home in [&both, &clipy_only, &empty] {
            std::fs::remove_dir_all(home).expect("cleanup");
        }
    }

    #[test]
    fn explicit_providers_are_mac_only() {
        let home = home_with(&[]);
        for setting in [
            ClipboardHistorySetting::Alfred,
            ClipboardHistorySetting::Clipy,
        ] {
            let error =
                select_history_providers(None, &setting, &[], false, &home)
                    .expect_err("macOS-only off macOS");
            assert!(error.contains("available only on macOS"), "{error}");
        }
        std::fs::remove_dir_all(&home).expect("cleanup");
    }

    #[test]
    fn off_refuses_and_command_needs_argv() {
        let home = home_with(&[]);
        let error = select_history_providers(
            None,
            &ClipboardHistorySetting::Off,
            &[],
            true,
            &home,
        )
        .expect_err("off refuses");
        assert!(error.contains("`off`"), "{error}");
        assert!(error.contains("%c"), "{error}");

        let error = select_history_providers(
            None,
            &ClipboardHistorySetting::Command,
            &[],
            true,
            &home,
        )
        .expect_err("command without argv");
        assert!(
            error.contains("capture.clipboard_history_command"),
            "{error}"
        );

        let (candidates, source) = select_history_providers(
            None,
            &ClipboardHistorySetting::Command,
            &["hist".to_string()],
            false,
            &home,
        )
        .expect("config command works off macOS");
        assert_eq!(
            source,
            SelectionSource::Config {
                setting: "command".to_string(),
            }
        );
        assert_eq!(
            candidates,
            [HistoryProvider::Command {
                argv: vec!["hist".to_string()],
                origin: CommandOrigin::Config,
            }]
        );
        std::fs::remove_dir_all(&home).expect("cleanup");
    }

    #[test]
    fn non_macos_auto_keeps_the_actionable_guidance() {
        let home = home_with(&[]);
        let error = select_history_providers(
            None,
            &ClipboardHistorySetting::Auto,
            &[],
            false,
            &home,
        )
        .expect_err("auto off macOS");
        assert!(error.contains("BOB_CLIPBOARD_HISTORY_CMD"), "{error}");
        assert!(error.contains("JSON array"), "{error}");
        assert!(
            error.contains("capture.clipboard_history_command"),
            "{error}"
        );
        std::fs::remove_dir_all(&home).expect("cleanup");
    }
}
