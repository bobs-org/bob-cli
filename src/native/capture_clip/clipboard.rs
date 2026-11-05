use std::{
    io,
    process::{Command, Output},
};

use super::history::{select_history_providers, EntryKind, HistoryEntry};
use crate::native::{config, env as bob_env};

pub(crate) fn read_clipboard() -> Result<String, String> {
    let output = clipboard_command_output()?;
    normalize_clipboard_output(output.stdout)
}

pub(crate) fn read_clipboard_history(
    count: usize,
) -> Result<Vec<String>, String> {
    let current = read_clipboard()?;
    if count == 1 {
        return Ok(vec![current]);
    }

    // Interim wiring until the agreement check replaces the merge: read
    // `count` entries from the first selected candidate and keep today's
    // lag-tolerant merge. A non-text entry at a used position maps to the
    // indexed file-or-image error.
    let candidates = read_history_entries(count)?
        .into_iter()
        .enumerate()
        .map(|(index, entry)| {
            let entry_number = index + 1;
            history_entry_text(entry, entry_number).and_then(|value| {
                normalize_clipboard_output(value.into_bytes()).map_err(
                    |error| {
                        format!(
                            "clipboard history entry {entry_number}: {error}"
                        )
                    },
                )
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    merge_history_candidates(current, candidates, count)
}

fn history_entry_text(
    entry: HistoryEntry,
    entry_number: usize,
) -> Result<String, String> {
    match entry.kind {
        EntryKind::Text => entry.text.ok_or_else(|| {
            format!(
                "clipboard history entry {entry_number} has no stored text"
            )
        }),
        EntryKind::NotText(detail) => Err(format!(
            "clipboard history entry {entry_number} is a file or image ({detail}); counted capture supports text history"
        )),
    }
}

pub(super) fn merge_history_candidates(
    current: String,
    mut candidates: Vec<String>,
    count: usize,
) -> Result<Vec<String>, String> {
    if let Some(index) = candidates.iter().position(|value| value == &current) {
        candidates.remove(index);
    }

    let available = candidates.len() + 1;
    if available < count {
        return Err(format!(
            "clipboard history requested {count} entries but only {available} are available"
        ));
    }

    let mut values = Vec::with_capacity(count);
    values.push(current);
    values.extend(candidates.into_iter().take(count - 1));
    Ok(values)
}

fn read_history_entries(count: usize) -> Result<Vec<HistoryEntry>, String> {
    let history: config::ClipboardHistoryConfig =
        config::load_clipboard_history_config(&config::config_path()).map_err(
            |error| match error {
                config::ConfigError::Read(message)
                | config::ConfigError::Invalid(message) => message,
            },
        )?;
    let env_command = bob_env::var("BOB_CLIPBOARD_HISTORY_CMD").ok();
    let (candidates, _) = select_history_providers(
        env_command.as_deref(),
        &history.setting,
        &history.command,
        cfg!(target_os = "macos"),
        &bob_env::home_dir(),
    )?;
    // The agreement phase polls every candidate; this interim wiring reads
    // the first one and keeps today's merge.
    let mut candidates = candidates.into_iter();
    let Some(provider) = candidates.next() else {
        return Err("no clipboard history provider is available".to_string());
    };
    provider
        .read(count)
        .map_err(|error| error.message().to_string())
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
