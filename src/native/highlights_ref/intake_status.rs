//! Per-host intake status record: the last intake success and the last
//! intake failure, for `bob ref doctor`.
//!
//! Lives at `bob_cli_state_dir()/ref/intake-status.json` and is written
//! atomically. Recording is best-effort: a failed write only warns and never
//! changes a command's result.

use std::{fs, path::PathBuf};

use chrono::{DateTime, Local, SecondsFormat};
use serde::{Deserialize, Serialize};

use crate::native::env::bob_cli_state_dir;

const SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(super) struct LastSuccess {
    pub(super) at: String,
    pub(super) command: String,
    pub(super) filed: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(super) struct LastFailure {
    pub(super) at: String,
    pub(super) command: String,
    pub(super) reason: String,
    pub(super) message: String,
}

#[derive(Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub(super) struct IntakeStatus {
    #[serde(default)]
    schema_version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) last_success: Option<LastSuccess>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) last_failure: Option<LastFailure>,
}

fn status_path() -> PathBuf {
    bob_cli_state_dir().join("ref").join("intake-status.json")
}

fn now() -> String {
    Local::now().to_rfc3339_opts(SecondsFormat::Secs, false)
}

/// The recorded status; `None` when absent or unreadable.
pub(super) fn read() -> Option<IntakeStatus> {
    let text = fs::read_to_string(status_path()).ok()?;
    serde_json::from_str(&text).ok()
}

/// Seconds since an RFC 3339 `at` stamp; `None` when it does not parse.
pub(super) fn age_secs(at: &str) -> Option<i64> {
    let when = DateTime::parse_from_rfc3339(at).ok()?;
    Some(
        (Local::now() - when.with_timezone(&Local))
            .num_seconds()
            .max(0),
    )
}

/// Record an intake that finished without leaving anything queued.
/// Returns a warning when the status file could not be written.
pub(super) fn record_success(command: &str, filed: usize) -> Option<String> {
    update(|status| {
        status.last_success = Some(LastSuccess {
            at: now(),
            command: command.to_string(),
            filed,
        });
    })
}

/// Record an intake that queued an item, stopped, or could not start.
/// Returns a warning when the status file could not be written.
pub(super) fn record_failure(
    command: &str,
    reason: &str,
    message: &str,
) -> Option<String> {
    update(|status| {
        status.last_failure = Some(LastFailure {
            at: now(),
            command: command.to_string(),
            reason: reason.to_string(),
            message: message.to_string(),
        });
    })
}

fn update(change: impl FnOnce(&mut IntakeStatus)) -> Option<String> {
    let mut status = read().unwrap_or_default();
    status.schema_version = SCHEMA_VERSION;
    change(&mut status);
    match write(&status) {
        Ok(()) => None,
        Err(error) => {
            Some(format!("warning: could not record intake status: {error}"))
        }
    }
}

fn write(status: &IntakeStatus) -> std::io::Result<()> {
    let path = status_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let temp = path.with_extension(format!("json.tmp{}", std::process::id()));
    let mut text =
        serde_json::to_string_pretty(status).map_err(std::io::Error::other)?;
    text.push('\n');
    fs::write(&temp, text)?;
    fs::rename(&temp, &path).inspect_err(|_| {
        let _ = fs::remove_file(&temp);
    })
}
