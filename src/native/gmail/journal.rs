//! The per-host `bob gmail` journal: `.bob/gmail/<host>.jsonl`.
//!
//! Shell phase owns only the namespace and the malformed-line count for
//! `doctor`; save/compose/pull phases append their own events.

use std::{collections::BTreeSet, io, path::Path};

use serde_json::json;

use crate::native::pref_journal::{self, Namespace};

pub(crate) const NAMESPACE: Namespace = Namespace {
    dir: ".bob/gmail",
    noun: "gmail",
};

const PULLED_KIND: &str = "gmail.pulled";

/// Fold the thread identities already filed by a label pull. The event has
/// no message content; the mail-note index and inbox file provide recovery
/// evidence if a process stops between their durable writes.
pub(crate) fn pulled_identities(
    vault: &Path,
) -> io::Result<(BTreeSet<(String, String)>, Vec<String>)> {
    let (events, warnings) = pref_journal::read_raw(vault, NAMESPACE)?;
    let mut identities = BTreeSet::new();
    for event in events {
        if event.kind != PULLED_KIND {
            continue;
        }
        let Some(object) =
            event.value.get("value").and_then(|value| value.as_object())
        else {
            continue;
        };
        let (Some(account), Some(thread_id)) = (
            object.get("account").and_then(|value| value.as_str()),
            object.get("thread_id").and_then(|value| value.as_str()),
        ) else {
            continue;
        };
        if !account.is_empty()
            && !thread_id.is_empty()
            && thread_id.bytes().all(|byte| byte.is_ascii_hexdigit())
        {
            identities.insert((
                account.to_ascii_lowercase(),
                thread_id.to_ascii_lowercase(),
            ));
        }
    }
    Ok((identities, warnings))
}

/// Append one idempotence marker after the corresponding inbox task is
/// durable. The inbox link itself is also checked on the next run to recover
/// if this append fails.
pub(crate) fn append_pulled(
    vault: &Path,
    host: &str,
    account: &str,
    thread_id: &str,
    inbox: &str,
    note: &str,
) -> io::Result<()> {
    let ts = pref_journal::event_timestamp()
        .to_rfc3339_opts(chrono::SecondsFormat::Secs, false);
    let value = json!({
        "account": account,
        "thread_id": thread_id.to_ascii_lowercase(),
        "inbox": inbox,
        "note": note,
    });
    let mut line = serde_json::to_string(&json!({
        "v": 1,
        "ts": ts,
        "host": host,
        "kind": PULLED_KIND,
        "value": value,
    }))
    .unwrap_or_else(|_| "{}".to_string());
    line.push('\n');
    pref_journal::append_line(vault, NAMESPACE, host, &line)
}

/// Count malformed `gmail.*` journal lines for `doctor`.
pub(crate) fn malformed_count(vault: &Path) -> usize {
    let directory = vault.join(NAMESPACE.dir);
    let mut paths = match std::fs::read_dir(&directory) {
        Ok(entries) => entries
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.path())
            .filter(|path| {
                path.extension().is_some_and(|ext| ext == "jsonl")
                    && path.is_file()
            })
            .collect::<Vec<_>>(),
        Err(_) => return 0,
    };
    paths.sort();
    let mut count = 0usize;
    for path in paths {
        let Ok(bytes) = std::fs::read(&path) else {
            continue;
        };
        let text = String::from_utf8_lossy(&bytes);
        for line in text.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            let parsed: Result<serde_json::Value, _> =
                serde_json::from_str(trimmed);
            let Ok(value) = parsed else {
                count += 1;
                continue;
            };
            let kind = value.get("kind").and_then(|v| v.as_str()).unwrap_or("");
            if kind.is_empty() || !kind.starts_with("gmail.") {
                // Unknown or missing kinds are skipped by the fold; only
                // malformed `gmail.*` lines count here so future save
                // events never trip the shell-phase doctor.
                if kind.starts_with("gmail.") {
                    count += 1;
                }
                continue;
            }
            if value.get("ts").is_none()
                || value.get("host").is_none()
                || value.get("value").is_none()
            {
                count += 1;
            }
        }
    }
    count
}
