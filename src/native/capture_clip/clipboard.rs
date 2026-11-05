use std::{
    io,
    process::{Command, Output},
    thread,
    time::{Duration, SystemTime},
};

use super::history::{
    select_history_providers, EntryKind, HistoryEntry, HistoryProvider,
    ProviderError, SelectionSource,
};
use crate::native::{config, env as bob_env};

const AGREEMENT_TIMEOUT: Duration = Duration::from_millis(1_500);
const AGREEMENT_POLL_INTERVAL: Duration = Duration::from_millis(150);
const FRESH_HISTORY_AGE: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ClipboardHistoryError {
    pub(crate) code: Option<&'static str>,
    pub(crate) message: String,
}

impl ClipboardHistoryError {
    fn io(message: String) -> Self {
        Self {
            code: None,
            message,
        }
    }

    fn coded(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code: Some(code),
            message: message.into(),
        }
    }
}

/// The accepted, single-provider history and the source rows needed to
/// report provenance in the next capture-output phase.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct AgreedClipboardHistory {
    pub(crate) values: Vec<String>,
    #[allow(dead_code)]
    pub(crate) provider: &'static str,
    #[allow(dead_code)]
    pub(crate) entries: Vec<HistoryEntry>,
}

pub(crate) fn read_clipboard() -> Result<String, String> {
    let output = clipboard_command_output()?;
    normalize_clipboard_output(output.stdout)
}

pub(crate) fn read_clipboard_history(
    count: usize,
) -> Result<AgreedClipboardHistory, ClipboardHistoryError> {
    let current = read_clipboard().map_err(ClipboardHistoryError::io)?;
    if count == 1 {
        return Ok(AgreedClipboardHistory {
            values: vec![current],
            provider: "live",
            entries: Vec::new(),
        });
    }

    let history = config::load_clipboard_history_config(&config::config_path())
        .map_err(|error| {
            let message = match error {
                config::ConfigError::Read(message)
                | config::ConfigError::Invalid(message) => message,
            };
            ClipboardHistoryError::coded(
                "clipboard_history_unavailable",
                message,
            )
        })?;
    let env_command = bob_env::var("BOB_CLIPBOARD_HISTORY_CMD").ok();
    let (candidates, source) = select_history_providers(
        env_command.as_deref(),
        &history.setting,
        &history.command,
        cfg!(target_os = "macos"),
        &bob_env::home_dir(),
    )
    .map_err(|message| {
        ClipboardHistoryError::coded("clipboard_history_unavailable", message)
    })?;

    read_agreed_history(
        current,
        &candidates,
        &source,
        count,
        |provider, requested| provider.read(requested),
        SystemTime::now,
        thread::sleep,
        provider_running,
    )
}

#[derive(Default)]
struct CandidateObservation {
    active: bool,
    entries: Option<Vec<HistoryEntry>>,
    last_error: Option<ProviderError>,
}

