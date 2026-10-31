//! The per-host `bob gcal sync` journal: `.bob/gcal/<host>.jsonl`.
//!
//! Records are `gcal.row`, `gcal.link`, `gcal.gone`, `gcal.series_root`,
//! and `gcal.flag`, union-merged and folded into state keyed by
//! (occurrence key, day). Series roots fold into a series-keyed map of
//! learned `[root::]` values; flags remember once-reported lifecycle
//! notices. One host-local status file under `$XDG_STATE_HOME/bob/gcal/`
//! feeds `list` and `doctor`; it is never written to the vault.

use std::{
    collections::{BTreeMap, HashSet},
    fs,
    path::{Path, PathBuf},
};

use chrono::{DateTime, FixedOffset};
use serde_json::{json, Value};

use crate::native::{
    env as bob_env,
    pref_journal::{self, Namespace, StoredEvent},
    vault_sync,
};

pub(crate) const NAMESPACE: Namespace = Namespace {
    dir: ".bob/gcal",
    noun: "gcal",
};

/// Journaled state for one (occurrence key, day).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct OccurrenceState {
    pub row: Option<RowRecord>,
    pub link: bool,
    pub gone: bool,
    /// Once-reported lifecycle notices (`kept_edit`, `kept_root`,
    /// `worked_kept`, `now_maybe`, `awaiting_reply`, …).
    pub flags: HashSet<String>,
}

/// One `gcal.row` record: what Bob wrote for one occurrence and day.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RowRecord {
    pub block_id: String,
    pub row_kind: String,
    pub spans: BTreeMap<String, String>,
    pub root: Option<String>,
    pub source: Option<String>,
    pub series_id: Option<String>,
    pub url: Option<String>,
    pub ts: DateTime<FixedOffset>,
    pub host: String,
    pub path: String,
}

/// One learned series root: a vault edit on one occurrence teaches the
/// series. Last writer wins, so a later edit (including a change back)
/// re-teaches the series.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LearnedRoot {
    pub root: String,
    pub day: String,
    pub block_id: String,
}

/// A new `gcal.row` to append.
#[derive(Debug, Clone)]
pub(crate) struct NewRow {
    pub calendar_id: String,
    pub event_id: String,
    pub ical_uid: Option<String>,
    pub series_id: Option<String>,
    pub day: String,
    pub path: String,
    pub block_id: String,
    pub row_kind: String,
    pub spans: BTreeMap<String, String>,
    pub root: Option<String>,
    pub source: Option<String>,
    /// The `📅` identity URL, when the event has one: the match key for
    /// block-less context rows.
    pub url: Option<String>,
}

/// A new `gcal.link` or `gcal.gone` to append.
#[derive(Debug, Clone)]
pub(crate) struct NewMark {
    pub calendar_id: String,
    pub event_id: String,
    pub day: String,
    pub block_id: Option<String>,
    pub reason: Option<String>,
}

pub(crate) fn occurrence_key(calendar_id: &str, event_id: &str) -> String {
    format!("{calendar_id}\u{1f}{event_id}")
}

pub(crate) fn host_label() -> String {
    vault_sync::hostname_slug()
}

/// Short SHA-256 (16 hex chars) of one managed span.
pub(crate) fn span_hash(text: &str) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(text.as_bytes());
    hex::encode(digest)[..16].to_string()
}

