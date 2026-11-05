use std::{
    ffi::OsString,
    io,
    path::PathBuf,
    process::Command,
    thread,
    time::{Duration, SystemTime},
};

use super::history::{
    select_history_providers, EntryKind, HistoryEntry, HistoryProvider,
    ProviderError, SelectionSource,
};
use crate::native::{
    config, env as bob_env,
    subprocess_runner::{self, RawRunOutput, RawRunSpec},
};

const AGREEMENT_TIMEOUT: Duration = Duration::from_millis(1_500);
const AGREEMENT_POLL_INTERVAL: Duration = Duration::from_millis(150);
const FRESH_HISTORY_AGE: Duration = Duration::from_secs(60);

/// Every clipboard subprocess (live reads and history commands) shares
/// this deadline, stdout cap, and stderr cap.
pub(super) const CLIPBOARD_TIMEOUT: Duration = Duration::from_secs(5);
pub(super) const CLIPBOARD_STDOUT_CAP: usize = 16 * 1024 * 1024;
pub(super) const CLIPBOARD_STDERR_CAP: usize = 64 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ClipboardHistoryError {
    pub(crate) code: Option<&'static str>,
    pub(crate) message: String,
}

impl ClipboardHistoryError {
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

/// Content-free diagnostics for the read-only `bob clipboard` report.
#[derive(Debug)]
pub(crate) struct ClipboardStatusReport {
    pub(crate) count: usize,
    pub(crate) live_source: String,
    pub(crate) selection: String,
    pub(crate) selection_source: String,
    pub(crate) config_path: Option<String>,
    pub(crate) providers: Vec<ClipboardProviderStatus>,
    pub(crate) result: Result<AgreedClipboardHistory, ClipboardHistoryError>,
}

#[derive(Debug)]
pub(crate) struct ClipboardProviderStatus {
    pub(crate) name: &'static str,
    pub(crate) selected: bool,
    pub(crate) store_path: Option<String>,
    pub(crate) exists: Option<bool>,
    pub(crate) schema_ok: Option<bool>,
    pub(crate) running: Option<bool>,
    pub(crate) newest_entry_age_seconds: Option<u64>,
    pub(crate) head_agrees_with_live: bool,
    pub(crate) error: Option<String>,
}

pub(crate) fn read_clipboard() -> Result<String, String> {
    let bytes = clipboard_command_output()?;
    normalize_clipboard_output(bytes)
}

// Shared by the status-command phase: the single-shot entry that reads
// the live value itself before running agreement.
#[allow(dead_code)]
pub(crate) fn read_clipboard_history(
    count: usize,
) -> Result<AgreedClipboardHistory, ClipboardHistoryError> {
    let current =
        read_clipboard().map_err(|message| ClipboardHistoryError {
            code: None,
            message,
        })?;
    read_clipboard_history_with_current(current, count)
}

/// Inspect the exact configured history path used by `%c<N>`, then add a
/// one-row diagnostic read for every history provider known on this host.
/// The returned report deliberately contains only booleans, paths, ages,
/// and errors; clipboard bytes and history text never leave this module.
pub(crate) fn clipboard_status(count: usize) -> ClipboardStatusReport {
    let live_source = live_source_name();
    let path = config::config_path();
    let config_result = config::load_clipboard_history_config(&path);
    let env_command = bob_env::var("BOB_CLIPBOARD_HISTORY_CMD").ok();

    let (selection_source, selection, config_path) = match &config_result {
        Ok(history) => {
            if env_command
                .as_deref()
                .is_some_and(|command| !command.trim().is_empty())
            {
                ("environment".to_string(), "command".to_string(), None)
            } else if history.setting_configured {
                (
                    "config".to_string(),
                    history.setting.as_str().to_string(),
                    Some(path.display().to_string()),
                )
            } else {
                (
                    "default".to_string(),
                    history.setting.as_str().to_string(),
                    None,
                )
            }
        }
        Err(_) => (
            if env_command
                .as_deref()
                .is_some_and(|command| !command.trim().is_empty())
            {
                "environment".to_string()
            } else {
                "config".to_string()
            },
            "unavailable".to_string(),
            Some(path.display().to_string()),
        ),
    };

    let live = read_clipboard();
    let selection_result = config_result.map_err(|error| match error {
        config::ConfigError::Read(message)
        | config::ConfigError::Invalid(message) => message,
    });
    let mut selected = Vec::new();
    let mut source = SelectionSource::DefaultAuto;
    let selection_error = match selection_result {
        Ok(history) => match select_history_providers(
            env_command.as_deref(),
            &history.setting,
            &history.command,
            cfg!(target_os = "macos"),
            &bob_env::home_dir(),
        ) {
            Ok((candidates, selected_source)) => {
                selected = candidates;
                source = selected_source;
                None
            }
            Err(message) => Some(message),
        },
        Err(message) => Some(message),
    };

    let live_value = live.as_ref().ok().cloned();
    let result = match live {
        Err(_) => Err(ClipboardHistoryError::coded(
            "clipboard_live_unavailable",
            "the live clipboard could not be read",
        )),
        Ok(current) => match selection_error {
            Some(message) => Err(ClipboardHistoryError::coded(
                "clipboard_history_unavailable",
                message,
            )),
            None => read_agreed_history(
                current,
                &selected,
                &source,
                count,
                |provider, requested| provider.read(requested),
                SystemTime::now,
                thread::sleep,
                provider_running,
            ),
        },
    };

    let diagnostic_providers = status_providers(&selected);
    let providers = diagnostic_providers
        .iter()
        .map(|provider| {
            diagnose_provider(provider, &selected, live_value.as_ref())
        })
        .collect();

    let selected_names = selected
        .iter()
        .map(provider_provenance_name)
        .collect::<Vec<_>>();
    let selection = if selection == "auto" && !selected_names.is_empty() {
        format!("auto → {}", selected_names.join(", "))
    } else {
        selection
    };

    ClipboardStatusReport {
        count,
        live_source,
        selection,
        selection_source,
        config_path,
        providers,
        result,
    }
}

fn status_providers(selected: &[HistoryProvider]) -> Vec<HistoryProvider> {
    let mut providers = Vec::new();
    #[cfg(target_os = "macos")]
    {
        let home = bob_env::home_dir();
        providers.push(HistoryProvider::Alfred(
            super::history::alfred_database(&home),
        ));
        providers.push(HistoryProvider::Clipy(super::history::clipy_database(
            &home,
        )));
    }
    providers.extend(
        selected
            .iter()
            .filter(|provider| {
                matches!(provider, HistoryProvider::Command { .. })
            })
            .cloned(),
    );
    providers
}

fn diagnose_provider(
    provider: &HistoryProvider,
    selected: &[HistoryProvider],
    live: Option<&String>,
) -> ClipboardProviderStatus {
    let store_path =
        provider_path(provider).map(|path| path.display().to_string());
    let exists = provider_path(provider).map(std::path::Path::is_file);
    let running = provider_running(provider);
    let mut schema_ok = None;
    let mut newest_entry_age_seconds = None;
    let mut head_agrees_with_live = false;
    let mut error = None;

    match provider.read(1) {
        Ok(entries) => {
            if store_path.is_some() {
                schema_ok = Some(true);
            }
            if let Some(entry) = entries.first() {
                if let Some(at) = entry.at {
                    newest_entry_age_seconds = Some(
                        SystemTime::now()
                            .duration_since(at)
                            .unwrap_or_default()
                            .as_secs(),
                    );
                }
                head_agrees_with_live = live.is_some_and(|current| {
                    normalize_history_entry(entry)
                        .is_ok_and(|head| head == *current)
                });
            }
        }
        Err(provider_error) => {
            if matches!(&provider_error, ProviderError::Unsupported(_)) {
                schema_ok = Some(false);
            }
            error =
                Some(if matches!(provider, HistoryProvider::Command { .. }) {
                    "command provider unavailable".to_string()
                } else {
                    provider_error.message().to_string()
                });
        }
    }

    ClipboardProviderStatus {
        name: provider_name(provider),
        selected: selected.contains(provider),
        store_path,
        exists,
        schema_ok,
        running,
        newest_entry_age_seconds,
        head_agrees_with_live,
        error,
    }
}

fn live_source_name() -> String {
    if bob_env::var("BOB_CLIPBOARD_CMD")
        .ok()
        .is_some_and(|command| !command.trim().is_empty())
    {
        return "BOB_CLIPBOARD_CMD".to_string();
    }
    #[cfg(target_os = "macos")]
    return "pbpaste".to_string();
    #[cfg(target_os = "linux")]
    {
        if bob_env::var_os("WAYLAND_DISPLAY").is_some() {
            return "wl-paste".to_string();
        }
        if bob_env::var_os("DISPLAY").is_some() {
            return "xclip or xsel".to_string();
        }
        if bob_env::var_os("TMUX").is_some() {
            return "tmux show-buffer".to_string();
        }
    }
    "unavailable".to_string()
}

/// The agreement read over an already-read live value, so one capture
/// invocation shares a single live read between `%c` items and `%c<N>`
/// history agreement.
pub(crate) fn read_clipboard_history_with_current(
    current: String,
    count: usize,
) -> Result<AgreedClipboardHistory, ClipboardHistoryError> {
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

fn clipboard_command_output() -> Result<Vec<u8>, String> {
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
            match run_bounded_command(
                "xclip",
                &["-selection", "clipboard", "-o"],
            ) {
                Ok(output) => {
                    return require_clipboard_success(output, "xclip")
                }
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

/// Run a clipboard subprocess with the shared deadline, output cap, and
/// process-group reaping.
pub(super) fn run_bounded_command(
    program: &str,
    args: &[&str],
) -> io::Result<RawRunOutput> {
    run_bounded_argv(
        program,
        args.iter().map(OsString::from).collect::<Vec<_>>(),
    )
}

/// The same bound for argv built at runtime (history commands).
pub(super) fn run_bounded_argv(
    program: &str,
    args: Vec<OsString>,
) -> io::Result<RawRunOutput> {
    subprocess_runner::run_raw(&RawRunSpec {
        program: PathBuf::from(program),
        args,
        env: Vec::new(),
        env_remove: Vec::new(),
        timeout: CLIPBOARD_TIMEOUT,
        stdout_cap: CLIPBOARD_STDOUT_CAP,
        stderr_cap: CLIPBOARD_STDERR_CAP,
    })
}

fn run_required_command(
    program: &str,
    args: &[&str],
    label: &str,
) -> Result<Vec<u8>, String> {
    let output = run_bounded_command(program, args)
        .map_err(|error| format!("run {label}: {error}"))?;
    require_clipboard_success(output, label)
}

/// Timeout, over-cap, and failure checks shared by every clipboard
/// command. Returns the raw bytes: NUL and UTF-8 detection need them.
pub(super) fn require_clipboard_success(
    output: RawRunOutput,
    label: &str,
) -> Result<Vec<u8>, String> {
    if output.timed_out {
        return Err(format!(
            "clipboard command {label} timed out after {} seconds",
            CLIPBOARD_TIMEOUT.as_secs()
        ));
    }
    if output.stdout_truncated {
        return Err(format!(
            "{label}: clipboard output exceeds 16 MiB; copy a file path instead"
        ));
    }
    match output.status {
        Some(0) => Ok(output.stdout),
        Some(code) => Err(format!(
            "clipboard command {label} exited with {code}{}",
            stderr_suffix(&output.stderr)
        )),
        None => Err(format!(
            "clipboard command {label} exited with a signal{}",
            stderr_suffix(&output.stderr)
        )),
    }
}

fn stderr_suffix(stderr: &[u8]) -> String {
    let detail = String::from_utf8_lossy(stderr);
    let detail = detail.trim();
    if detail.is_empty() {
        String::new()
    } else {
        format!(": {detail}")
    }
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