/// Poll the ordered candidates until one has a text head that agrees with the
/// live clipboard. The reader and clock hooks make timing and provider
/// behavior deterministic in unit tests.
pub(super) fn read_agreed_history<R, N, S, P>(
    current: String,
    candidates: &[HistoryProvider],
    source: &SelectionSource,
    count: usize,
    mut read: R,
    mut now: N,
    mut sleep: S,
    mut process_running: P,
) -> Result<AgreedClipboardHistory, ClipboardHistoryError>
where
    R: FnMut(
        &HistoryProvider,
        usize,
    ) -> Result<Vec<HistoryEntry>, ProviderError>,
    N: FnMut() -> SystemTime,
    S: FnMut(Duration),
    P: FnMut(&HistoryProvider) -> Option<bool>,
{
    let started = now();
    let deadline = started.checked_add(AGREEMENT_TIMEOUT).unwrap_or(started);
    let mut observations = candidates
        .iter()
        .map(|_| CandidateObservation {
            active: true,
            ..CandidateObservation::default()
        })
        .collect::<Vec<_>>();

    'poll: loop {
        for (index, provider) in candidates.iter().enumerate() {
            if now() >= deadline {
                break 'poll;
            }
            if !observations[index].active {
                continue;
            }
            match read(provider, count) {
                Ok(entries) => {
                    // A command may return more than requested. Never inspect
                    // or normalize surplus rows.
                    let entries =
                        entries.into_iter().take(count).collect::<Vec<_>>();
                    let matches = entries.first().is_some_and(|entry| {
                        normalize_history_entry(entry)
                            .is_ok_and(|head| head == current)
                    });
                    observations[index].entries = Some(entries.clone());
                    observations[index].last_error = None;
                    if matches {
                        if now() > deadline {
                            break 'poll;
                        }
                        return accepted_history(
                            current, provider, entries, count,
                        );
                    }
                }
                Err(ProviderError::Busy(message)) => {
                    observations[index].last_error =
                        Some(ProviderError::Busy(message));
                }
                Err(error)
                    if matches!(source, SelectionSource::DefaultAuto) =>
                {
                    observations[index].active = false;
                    observations[index].last_error = Some(error);
                }
                Err(error) => {
                    return Err(ClipboardHistoryError::coded(
                        "clipboard_history_unavailable",
                        format!(
                            "{}: {} history provider is unavailable: {}",
                            source_description(source),
                            provider_name(provider),
                            error.message()
                        ),
                    ));
                }
            }
        }
        let current_time = now();
        if current_time >= deadline {
            break;
        }
        if !observations.iter().any(|observation| observation.active) {
            let details = candidates
                .iter()
                .zip(&observations)
                .map(|(provider, observation)| {
                    format!(
                        "{}: {}",
                        provider_name(provider),
                        observation
                            .last_error
                            .as_ref()
                            .map(ProviderError::message)
                            .unwrap_or("unavailable")
                    )
                })
                .collect::<Vec<_>>()
                .join("; ");
            return Err(ClipboardHistoryError::coded(
                "clipboard_history_unavailable",
                format!(
                    "{}: no clipboard history provider is available ({details})",
                    source_description(source)
                ),
            ));
        }
        let remaining =
            deadline.duration_since(current_time).unwrap_or_default();
        sleep(AGREEMENT_POLL_INTERVAL.min(remaining));
    }

    let current_time = now();
    let diagnoses = candidates
        .iter()
        .zip(&observations)
        .map(|(provider, observation)| {
            format_diagnosis(
                provider,
                source,
                observation,
                current_time,
                &mut process_running,
            )
        })
        .collect::<Vec<_>>()
        .join("; ");
    Err(ClipboardHistoryError::coded(
        "clipboard_history_mismatch",
        format!(
            "clipboard history did not agree with the live clipboard within 1.5 seconds: {diagnoses}. `%c` still captures the live clipboard alone"
        ),
    ))
}

fn accepted_history(
    current: String,
    provider: &HistoryProvider,
    entries: Vec<HistoryEntry>,
    count: usize,
) -> Result<AgreedClipboardHistory, ClipboardHistoryError> {
    if entries.len() < count {
        return Err(ClipboardHistoryError::coded(
            "clipboard_history_short",
            format!(
                "{} clipboard history requested {count} entries but only {} are available",
                provider_name(provider),
                entries.len()
            ),
        ));
    }

    let mut values = Vec::with_capacity(count);
    values.push(current);
    for (index, entry) in entries.iter().take(count).enumerate().skip(1) {
        let entry_number = index + 1;
        let text = match &entry.kind {
            EntryKind::Text => entry.text.as_deref().ok_or_else(|| {
                ClipboardHistoryError::coded(
                    "clipboard_history_not_text",
                    format!(
                        "clipboard history entry {entry_number} has no stored text"
                    ),
                )
            })?,
            EntryKind::NotText(detail) => {
                return Err(ClipboardHistoryError::coded(
                    "clipboard_history_not_text",
                    format!(
                        "clipboard history entry {entry_number} is a file or image ({detail}); counted capture supports text history"
                    ),
                ));
            }
        };
        let normalized = normalize_clipboard_output(text.as_bytes().to_vec())
            .map_err(|error| {
            ClipboardHistoryError::coded(
                "clipboard_history_not_text",
                format!("clipboard history entry {entry_number}: {error}"),
            )
        })?;
        values.push(normalized);
    }

    Ok(AgreedClipboardHistory {
        values,
        provider: provider_provenance_name(provider),
        entries,
    })
}