/// Read and fold every `gcal.*` record, plus the newest record's
/// timestamp and host for the `list` header fallback.
pub(crate) fn load_state(
    vault: &Path,
) -> std::io::Result<(
    BTreeMap<(String, String), OccurrenceState>,
    Vec<String>,
    Option<(DateTime<FixedOffset>, String)>,
)> {
    let (stored, mut warnings) = pref_journal::read_raw(vault, NAMESPACE)?;
    let mut ordered = stored.iter().collect::<Vec<_>>();
    pref_journal::sort_by_writer_order(&mut ordered, |event| {
        (
            event.ts,
            event.host.as_str(),
            event.path.as_str(),
            event.line,
        )
    });
    let mut malformed = 0usize;
    let mut state: BTreeMap<(String, String), OccurrenceState> =
        BTreeMap::new();
    let mut newest: Option<(DateTime<FixedOffset>, String)> = None;
    for event in ordered {
        let is_gcal = event.kind.starts_with("gcal.");
        if !is_gcal {
            continue;
        }
        newest = Some((event.ts, event.host.clone()));
        match parse_record(event) {
            Some((key, day, update)) => {
                let entry = state.entry((key, day)).or_default();
                update.apply(entry);
            }
            None => {
                malformed += 1;
                if malformed <= pref_journal::MAX_MALFORMED_WARNINGS {
                    warnings.push(format!(
                        "skipping malformed gcal journal line {}:{}",
                        event.path, event.line
                    ));
                }
            }
        }
    }
    if malformed > pref_journal::MAX_MALFORMED_WARNINGS {
        warnings
            .push(format!("skipped {malformed} malformed gcal journal lines"));
    }
    Ok((state, warnings, newest))
}

enum Update {
    Row(RowRecord),
    Link,
    Gone,
    Flag(String),
}

impl Update {
    fn apply(self, state: &mut OccurrenceState) {
        match self {
            Self::Row(row) => state.row = Some(row),
            Self::Link => state.link = true,
            Self::Gone => state.gone = true,
            Self::Flag(reason) => {
                state.flags.insert(reason);
            }
        }
    }
}

fn parse_record(event: &StoredEvent) -> Option<(String, String, Update)> {
    let object = event.value.as_object()?;
    let key_value = object.get("key")?.as_object()?;
    let calendar_id = key_value.get("calendar_id")?.as_str()?;
    let event_id = key_value.get("event_id")?.as_str()?;
    let day = object.get("day")?.as_str()?.to_string();
    let key = occurrence_key(calendar_id, event_id);
    let update = match event.kind.as_str() {
        "gcal.row" => Update::Row(RowRecord {
            block_id: object.get("block_id")?.as_str()?.to_string(),
            row_kind: object
                .get("row")
                .and_then(Value::as_str)
                .unwrap_or("task")
                .to_string(),
            spans: object
                .get("spans")
                .and_then(Value::as_object)
                .map(|spans| {
                    spans
                        .iter()
                        .filter_map(|(name, hash)| {
                            Some((name.clone(), hash.as_str()?.to_string()))
                        })
                        .collect()
                })
                .unwrap_or_default(),
            root: object
                .get("root")
                .and_then(|root| root.get("value"))
                .and_then(Value::as_str)
                .map(str::to_string),
            source: object
                .get("root")
                .and_then(|root| root.get("source"))
                .and_then(Value::as_str)
                .map(str::to_string),
            series_id: object
                .get("series_id")
                .and_then(Value::as_str)
                .map(str::to_string),
            url: object
                .get("url")
                .and_then(Value::as_str)
                .map(str::to_string),
            ts: event.ts,
            host: event.host.clone(),
            path: object
                .get("path")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
        }),
        "gcal.link" => Update::Link,
        "gcal.gone" => Update::Gone,
        "gcal.flag" => Update::Flag(
            object
                .get("reason")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
        ),
        _ => return None,
    };
    Some((key, day, update))
}

