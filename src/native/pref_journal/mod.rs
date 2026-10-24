//! Shared append-only preference journals (`docs/task-relations.md` §16,
//! `docs/pomodoro-themes.md`).
//!
//! Every vocabulary preference (Pomodoro theme hides, relation label
//! hides) lives in one namespaced directory of per-host JSONL files:
//! `.bob/themes/<host>.jsonl` for `theme.*` events, `.bob/relations/`
//! `<host>.jsonl` for `relation_label.*` events. This module owns the
//! namespace mechanics every journal shares: directory paths, the host
//! label, event timestamps, single-`O_APPEND` writes, the read-all scan
//! with bounded malformed-line warnings, last-writer-wins ordering, and
//! the rename state move. Payload parsing and the per-key fold stay in
//! each namespace module, because each namespace carries its own event
//! kinds.

use std::{
    collections::BTreeMap,
    fs, io,
    io::Write,
    path::{Path, PathBuf},
};

use chrono::{DateTime, FixedOffset, Local, TimeZone};
use serde_json::Value;

use crate::native::{env as bob_env, vault_sync};

pub(crate) const MAX_MALFORMED_WARNINGS: usize = 8;

/// One preference namespace: where its journal lives and which noun its
/// warnings name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Namespace {
    /// Vault-relative journal directory, e.g. `.bob/themes`.
    pub(crate) dir: &'static str,
    /// Singular noun for warnings, e.g. `theme` or `relation`.
    pub(crate) noun: &'static str,
}

pub(crate) const THEMES: Namespace = Namespace {
    dir: ".bob/themes",
    noun: "theme",
};
pub(crate) const RELATIONS: Namespace = Namespace {
    dir: ".bob/relations",
    noun: "relation",
};

/// One raw journal line with its envelope decoded; each namespace
/// parses `value` into its own payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StoredEvent {
    pub(crate) ts: DateTime<FixedOffset>,
    pub(crate) host: String,
    pub(crate) path: String,
    pub(crate) line: u32,
    pub(crate) kind: String,
    pub(crate) value: Value,
}

pub(crate) fn host_label() -> String {
    vault_sync::hostname_slug()
}

pub(crate) fn journal_dir(vault: &Path, namespace: Namespace) -> PathBuf {
    vault.join(namespace.dir)
}

pub(crate) fn host_journal_path(
    vault: &Path,
    namespace: Namespace,
    host: &str,
) -> PathBuf {
    journal_dir(vault, namespace).join(format!("{host}.jsonl"))
}

pub(crate) fn event_timestamp() -> DateTime<FixedOffset> {
    let naive = bob_env::current_datetime();
    if let Some(timezone) = bob_env::configured_timezone()
        && let Some(datetime) = timezone.from_local_datetime(&naive).single()
    {
        return datetime.fixed_offset();
    }
    Local
        .from_local_datetime(&naive)
        .single()
        .or_else(|| Local.from_local_datetime(&naive).earliest())
        .map(|datetime| datetime.fixed_offset())
        .unwrap_or_else(|| {
            DateTime::from_naive_utc_and_offset(
                naive,
                FixedOffset::east_opt(0).unwrap(),
            )
        })
}

/// Append one already-serialized JSON line with a single `O_APPEND`
/// write. Creates the namespace directory on first use.
pub(crate) fn append_line(
    vault: &Path,
    namespace: Namespace,
    host: &str,
    line: &str,
) -> io::Result<()> {
    let directory = journal_dir(vault, namespace);
    fs::create_dir_all(&directory)?;
    let path = host_journal_path(vault, namespace, host);
    let mut options = fs::OpenOptions::new();
    options.create(true).append(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o644);
    }
    let mut file = options.open(&path)?;
    file.write_all(line.as_bytes())?;
    Ok(())
}