fn normalize_history_entry(entry: &HistoryEntry) -> Result<String, String> {
    match &entry.kind {
        EntryKind::Text => {
            let text = entry.text.as_deref().ok_or_else(|| {
                "newest history entry has no stored text".to_string()
            })?;
            normalize_clipboard_output(text.as_bytes().to_vec())
        }
        EntryKind::NotText(_) => {
            Err("newest history entry is not text".to_string())
        }
    }
}

fn format_diagnosis(
    provider: &HistoryProvider,
    source: &SelectionSource,
    observation: &CandidateObservation,
    now: SystemTime,
    process_running: &mut impl FnMut(&HistoryProvider) -> Option<bool>,
) -> String {
    let mut parts = vec![format!(
        "{} {}",
        source_description(source),
        provider_name(provider)
    )];
    if let Some(error) = observation.last_error.as_ref()
        && observation.entries.is_none()
    {
        if matches!(error, ProviderError::Missing(_))
            && let Some(path) = provider_path(provider)
        {
            parts.push(format!("store not found at {}", path.display()));
        }
        parts.push(format!("history unavailable: {}", error.message()));
        if let Some(running) = process_running(provider) {
            parts.push(if running {
                "process is running".to_string()
            } else {
                "process is not running".to_string()
            });
        }
        return parts.join(" · ");
    }

    let newest = observation
        .entries
        .as_ref()
        .and_then(|entries| entries.first());
    if let Some(entry) = newest {
        if let Some(at) = entry.at {
            let age = now.duration_since(at).unwrap_or_default();
            parts.push(format!("newest entry {} old", format_age(age)));
        }
        if let Err(reason) = normalize_history_entry(entry) {
            parts.push(format!(
                "newest entry could not be normalized ({reason}) and doesn't match the live clipboard"
            ));
        } else {
            parts.push(
                "newest entry doesn't match the live clipboard".to_string(),
            );
        }
        let running = process_running(provider);
        if let Some(running) = running {
            parts.push(if running {
                "process is running".to_string()
            } else {
                "process is not running".to_string()
            });
        }
        if running == Some(true)
            && entry.at.is_some_and(|at| {
                now.duration_since(at).unwrap_or_default() <= FRESH_HISTORY_AGE
            })
        {
            parts.push(
                "the manager may skip password-manager, transient, or ignored-app copies"
                    .to_string(),
            );
        }
        if let Some(error) = observation.last_error.as_ref() {
            parts.push(format!("last read failed: {}", error.message()));
        }
    } else {
        parts.push("history has no newest entry".to_string());
        if let Some(error) = observation.last_error.as_ref() {
            parts.push(format!("last read failed: {}", error.message()));
        }
    }
    parts.join(" · ")
}

fn provider_name(provider: &HistoryProvider) -> &'static str {
    match provider {
        HistoryProvider::Command { .. } => "command",
        HistoryProvider::Alfred(_) => "Alfred",
        HistoryProvider::Clipy(_) => "Clipy",
    }
}

fn provider_provenance_name(provider: &HistoryProvider) -> &'static str {
    match provider {
        HistoryProvider::Command { .. } => "command",
        HistoryProvider::Alfred(_) => "alfred",
        HistoryProvider::Clipy(_) => "clipy",
    }
}

fn provider_path(provider: &HistoryProvider) -> Option<&std::path::Path> {
    match provider {
        HistoryProvider::Alfred(path) | HistoryProvider::Clipy(path) => {
            Some(path)
        }
        HistoryProvider::Command { .. } => None,
    }
}

fn provider_running(provider: &HistoryProvider) -> Option<bool> {
    let process = match provider {
        HistoryProvider::Alfred(_) => "Alfred",
        HistoryProvider::Clipy(_) => "Clipy",
        HistoryProvider::Command { .. } => return None,
    };
    let output = Command::new("pgrep").args(["-x", process]).output().ok()?;
    Some(output.status.success())
}

fn source_description(source: &SelectionSource) -> String {
    match source {
        SelectionSource::EnvCommand => "BOB_CLIPBOARD_HISTORY_CMD".to_string(),
        SelectionSource::Config { setting } => {
            format!("capture.clipboard_history: {setting}")
        }
        SelectionSource::DefaultAuto => {
            "capture.clipboard_history: auto".to_string()
        }
    }
}

