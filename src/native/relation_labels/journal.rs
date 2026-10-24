//! Relation label preference journal: `.bob/relations/<host>.jsonl`
//! (`docs/task-relations.md` §16, RV3).
//!
//! Events (`v: 1`):
//!
//! ```json
//! {"v":1,"ts":"2026-10-24T09:41:07-04:00","host":"mac","kind":"relation_label.hide","key":"implements","name":"implements","via":"bob-mac-capture"}
//! {"v":1,"ts":"2026-10-24T09:42:00-04:00","host":"mac","kind":"relation_label.unhide","key":"implements","name":"implements","via":"bob-mac-capture"}
//! {"v":1,"ts":"2026-10-24T10:02:11-04:00","host":"apollo","kind":"relation_label.rename","from":"inspired by","to":"prompted by","from_name":"inspired by","to_name":"prompted by","via":"bob-cli"}
//! ```
//!
//! The fold is exactly the theme rule, over the shared namespaced
//! module: read every `*.jsonl`, skip malformed lines with a bounded
//! warning, sort by `(ts instant, host, path, line)`, last writer wins
//! per key, and a rename moves the source key's explicit state unless
//! the target already has state. Bob is the only writer: appends
//! happen under `bob_sync.lock` with a 3 s wait, one `O_APPEND` write
//! per event, and the directory is created on first use.

#[cfg(test)]
use std::path::PathBuf;
use std::{collections::BTreeMap, io, path::Path};

use chrono::{DateTime, FixedOffset};
use serde_json::{json, Value};

use super::relation_label_key;
use crate::native::pref_journal::{self, StoredEvent};