/// Fold every `gcal.series_root` record into a series-keyed map of
/// learned roots, keyed by (calendar id, series id). Last writer wins,
/// so a later vault edit (including a change back) re-teaches the
/// series. Malformed lines are skipped without counting: callers that
/// need the count use [`malformed_count`].
pub(crate) fn load_series_roots(
    vault: &Path,
) -> std::io::Result<BTreeMap<(String, String), LearnedRoot>> {
    let (stored, _) = pref_journal::read_raw(vault, NAMESPACE)?;
    let mut ordered = stored.iter().collect::<Vec<_>>();
    pref_journal::sort_by_writer_order(&mut ordered, |event| {
        (
            event.ts,
            event.host.as_str(),
            event.path.as_str(),
            event.line,
        )
    });
    let mut roots = BTreeMap::new();
    for event in ordered {
        if event.kind.as_str() != "gcal.series_root" {
            continue;
        }
        let Some(object) = event.value.as_object() else {
            continue;
        };
        let key = object.get("key").and_then(Value::as_object);
        let (Some(calendar_id), Some(series)) = (
            key.and_then(|key| key.get("calendar_id"))
                .and_then(Value::as_str),
            key.and_then(|key| key.get("series"))
                .and_then(Value::as_str),
        ) else {
            continue;
        };
        let Some(root) = object
            .get("root")
            .and_then(Value::as_str)
            .map(str::to_string)
        else {
            continue;
        };
        let learned_from =
            object.get("learned_from").and_then(Value::as_object);
        roots.insert(
            (calendar_id.to_string(), series.to_string()),
            LearnedRoot {
                root,
                day: learned_from
                    .and_then(|from| from.get("day"))
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
                block_id: learned_from
                    .and_then(|from| from.get("block_id"))
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
            },
        );
    }
    Ok(roots)
}

/// Append one `gcal.series_root` record: a vault edit on one occurrence
/// of a recurring event teaches the series.
#[allow(clippy::too_many_arguments)]
pub(crate) fn append_series_root(
    vault: &Path,
    host: &str,
    calendar_id: &str,
    series: &str,
    root: &str,
    day: &str,
    block_id: &str,
) -> std::io::Result<String> {
    let line = serde_json::to_string(&json!({
        "v": 1,
        "ts": pref_journal::event_timestamp()
            .to_rfc3339_opts(chrono::SecondsFormat::Secs, false),
        "host": host,
        "kind": "gcal.series_root",
        "key": {"calendar_id": calendar_id, "series": series},
        "root": root,
        "learned_from": {"day": day, "block_id": block_id},
    }))
    .unwrap_or_else(|_| "{}".to_string())
        + "\n";
    pref_journal::append_line(vault, NAMESPACE, host, &line)?;
    Ok(line)
}

/// Count malformed `gcal.*` journal lines for `doctor`: unparseable
/// lines plus well-formed lines whose kind is a known gcal kind but
/// whose body does not parse, plus unknown `gcal.*` kinds. This walks
/// the files directly because [`pref_journal::read_raw`] drops
/// unparseable lines before folding.
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
        for line in bytes.split(|byte| *byte == b'\n') {
            if line.is_empty()
                || line.iter().all(|byte| byte.is_ascii_whitespace())
            {
                continue;
            }
            let Ok(text) = std::str::from_utf8(line) else {
                count += 1;
                continue;
            };
            let Ok(value) = serde_json::from_str::<Value>(text.trim()) else {
                count += 1;
                continue;
            };
            // Mirror `pref_journal`'s envelope: `v: 1` and a valid `ts`.
            let envelope_ok = value.as_object().is_some_and(|object| {
                object.get("v").and_then(Value::as_u64) == Some(1)
                    && object.get("ts").and_then(Value::as_str).is_some_and(
                        |ts| DateTime::parse_from_rfc3339(ts).is_ok(),
                    )
            });
            if !envelope_ok {
                count += 1;
                continue;
            }
            let kind = value
                .as_object()
                .and_then(|object| object.get("kind"))
                .and_then(Value::as_str)
                .unwrap_or("");
            if !kind.starts_with("gcal.") {
                continue;
            }
            let event = StoredEvent {
                ts: pref_journal::event_timestamp(),
                host: String::new(),
                path: String::new(),
                line: 0,
                kind: kind.to_string(),
                value,
            };
            if parse_record(&event).is_none()
                && parse_series_root(&event).is_none()
            {
                count += 1;
            }
        }
    }
    count
}