/// Read every `*.jsonl` file in the namespace directory, in sorted
/// path order, skipping malformed lines with a bounded warning.
pub(crate) fn read_raw(
    vault: &Path,
    namespace: Namespace,
) -> io::Result<(Vec<StoredEvent>, Vec<String>)> {
    let directory = vault.join(namespace.dir);
    if !directory.is_dir() {
        return Ok((Vec::new(), Vec::new()));
    }

    let mut paths = match fs::read_dir(&directory) {
        Ok(entries) => entries
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.path())
            .filter(|path| {
                path.extension().is_some_and(|ext| ext == "jsonl")
                    && path.is_file()
            })
            .collect::<Vec<PathBuf>>(),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok((Vec::new(), Vec::new()));
        }
        Err(error) => return Err(error),
    };
    paths.sort();

    let mut events = Vec::new();
    let mut malformed = 0usize;
    let mut warnings = Vec::new();
    for path in paths {
        let bytes = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(_) => continue,
        };
        let relative = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        for (index, line) in bytes.split(|byte| *byte == b'\n').enumerate() {
            if line.is_empty()
                || line.iter().all(|byte| byte.is_ascii_whitespace())
            {
                continue;
            }
            let Ok(text) = std::str::from_utf8(line) else {
                malformed += 1;
                push_malformed(
                    &mut warnings,
                    namespace,
                    &relative,
                    index + 1,
                    malformed,
                );
                continue;
            };
            let Some(event) = parse_envelope(
                text,
                &relative,
                u32::try_from(index + 1).unwrap_or(u32::MAX),
            ) else {
                malformed += 1;
                push_malformed(
                    &mut warnings,
                    namespace,
                    &relative,
                    index + 1,
                    malformed,
                );
                continue;
            };
            events.push(event);
        }
    }
    if malformed > MAX_MALFORMED_WARNINGS {
        warnings.push(format!(
            "skipped {malformed} malformed {} journal lines",
            namespace.noun
        ));
    }
    Ok((events, warnings))
}

fn push_malformed(
    warnings: &mut Vec<String>,
    namespace: Namespace,
    path: &str,
    line: usize,
    count: usize,
) {
    if count <= MAX_MALFORMED_WARNINGS {
        warnings.push(format!(
            "skipping malformed {} journal line {path}:{line}",
            namespace.noun
        ));
    }
}

fn parse_envelope(
    line: &str,
    path: &str,
    line_number: u32,
) -> Option<StoredEvent> {
    let value: Value = serde_json::from_str(line.trim()).ok()?;
    let object = value.as_object()?;
    if object.get("v").and_then(Value::as_u64) != Some(1) {
        return None;
    }
    let ts = object
        .get("ts")
        .and_then(Value::as_str)
        .and_then(|ts| DateTime::parse_from_rfc3339(ts).ok())?;
    let host = object.get("host").and_then(Value::as_str)?.to_string();
    if host.is_empty() {
        return None;
    }
    let kind = object.get("kind").and_then(Value::as_str)?.to_string();
    Some(StoredEvent {
        ts,
        host,
        path: path.to_string(),
        line: line_number,
        kind,
        value,
    })
}

/// Last writer wins: `(ts instant, host, path, line)`. Every fold
/// applies events in this order, so concurrent hosts converge.
pub(crate) fn sort_by_writer_order<T>(
    items: &mut [T],
    key: impl Fn(&T) -> (DateTime<FixedOffset>, &str, &str, u32),
) {
    items.sort_by(|left, right| {
        let (left_ts, left_host, left_path, left_line) = key(left);
        let (right_ts, right_host, right_path, right_line) = key(right);
        left_ts
            .cmp(&right_ts)
            .then_with(|| left_host.cmp(right_host))
            .then_with(|| left_path.cmp(&right_path))
            .then_with(|| left_line.cmp(&right_line))
    });
}

/// A rename moves the source key's explicit state to the target,
/// unless the target already has state. Events after a rename on the
/// old key still apply to the old key, because the fold keeps
/// applying in timestamp order.
pub(crate) fn move_state_for_rename<V: Clone>(
    by_key: &mut BTreeMap<String, V>,
    from: &str,
    to: &str,
) {
    if from == to {
        return;
    }
    if let Some(source) = by_key.get(from).cloned()
        && !by_key.contains_key(to)
    {
        by_key.insert(to.to_string(), source);
        by_key.remove(from);
    }
}

/// A required key field: trimmed, non-empty, normalized with the
/// caller's key function.
pub(crate) fn required_key(
    object: &serde_json::Map<String, Value>,
    field: &str,
    normalize: impl Fn(&str) -> String,
) -> Option<String> {
    let key = object.get(field).and_then(Value::as_str)?.trim();
    if key.is_empty() {
        return None;
    }
    let key = normalize(key);
    if key.is_empty() {
        None
    } else {
        Some(key)
    }
}

pub(crate) fn optional_name(
    object: &serde_json::Map<String, Value>,
    field: &str,
) -> String {
    object
        .get(field)
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string()
}