const NAMESPACE: pref_journal::Namespace = pref_journal::RELATIONS;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum EventPayload {
    Hide {
        key: String,
        name: String,
    },
    Unhide {
        key: String,
        name: String,
    },
    Rename {
        from: String,
        to: String,
        from_name: String,
        to_name: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct JournalEvent {
    pub(crate) ts: DateTime<FixedOffset>,
    pub(crate) host: String,
    pub(crate) path: String,
    pub(crate) line: u32,
    pub(crate) payload: EventPayload,
}

/// Folded preferences: hidden state plus the stored display name, so
/// a hidden key with zero uses stays listable.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Prefs {
    by_key: BTreeMap<String, KeyPref>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct KeyPref {
    hidden: bool,
    name: String,
}

impl Prefs {
    pub(crate) fn hidden(&self, key: &str) -> bool {
        self.by_key.get(key).is_some_and(|pref| pref.hidden)
    }

    pub(crate) fn names(&self) -> impl Iterator<Item = (&String, &String)> {
        self.by_key.iter().map(|(key, pref)| (key, &pref.name))
    }
}

pub(crate) fn load_prefs(vault: &Path) -> io::Result<(Prefs, Vec<String>)> {
    let (events, warnings) = read_journal(vault)?;
    Ok((fold_events(&events), warnings))
}

pub(crate) fn host_label() -> String {
    pref_journal::host_label()
}

/// Tests locate the host file a write just appended.
#[cfg(test)]
pub(crate) fn host_journal_path(vault: &Path, host: &str) -> PathBuf {
    pref_journal::host_journal_path(vault, NAMESPACE, host)
}

pub(crate) fn event_timestamp() -> DateTime<FixedOffset> {
    pref_journal::event_timestamp()
}

pub(crate) fn serialize_event(
    ts: DateTime<FixedOffset>,
    host: &str,
    payload: &EventPayload,
    via: &str,
) -> String {
    let ts = ts.to_rfc3339_opts(chrono::SecondsFormat::Secs, false);
    let value = match payload {
        EventPayload::Hide { key, name } => json!({
            "v": 1,
            "ts": ts,
            "host": host,
            "kind": "relation_label.hide",
            "key": key,
            "name": name,
            "via": via,
        }),
        EventPayload::Unhide { key, name } => json!({
            "v": 1,
            "ts": ts,
            "host": host,
            "kind": "relation_label.unhide",
            "key": key,
            "name": name,
            "via": via,
        }),
        EventPayload::Rename {
            from,
            to,
            from_name,
            to_name,
        } => json!({
            "v": 1,
            "ts": ts,
            "host": host,
            "kind": "relation_label.rename",
            "from": from,
            "to": to,
            "from_name": from_name,
            "to_name": to_name,
            "via": via,
        }),
    };
    let mut line =
        serde_json::to_string(&value).unwrap_or_else(|_| "{}".to_string());
    line.push('\n');
    line
}

/// Append one journal event with a single `O_APPEND` write. Creates
/// `.bob/relations/` on first use.
pub(crate) fn append_event_at(
    vault: &Path,
    host: &str,
    payload: &EventPayload,
    via: &str,
    ts: DateTime<FixedOffset>,
) -> io::Result<String> {
    let line = serialize_event(ts, host, payload, via);
    pref_journal::append_line(vault, NAMESPACE, host, &line)?;
    Ok(line)
}

pub(crate) fn read_journal(
    vault: &Path,
) -> io::Result<(Vec<JournalEvent>, Vec<String>)> {
    let (stored, warnings) = pref_journal::read_raw(vault, NAMESPACE)?;
    let mut events = Vec::new();
    let mut malformed = 0usize;
    let mut warnings = warnings;
    for event in &stored {
        let Some(payload) = parse_payload(event) else {
            malformed += 1;
            if malformed <= pref_journal::MAX_MALFORMED_WARNINGS {
                warnings.push(format!(
                    "skipping malformed relation journal line {}:{}",
                    event.path, event.line
                ));
            }
            continue;
        };
        events.push(JournalEvent {
            ts: event.ts,
            host: event.host.clone(),
            path: event.path.clone(),
            line: event.line,
            payload,
        });
    }
    if malformed > pref_journal::MAX_MALFORMED_WARNINGS {
        warnings.push(format!(
            "skipped {malformed} malformed relation journal lines"
        ));
    }
    Ok((events, warnings))
}

#[cfg(test)]
pub(crate) fn parse_event(
    line: &str,
    path: &str,
    line_number: u32,
) -> Option<JournalEvent> {
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
    let stored = StoredEvent {
        ts,
        host,
        path: path.to_string(),
        line: line_number,
        kind,
        value,
    };
    let payload = parse_payload(&stored)?;
    Some(JournalEvent {
        ts: stored.ts,
        host: stored.host.clone(),
        path: stored.path.clone(),
        line: stored.line,
        payload,
    })
}

fn parse_payload(event: &StoredEvent) -> Option<EventPayload> {
    let object = event.value.as_object()?;
    match event.kind.as_str() {
        "relation_label.hide" => Some(EventPayload::Hide {
            key: required_key(object, "key")?,
            name: pref_journal::optional_name(object, "name"),
        }),
        "relation_label.unhide" => Some(EventPayload::Unhide {
            key: required_key(object, "key")?,
            name: pref_journal::optional_name(object, "name"),
        }),
        "relation_label.rename" => Some(EventPayload::Rename {
            from: required_key(object, "from")?,
            to: required_key(object, "to")?,
            from_name: pref_journal::optional_name(object, "from_name"),
            to_name: pref_journal::optional_name(object, "to_name"),
        }),
        _ => None,
    }
}

fn required_key(
    object: &serde_json::Map<String, Value>,
    field: &str,
) -> Option<String> {
    pref_journal::required_key(object, field, relation_label_key)
}

pub(crate) fn fold_events(events: &[JournalEvent]) -> Prefs {
    let mut ordered = events.iter().collect::<Vec<_>>();
    pref_journal::sort_by_writer_order(&mut ordered, |event| {
        (
            event.ts,
            event.host.as_str(),
            event.path.as_str(),
            event.line,
        )
    });

    let mut prefs = Prefs::default();
    for event in ordered {
        match &event.payload {
            EventPayload::Hide { key, name } => {
                let slot = prefs.by_key.entry(key.clone()).or_default();
                slot.hidden = true;
                if !name.is_empty() {
                    slot.name = name.clone();
                }
            }
            EventPayload::Unhide { key, name } => {
                let slot = prefs.by_key.entry(key.clone()).or_default();
                slot.hidden = false;
                if !name.is_empty() {
                    slot.name = name.clone();
                }
            }
            EventPayload::Rename {
                from, to, to_name, ..
            } => {
                pref_journal::move_state_for_rename(
                    &mut prefs.by_key,
                    from,
                    to,
                );
                // The target keeps its stored name unless the rename
                // supplies one and the target has no name yet.
                if !to_name.is_empty()
                    && let Some(slot) = prefs.by_key.get_mut(to)
                    && slot.name.is_empty()
                {
                    slot.name = to_name.clone();
                }
            }
        }
    }
    prefs
}