/// Parse one `gcal.series_root` record without folding it.
fn parse_series_root(
    event: &StoredEvent,
) -> Option<((String, String), LearnedRoot)> {
    if event.kind.as_str() != "gcal.series_root" {
        return None;
    }
    let object = event.value.as_object()?;
    let key = object.get("key")?.as_object()?;
    let calendar_id = key.get("calendar_id")?.as_str()?;
    let series = key.get("series")?.as_str()?;
    let root = object.get("root")?.as_str()?;
    Some((
        (calendar_id.to_string(), series.to_string()),
        LearnedRoot {
            root: root.to_string(),
            day: String::new(),
            block_id: String::new(),
        },
    ))
}

/// Append one `gcal.row` record with a single `O_APPEND` write.
pub(crate) fn append_row(
    vault: &Path,
    host: &str,
    row: &NewRow,
) -> std::io::Result<String> {
    let line = serde_json::to_string(&json!({
        "v": 1,
        "ts": pref_journal::event_timestamp()
            .to_rfc3339_opts(chrono::SecondsFormat::Secs, false),
        "host": host,
        "kind": "gcal.row",
        "key": {"calendar_id": row.calendar_id, "event_id": row.event_id},
        "ical_uid": row.ical_uid,
        "series_id": row.series_id,
        "day": row.day,
        "path": row.path,
        "block_id": row.block_id,
        "url": row.url,
        "row": row.row_kind,
        "spans": row.spans,
        "root": {"value": row.root, "source": row.source},
    }))
    .unwrap_or_else(|_| "{}".to_string())
        + "\n";
    pref_journal::append_line(vault, NAMESPACE, host, &line)?;
    Ok(line)
}

/// Append one `gcal.link` or `gcal.gone` record.
pub(crate) fn append_mark(
    vault: &Path,
    host: &str,
    kind: &str,
    mark: &NewMark,
) -> std::io::Result<String> {
    let mut value = json!({
        "v": 1,
        "ts": pref_journal::event_timestamp()
            .to_rfc3339_opts(chrono::SecondsFormat::Secs, false),
        "host": host,
        "kind": kind,
        "key": {"calendar_id": mark.calendar_id, "event_id": mark.event_id},
        "day": mark.day,
    });
    if let Some(block_id) = mark.block_id.as_deref() {
        value["block_id"] = json!(block_id);
    }
    if let Some(reason) = mark.reason.as_deref() {
        value["reason"] = json!(reason);
    }
    let line = serde_json::to_string(&value)
        .unwrap_or_else(|_| "{}".to_string())
        + "\n";
    pref_journal::append_line(vault, NAMESPACE, host, &line)?;
    Ok(line)
}

/// Host-local sync status, read by `list` and `doctor`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SyncStatus {
    pub ts: DateTime<FixedOffset>,
    pub host: String,
    pub day: String,
    pub outcome: String,
    pub tasks_added: usize,
    pub context_added: usize,
    pub links_added: usize,
    pub error: Option<String>,
}

fn status_path() -> PathBuf {
    bob_env::state_home()
        .join("bob")
        .join("gcal")
        .join("status.json")
}

/// Record host-local status after a completed run (never the vault).
pub(crate) fn write_status(status: &SyncStatus) -> std::io::Result<()> {
    let path = status_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let value = json!({
        "ts": status.ts.to_rfc3339_opts(chrono::SecondsFormat::Secs, false),
        "host": status.host,
        "day": status.day,
        "outcome": status.outcome,
        "tasks_added": status.tasks_added,
        "context_added": status.context_added,
        "links_added": status.links_added,
        "error": status.error,
    });
    fs::write(path, serde_json::to_string(&value).unwrap_or_default())
}

