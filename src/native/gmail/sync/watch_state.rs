//! Body-free, cross-host journal state for Gmail watched-thread turns.
//!
//! `gmail.turn` is the applied-message marker. It is deliberately ordered by
//! Gmail's `(internal_date, message_id)` identity, while fetched messages are
//! still ordered by [`super::super::turn::fold`]'s Gmail thread order.

use std::{
    collections::{BTreeMap, BTreeSet},
    io,
    path::Path,
};

use chrono::{DateTime, FixedOffset};
use serde_json::{json, Map, Value};

use super::{super::turn, ThreadKey};
use crate::native::{
    gmail::journal::NAMESPACE,
    pref_journal::{self, StoredEvent, MAX_MALFORMED_WARNINGS},
};

const TURN_KIND: &str = "gmail.turn";
const GONE_KIND: &str = "gmail.gone";

/// The effect encoded by an applied turn event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AppliedEffect {
    Hold,
    Lift,
    Bounced,
}

impl AppliedEffect {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Hold => "hold",
            Self::Lift => "lift",
            Self::Bounced => "bounced",
        }
    }

    pub(crate) fn parse(value: &str) -> Option<Self> {
        match value {
            "hold" => Some(Self::Hold),
            "lift" => Some(Self::Lift),
            "bounced" => Some(Self::Bounced),
            _ => None,
        }
    }
}

/// One successfully consumed counted move. The values are body-free and
/// safe to persist in a synced per-host Gmail journal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AppliedTurn {
    pub(crate) key: ThreadKey,
    pub(crate) message_id: String,
    pub(crate) internal_date: i64,
    pub(crate) turn: turn::Turn,
    pub(crate) effect: AppliedEffect,
}

/// Folded applied markers and one-time gone reports from every host journal.
#[derive(Debug, Clone, Default)]
pub(crate) struct WatchJournal {
    markers: BTreeMap<ThreadKey, AppliedTurn>,
    marker_events: BTreeMap<ThreadKey, StoredEvent>,
    gone: BTreeSet<ThreadKey>,
    warnings: Vec<String>,
}

impl WatchJournal {
    /// Read and fold all `.bob/gmail/*.jsonl` files. Malformed Gmail watch
    /// events are skipped with bounded, body-free warnings.
    pub(crate) fn load(vault: &Path) -> io::Result<Self> {
        let (events, warnings) = pref_journal::read_raw(vault, NAMESPACE)?;
        Ok(Self::from_events(events, warnings))
    }

    fn from_events(
        events: Vec<StoredEvent>,
        mut warnings: Vec<String>,
    ) -> Self {
        let mut result = Self {
            warnings: std::mem::take(&mut warnings),
            ..Self::default()
        };
        let mut malformed = 0usize;
        for event in events {
            match event.kind.as_str() {
                TURN_KIND => match parse_turn(&event) {
                    Some(turn) => {
                        let key = turn.key.clone();
                        let replace =
                            result.markers.get(&key).is_none_or(|current| {
                                marker_order(&turn) > marker_order(current)
                                    || (marker_order(&turn)
                                        == marker_order(current)
                                        && result
                                            .marker_events
                                            .get(&key)
                                            .is_none_or(|old| {
                                                event_order(&event)
                                                    >= event_order(old)
                                            }))
                            });
                        if replace {
                            result.markers.insert(key.clone(), turn);
                            result.marker_events.insert(key, event);
                        }
                    }
                    None => malformed += 1,
                },
                GONE_KIND => match parse_gone(&event) {
                    Some(key) => {
                        result.gone.insert(key);
                    }
                    None => malformed += 1,
                },
                _ => continue,
            }
        }
        if malformed > 0 {
            for _ in 0..malformed.min(MAX_MALFORMED_WARNINGS) {
                result.warnings.push(
                    "skipping malformed Gmail watch journal event".to_string(),
                );
            }
            if malformed > MAX_MALFORMED_WARNINGS {
                result.warnings.push(format!(
                    "skipped {malformed} malformed Gmail watch journal events"
                ));
            }
        }
        result
    }

    /// Latest successfully applied counted message for this thread.
    pub(crate) fn marker(&self, key: &ThreadKey) -> Option<&AppliedTurn> {
        self.markers.get(key)
    }

    /// A `gmail.gone` event has already been recorded on any host.
    pub(crate) fn was_reported_gone(&self, key: &ThreadKey) -> bool {
        self.gone.contains(key)
    }

    /// Body-free journal warnings.
    pub(crate) fn warnings(&self) -> &[String] {
        &self.warnings
    }

    /// Append a consumed turn after its task and note writes are durable.
    pub(crate) fn append_turn(
        vault: &Path,
        host: &str,
        event: &AppliedTurn,
    ) -> io::Result<()> {
        let value = json!({
            "account": event.key.account(),
            "thread_id": event.key.thread_id(),
            "message_id": event.message_id,
            "internal_date": event.internal_date,
            "turn": event.turn.as_str(),
            "effect": event.effect.as_str(),
        });
        append_event(vault, host, TURN_KIND, value)
    }

    /// Append the one-time missing/trashed report after local application.
    /// The caller checks [`was_reported_gone`](Self::was_reported_gone) while
    /// holding the transaction lock before invoking this method.
    pub(crate) fn append_gone(
        vault: &Path,
        host: &str,
        key: &ThreadKey,
    ) -> io::Result<()> {
        let mut value = Map::new();
        value.insert("account".into(), json!(key.account()));
        value.insert("thread_id".into(), json!(key.thread_id()));
        append_event(vault, host, GONE_KIND, Value::Object(value))
    }
}

