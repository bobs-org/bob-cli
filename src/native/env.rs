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

pub fn home_dir() -> PathBuf {
    env::var_os("HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
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
        "{:04}{:02}{:02}_day.md",
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