/// Read the newest host-local status, if any host recorded one.
pub(crate) fn read_status() -> Option<SyncStatus> {
    let bytes = fs::read(status_path()).ok()?;
    let value: Value = serde_json::from_slice(&bytes).ok()?;
    let object = value.as_object()?;
    Some(SyncStatus {
        ts: object
            .get("ts")
            .and_then(Value::as_str)
            .and_then(|ts| DateTime::parse_from_rfc3339(ts).ok())?,
        host: object
            .get("host")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        day: object
            .get("day")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        outcome: object
            .get("outcome")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        tasks_added: object
            .get("tasks_added")
            .and_then(Value::as_u64)
            .unwrap_or(0) as usize,
        context_added: object
            .get("context_added")
            .and_then(Value::as_u64)
            .unwrap_or(0) as usize,
        links_added: object
            .get("links_added")
            .and_then(Value::as_u64)
            .unwrap_or(0) as usize,
        error: object
            .get("error")
            .and_then(Value::as_str)
            .map(str::to_string),
    })
}

/// `3 min ago` style relative time for the `list` header and doctor.
pub(crate) fn ago(ts: &DateTime<FixedOffset>) -> String {
    let now = pref_journal::event_timestamp();
    let seconds = now.signed_duration_since(*ts).num_seconds().max(0);
    if seconds < 60 {
        return format!("{seconds}s ago");
    }
    let minutes = seconds / 60;
    if minutes < 60 {
        return format!("{minutes} min ago");
    }
    let hours = minutes / 60;
    if hours < 48 {
        return format!("{hours} h ago");
    }
    format!("{} d ago", hours / 24)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn span_hash_is_stable_and_short() {
        assert_eq!(span_hash("a"), span_hash("a"));
        assert_eq!(span_hash("a").len(), 16);
        assert_ne!(span_hash("a"), span_hash("b"));
    }

    #[test]
    fn row_link_gone_fold_in_writer_order() {
        let dir = tempfile::tempdir().expect("vault");
        let host = "mac";
        append_row(
            dir.path(),
            host,
            &NewRow {
                calendar_id: "primary".to_string(),
                event_id: "e1".to_string(),
                ical_uid: None,
                series_id: None,
                day: "2026-10-30".to_string(),
                path: "2026/20261030.md".to_string(),
                block_id: "interview".to_string(),
                url: None,
                row_kind: "task".to_string(),
                spans: BTreeMap::from([(
                    "title".to_string(),
                    span_hash("Product interview"),
                )]),
                root: Some("job".to_string()),
                source: Some("colour tomato".to_string()),
            },
        )
        .expect("row");
        append_mark(
            dir.path(),
            host,
            "gcal.link",
            &NewMark {
                calendar_id: "primary".to_string(),
                event_id: "e1".to_string(),
                day: "2026-10-30".to_string(),
                block_id: Some("interview".to_string()),
                reason: None,
            },
        )
        .expect("link");
        append_mark(
            dir.path(),
            host,
            "gcal.gone",
            &NewMark {
                calendar_id: "primary".to_string(),
                event_id: "e9".to_string(),
                day: "2026-10-30".to_string(),
                block_id: None,
                reason: Some("line_removed".to_string()),
            },
        )
        .expect("gone");
        let (state, warnings, newest) = load_state(dir.path()).expect("fold");
        assert!(warnings.is_empty(), "{warnings:?}");
        let key = occurrence_key("primary", "e1");
        let entry = state
            .get(&(key, "2026-10-30".to_string()))
            .expect("row state");
        assert_eq!(entry.row.as_ref().expect("row").block_id, "interview");
        assert!(entry.link);
        assert!(!entry.gone);
        let gone = state
            .get(&(occurrence_key("primary", "e9"), "2026-10-30".to_string()))
            .expect("gone state");
        assert!(gone.gone);
        assert!(gone.row.is_none());
        assert_eq!(newest.expect("newest").1, host);
    }
}