// The exact writer event is only a deterministic tie-break when two hosts
// recorded the same Gmail message marker. Gmail date and message ID remain
// the primary ordering requested by the watch contract.
fn marker_order(event: &AppliedTurn) -> (i64, &str) {
    (event.internal_date, event.message_id.as_str())
}

fn event_order(
    event: &StoredEvent,
) -> (DateTime<FixedOffset>, &str, &str, u32) {
    (
        event.ts,
        event.host.as_str(),
        event.path.as_str(),
        event.line,
    )
}

fn parse_turn(event: &StoredEvent) -> Option<AppliedTurn> {
    let value = event.value.get("value")?.as_object()?;
    let key = parse_key(value)?;
    let message_id = value.get("message_id")?.as_str()?;
    if message_id.is_empty()
        || message_id.len() > 128
        || !message_id.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return None;
    }
    let internal_date = value.get("internal_date")?.as_i64()?;
    if internal_date < 0 {
        return None;
    }
    let turn = match value.get("turn")?.as_str()? {
        "yours" => turn::Turn::Yours,
        "theirs" => turn::Turn::Theirs,
        _ => return None,
    };
    let effect = AppliedEffect::parse(value.get("effect")?.as_str()?)?;
    Some(AppliedTurn {
        key,
        message_id: message_id.to_ascii_lowercase(),
        internal_date,
        turn,
        effect,
    })
}

fn parse_gone(event: &StoredEvent) -> Option<ThreadKey> {
    let value = event.value.get("value")?.as_object()?;
    parse_key(value)
}

fn parse_key(value: &Map<String, Value>) -> Option<ThreadKey> {
    let thread_id = value.get("thread_id")?.as_str()?;
    if thread_id.len() > 128 {
        return None;
    }
    ThreadKey::new(value.get("account")?.as_str()?, thread_id)
}

fn append_event(
    vault: &Path,
    host: &str,
    kind: &str,
    value: Value,
) -> io::Result<()> {
    let ts = pref_journal::event_timestamp()
        .to_rfc3339_opts(chrono::SecondsFormat::Secs, false);
    let mut line = serde_json::to_string(&json!({
        "v": 1,
        "ts": ts,
        "host": host,
        "kind": kind,
        "value": value,
    }))
    .map_err(io::Error::other)?;
    line.push('\n');
    pref_journal::append_line(vault, NAMESPACE, host, &line)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};

    fn event(host: &str, line: u32, kind: &str, value: Value) -> StoredEvent {
        StoredEvent {
            ts: Utc
                .timestamp_opt(i64::from(line), 0)
                .unwrap()
                .fixed_offset(),
            host: host.to_string(),
            path: format!("{host}.jsonl"),
            line,
            kind: kind.to_string(),
            value: json!({"value": value}),
        }
    }

    fn turn_event(host: &str, message: &str, date: i64) -> StoredEvent {
        event(
            host,
            1,
            TURN_KIND,
            json!({
                "account": "Me@example.com",
                "thread_id": "AA01",
                "message_id": message,
                "internal_date": date,
                "turn": "theirs",
                "effect": "hold",
            }),
        )
    }

    #[test]
    fn folds_markers_across_hosts_by_gmail_date_then_message_id() {
        let state = WatchJournal::from_events(
            vec![
                turn_event("mac", "aa02", 10),
                turn_event("athena", "aa01", 10),
                turn_event("apollo", "aa00", 9),
            ],
            Vec::new(),
        );
        let key = ThreadKey::new("me@example.com", "aa01").unwrap();
        assert_eq!(state.marker(&key).unwrap().message_id, "aa02");
    }

    #[test]
    fn skips_malformed_watch_events_with_bounded_warnings() {
        let events = (0..(MAX_MALFORMED_WARNINGS + 2))
            .map(|line| {
                event(
                    "athena",
                    line as u32 + 1,
                    TURN_KIND,
                    json!({"account": [], "thread_id": "not-hex"}),
                )
            })
            .collect();
        let state = WatchJournal::from_events(events, Vec::new());
        assert_eq!(state.markers.len(), 0);
        assert_eq!(state.warnings.len(), MAX_MALFORMED_WARNINGS + 1);
        assert!(state.warnings.last().unwrap().contains("malformed"));
    }

    #[test]
    fn folds_gone_once_per_thread_across_hosts() {
        let key = ThreadKey::new("me@example.com", "aa01").unwrap();
        let state = WatchJournal::from_events(
            vec![
                event(
                    "mac",
                    1,
                    GONE_KIND,
                    json!({"account": "ME@example.com", "thread_id": "aa01"}),
                ),
                event(
                    "athena",
                    1,
                    GONE_KIND,
                    json!({"account": "me@example.com", "thread_id": "AA01"}),
                ),
            ],
            Vec::new(),
        );
        assert!(state.was_reported_gone(&key));
        assert_eq!(state.gone.len(), 1);
    }

    #[test]
    fn appends_body_free_turn_marker_for_cross_host_readback() {
        let vault = tempfile::tempdir().unwrap();
        let key = ThreadKey::new("me@example.com", "aa01").unwrap();
        let applied = AppliedTurn {
            key: key.clone(),
            message_id: "aa02".to_string(),
            internal_date: 20,
            turn: turn::Turn::Theirs,
            effect: AppliedEffect::Hold,
        };
        WatchJournal::append_turn(vault.path(), "testhost", &applied).unwrap();
        let state = WatchJournal::load(vault.path()).unwrap();
        assert_eq!(state.marker(&key), Some(&applied));
        let journal = std::fs::read_to_string(pref_journal::host_journal_path(
            vault.path(),
            NAMESPACE,
            "testhost",
        ))
        .unwrap();
        assert!(journal.contains("\"kind\":\"gmail.turn\""));
        assert!(!journal.contains("snippet"));
        assert!(!journal.contains("body"));
    }
}
