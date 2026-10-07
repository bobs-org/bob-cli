use std::{
    env,
    ffi::{OsStr, OsString},
    path::{Path, PathBuf},
    process::Command,
};

use chrono::{Datelike, Local, NaiveDate, NaiveDateTime, NaiveTime};

pub fn bob_dir() -> PathBuf {
    env::var_os("BOB_DIR")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .map(|path| expand_tilde(&path))
        .unwrap_or_else(|| home_dir().join("bob"))
}

pub fn plugins_dir() -> PathBuf {
    env::var_os("BOB_PLUGINS_DIR")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .map(|path| expand_tilde(&path))
        .unwrap_or_else(|| {
            home_dir().join("projects/github/bobs-org/bob-plugins")
        })
}

pub fn plugin_backups_dir() -> PathBuf {
    env::var_os("BOB_PLUGIN_BACKUPS_DIR")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .map(|path| expand_tilde(&path))
        .unwrap_or_else(|| {
            home_dir().join(".local/state/bob-cli/plugin-backups")
        })
}

pub fn home_dir() -> PathBuf {
    env::var_os("HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

pub fn state_home() -> PathBuf {
    env::var_os("XDG_STATE_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            env::var_os("HOME")
                .filter(|value| !value.is_empty())
                .map(|home| PathBuf::from(home).join(".local/state"))
        })
        .unwrap_or_else(|| env::temp_dir().join("bob-cli-state"))
}

pub fn bob_cli_state_dir() -> PathBuf {
    state_home().join("bob-cli")
}

pub fn bob_cli_cache_dir() -> PathBuf {
    env::var_os("XDG_CACHE_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            env::var_os("HOME")
                .filter(|value| !value.is_empty())
                .map(|home| PathBuf::from(home).join(".cache"))
        })
        .unwrap_or_else(|| env::temp_dir().join("bob-cli-cache"))
        .join("bob-cli")
}

pub fn expand_tilde(path: &Path) -> PathBuf {
    let Some(path_text) = path.to_str() else {
        return path.to_path_buf();
    };

    if path_text == "~" {
        return home_dir();
    }

    if let Some(suffix) = path_text.strip_prefix("~/") {
        return home_dir().join(suffix);
    }

    path.to_path_buf()
}

pub fn current_datetime() -> NaiveDateTime {
    if let Some(override_value) =
        env::var("BOB_NOW").ok().filter(|value| !value.is_empty())
        && let Some(parsed) = parse_datetime_override(&override_value)
    {
        return parsed;
    }

    if let Some(date_value) =
        env::var("DATE").ok().filter(|value| !value.is_empty())
    {
        if let Some(parsed) = parse_datetime_override(&date_value) {
            return parsed;
        }

        if let Some(parsed) = date_command_datetime(&date_value) {
            return parsed;
        }
    }

    Local::now().naive_local()
}

pub fn default_day_file(bob_dir: &Path) -> PathBuf {
    let today = current_datetime();
    bob_dir.join(format!("{:04}", today.year())).join(format!(
        "{:04}{:02}{:02}.md",
        today.year(),
        today.month(),
        today.day()
    ))
}

pub fn parse_datetime_override(value: &str) -> Option<NaiveDateTime> {
    let normalized = value.replace('T', " ");
    for format in ["%Y-%m-%d %H:%M:%S", "%Y-%m-%d %H:%M"] {
        if let Ok(parsed) = NaiveDateTime::parse_from_str(&normalized, format) {
            return Some(parsed);
        }
    }

    NaiveDate::parse_from_str(&normalized, "%Y-%m-%d")
        .ok()
        .and_then(|date| date.and_hms_opt(0, 0, 0))
}

pub fn parse_hhmm(value: &str) -> Option<NaiveTime> {
    let normalized = value.replace(':', "");
    if normalized.len() != 4
        || !normalized.bytes().all(|byte| byte.is_ascii_digit())
    {
        return None;
    }

    let hour = normalized[..2].parse().ok()?;
    let minute = normalized[2..].parse().ok()?;
    NaiveTime::from_hms_opt(hour, minute, 0)
}

pub fn exit_code(status: std::process::ExitStatus) -> i32 {
    if let Some(code) = status.code() {
        return code;
    }

    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(signal) = status.signal() {
            return 128 + signal;
        }
    }

    1
}

pub fn os_to_string(value: &OsStr) -> String {
    value.to_string_lossy().into_owned()
}

/// Resolve the `uv` binary shared by the web-clip and Keep adapters.
///
/// Returns the path plus whether it was found outside `PATH`. `PATH` is
/// checked first; then `$HOME/.local/bin/uv`, `$HOME/.cargo/bin/uv`,
/// `/opt/homebrew/bin/uv`, and `/usr/local/bin/uv`, in that order. The
/// Mac app environment sees only `~/.local/bin`, so the fallbacks keep
/// `bob ref create` and `bob gkeep` working there.
pub fn resolve_uv() -> Option<(PathBuf, bool)> {
    if let Some(path) = find_on_path("uv") {
        return Some((path, false));
    }
    let home = home_dir();
    let mut candidates = vec![
        home.join(".local/bin/uv"),
        home.join(".cargo/bin/uv"),
        PathBuf::from("/opt/homebrew/bin/uv"),
        PathBuf::from("/usr/local/bin/uv"),
    ];
    candidates.retain(|path| is_executable_file(path));
    candidates.into_iter().next().map(|path| (path, true))
}

/// Look `name` up on `PATH`, returning an executable file if found.
pub fn find_on_path(name: &str) -> Option<PathBuf> {
    let paths = env::var_os("PATH")?;
    env::split_paths(&paths)
        .map(|dir| dir.join(name))
        .find(|path| is_executable_file(path))
}

#[cfg(unix)]
pub fn is_executable_file(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    path.is_file()
        && path
            .metadata()
            .map(|metadata| metadata.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
}

#[cfg(not(unix))]
pub fn is_executable_file(path: &Path) -> bool {
    path.is_file()
}

fn date_command_datetime(date_command: &str) -> Option<NaiveDateTime> {
    let output = run_date_command(date_command, ["+%Y-%m-%d %H:%M:%S"])?;
    parse_datetime_override(output.trim())
}

fn run_date_command<const N: usize>(
    date_command: &str,
    args: [&str; N],
) -> Option<String> {
    let parts = split_command(date_command);
    let (program, command_args) = parts.split_first()?;
    let output = Command::new(program)
        .args(command_args)
        .args(args)
        .output()
        .ok()?;

    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).into_owned())
}

fn split_command(command: &str) -> Vec<OsString> {
    command.split_whitespace().map(OsString::from).collect()
}