fn format_age(age: Duration) -> String {
    let seconds = age.as_secs();
    if seconds < 60 {
        format!("{seconds} seconds")
    } else if seconds < 60 * 60 {
        format!("{} minutes", seconds / 60)
    } else if seconds < 24 * 60 * 60 {
        format!("{} hours", seconds / (60 * 60))
    } else {
        format!("{} days", seconds / (24 * 60 * 60))
    }
}

fn clipboard_command_output() -> Result<Output, String> {
    if let Some(command) = bob_env::var("BOB_CLIPBOARD_CMD")
        .ok()
        .filter(|value| !value.trim().is_empty())
    {
        let parts = command.split_whitespace().collect::<Vec<_>>();
        let (program, args) = parts.split_first().ok_or_else(|| {
            "BOB_CLIPBOARD_CMD must name a command".to_string()
        })?;
        return run_required_command(program, args, "BOB_CLIPBOARD_CMD");
    }

    #[cfg(target_os = "macos")]
    {
        run_required_command("pbpaste", &[], "pbpaste")
    }

    #[cfg(target_os = "linux")]
    {
        if bob_env::var_os("WAYLAND_DISPLAY").is_some() {
            return run_required_command(
                "wl-paste",
                &["--no-newline", "--type", "text"],
                "wl-paste",
            );
        }

        if bob_env::var_os("DISPLAY").is_some() {
            match run_command("xclip", &["-selection", "clipboard", "-o"]) {
                Ok(output) => return require_success(output, "xclip"),
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    return run_required_command(
                        "xsel",
                        &["--clipboard", "--output"],
                        "xsel (after xclip was not found)",
                    );
                }
                Err(error) => {
                    return Err(format!("run xclip: {error}"));
                }
            }
        }
    }

    #[cfg(not(target_os = "macos"))]
    if bob_env::var_os("TMUX").is_some() {
        return run_required_command(
            "tmux",
            &["show-buffer"],
            "tmux show-buffer",
        );
    }

    #[cfg(not(target_os = "macos"))]
    let tried = if cfg!(target_os = "macos") {
        "pbpaste and tmux"
    } else if cfg!(target_os = "linux") {
        "wl-paste, xclip/xsel, and tmux"
    } else {
        "tmux"
    };
    #[cfg(not(target_os = "macos"))]
    Err(format!(
        "no clipboard source is available (tried {tried}); set \
BOB_CLIPBOARD_CMD to a command that prints clipboard text"
    ))
}

fn run_required_command(
    program: &str,
    args: &[&str],
    label: &str,
) -> Result<Output, String> {
    let output = run_command(program, args)
        .map_err(|error| format!("run {label}: {error}"))?;
    require_success(output, label)
}

fn run_command(program: &str, args: &[&str]) -> io::Result<Output> {
    Command::new(program).args(args).output()
}

fn require_success(output: Output, label: &str) -> Result<Output, String> {
    if output.status.success() {
        return Ok(output);
    }

    let stderr = String::from_utf8_lossy(&output.stderr);
    let detail = stderr.trim();
    let suffix = if detail.is_empty() {
        String::new()
    } else {
        format!(": {detail}")
    };
    Err(format!(
        "clipboard command {label} exited with {}{suffix}",
        output
            .status
            .code()
            .map_or_else(|| "a signal".to_string(), |code| code.to_string())
    ))
}

pub(super) fn normalize_clipboard_output(
    bytes: Vec<u8>,
) -> Result<String, String> {
    if bytes.contains(&0) {
        return Err(
            "clipboard contains binary data (embedded NUL); copy a file path \
when attaching binary content"
                .to_string(),
        );
    }
    let text = String::from_utf8(bytes).map_err(|_| {
        "clipboard is not valid UTF-8; copy a file path when attaching binary \
content"
            .to_string()
    })?;
    let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
    let mut lines = normalized.split('\n').collect::<Vec<_>>();
    while lines.last().is_some_and(|line| line.trim().is_empty()) {
        lines.pop();
    }
    let normalized = lines.join("\n");
    if normalized.trim().is_empty() {
        return Err("clipboard is empty".to_string());
    }
    Ok(normalized)
}
