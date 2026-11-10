//! The per-host `bob gcal sync` journal: `.bob/gcal/<host>.jsonl`.
//!
//! Records are `gcal.row`, `gcal.link`, `gcal.gone`, `gcal.series_root`,
//! `gcal.flag`, `gcal.reset`, and `gcal.note`, union-merged and folded
//! into state keyed by (occurrence key, day). Series roots fold into a
//! series-keyed map of learned `[root::]` values; notes fold into a
//! last-writer-wins book keyed by event or series; flags remember
//! once-reported lifecycle notices. One host-local status file under
//! `$XDG_STATE_HOME/bob/gcal/` feeds `list` and `doctor`; it is never
//! written to the vault.

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

/// Canonical text larger than this makes a `gcal.note` record malformed.
pub(crate) const MAX_NOTE_BYTES: usize = 2048;

/// Journaled state for one (occurrence key, day).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct OccurrenceState {
    pub row: Option<RowRecord>,
    pub link: bool,
    pub gone: bool,
    /// When the first `gcal.gone` tombstone was written; `None` while
    /// the occurrence is not gone.
    pub gone_at: Option<DateTime<FixedOffset>>,
    /// Why the first tombstone was written: `line_removed` for your
    /// deletion, or the Google-side lifecycle reason.
    pub gone_reason: Option<String>,
    /// Once-reported lifecycle notices (`kept_edit`, `kept_root`,
    /// `worked_kept`, `now_maybe`, `awaiting_reply`, …).
    pub flags: HashSet<String>,
}

/// Which private-note fields a `gcal.row` record carries. Additive:
/// older binaries read other fields with `object.get` and ignore it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NoteProvenance {
    pub hash: String,
    pub fields: Vec<String>,
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
    pub description: DescriptionState,
    pub note: Option<NoteProvenance>,
    pub ts: DateTime<FixedOffset>,
    pub host: String,
    pub path: String,
}

/// What Bob last wrote for one row's description bullets: a sibling of
/// `spans` in each `gcal.row` record. Older binaries read fields with
/// `object.get` and ignore it, so a downgrade is safe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DescriptionState {
    /// The row predates the feature, or was synced while `off`.
    Absent,
    /// What Bob last wrote: the hash over `(depth, body)` pairs (which
    /// ignores indent characters) and the bullet line count.
    Tracked { hash: String, lines: usize },
    /// Never touched: `null`, or a malformed value some other writer left.
    HandsOff,
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

/// Whether a private event note covers one occurrence or the series.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NoteScope {
    Event,
    Series,
}

impl NoteScope {
    #[allow(dead_code)] // The write command serializes this in a later phase.
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Event => "event",
            Self::Series => "series",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        match value {
            "event" => Some(Self::Event),
            "series" => Some(Self::Series),
            _ => None,
        }
    }
}

/// Display-only identity captured when a note was written.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct NoteSeen {
    pub title: Option<String>,
    pub start: Option<String>,
    pub ical_uid: Option<String>,
    pub original_start: Option<String>,
}

/// One folded private event note. `text: ""` with `cleared` is a
/// deliberate clear, distinct from a key that was never noted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FoldedNote {
    pub account: String,
    pub scope: NoteScope,
    pub calendar_id: String,
    pub key: String,
    pub text: String,
    pub cleared: bool,
    pub seen: Option<NoteSeen>,
    pub host: String,
    pub ts: DateTime<FixedOffset>,
}

/// Last-writer-wins fold of `gcal.note` records.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct NoteBook {
    events: BTreeMap<(String, String), FoldedNote>,
    series: BTreeMap<(String, String), FoldedNote>,
}

impl NoteBook {
    pub(crate) fn event(
        &self,
        calendar_id: &str,
        event_id: &str,
    ) -> Option<&FoldedNote> {
        self.events
            .get(&(calendar_id.to_string(), event_id.to_string()))
    }

    pub(crate) fn series(
        &self,
        calendar_id: &str,
        series: &str,
    ) -> Option<&FoldedNote> {
        self.series
            .get(&(calendar_id.to_string(), series.to_string()))
    }

    /// Raw event-scope record, including clears and foreign accounts.
    /// Hold logic decides whether it counts.
    pub(crate) fn event_record(
        &self,
        calendar_id: &str,
        event_id: &str,
    ) -> Option<&FoldedNote> {
        self.event(calendar_id, event_id)
    }

    /// Raw series-scope record, including clears and foreign accounts.
    pub(crate) fn series_record(
        &self,
        calendar_id: &str,
        series: &str,
    ) -> Option<&FoldedNote> {
        self.series(calendar_id, series)
    }
}

/// A new `gcal.note` to append.
#[allow(dead_code)] // The write command constructs this in a later phase.
#[derive(Debug, Clone)]
pub(crate) struct NewNote {
    pub account: String,
    pub calendar_id: String,
    pub event_id: Option<String>,
    pub series: Option<String>,
    pub scope: NoteScope,
    pub text: String,
    pub seen: Option<NoteSeen>,
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
    pub description: DescriptionState,
    pub note: Option<NoteProvenance>,
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
        if !is_gcal
            || event.kind == "gcal.series_root"
            || event.kind == "gcal.note"
        {
            continue;
        }
        newest = Some((event.ts, event.host.clone()));
        if event.kind == "gcal.reset" {
            if let Some(day) = parse_reset(event) {
                state.retain(|(_, state_day), _| state_day != day);
            } else {
                malformed += 1;
                if malformed <= pref_journal::MAX_MALFORMED_WARNINGS {
                    warnings.push(format!(
                        "skipping malformed gcal journal line {}:{}",
                        event.path, event.line
                    ));
                }
            }
            continue;
        }
        match parse_record(event) {
            Some((key, day, update)) => {
                let entry = state.entry((key, day)).or_default();
                update.apply(entry, event.ts);
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
    Gone(Option<String>),
    Flag(String),
}

impl Update {
    /// Fold one record in writer order. A `gcal.row` after a `gcal.gone`
    /// revives the occurrence (a pasted-back row is managed again); a
    /// later tombstone starts a fresh `gone_at`. Repeat tombstones keep
    /// the first one's time and reason.
    fn apply(self, state: &mut OccurrenceState, ts: DateTime<FixedOffset>) {
        match self {
            Self::Row(row) => {
                state.row = Some(row);
                state.gone = false;
                state.gone_at = None;
                state.gone_reason = None;
            }
            Self::Link => state.link = true,
            Self::Gone(reason) => {
                if !state.gone {
                    state.gone_at = Some(ts);
                    state.gone_reason = reason;
                }
                state.gone = true;
            }
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
            description: parse_description_state(object.get("description")),
            note: parse_note_provenance(object.get("note")),
            ts: event.ts,
            host: event.host.clone(),
            path: object
                .get("path")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
        }),
        "gcal.link" => Update::Link,
        "gcal.gone" => Update::Gone(
            object
                .get("reason")
                .and_then(Value::as_str)
                .map(str::to_string),
        ),
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

fn parse_reset(event: &StoredEvent) -> Option<&str> {
    if event.kind != "gcal.reset" {
        return None;
    }
    let object = event.value.as_object()?;
    if object.get("reason").and_then(Value::as_str) != Some("force") {
        return None;
    }
    object.get("day")?.as_str()
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

/// Fold every `gcal.note` record into a last-writer-wins book, ordered
/// by `(ts, host, path, line)` like [`load_series_roots`]. Malformed
/// lines are skipped with a warning; callers that need the count use
/// [`malformed_count`].
pub(crate) fn load_notes(
    vault: &Path,
) -> std::io::Result<(NoteBook, Vec<String>)> {
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
    let mut book = NoteBook::default();
    let mut malformed = 0usize;
    for event in ordered {
        if event.kind.as_str() != "gcal.note" {
            continue;
        }
        match parse_note_record(event) {
            Some(note) => {
                let key = (note.calendar_id.clone(), note.key.clone());
                match note.scope {
                    NoteScope::Event => {
                        book.events.insert(key, note);
                    }
                    NoteScope::Series => {
                        book.series.insert(key, note);
                    }
                }
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
    Ok((book, warnings))
}

/// Append one `gcal.note` record with a single `O_APPEND` write.
#[allow(dead_code)] // The write command appends through this in a later phase.
pub(crate) fn append_note(
    vault: &Path,
    host: &str,
    note: &NewNote,
) -> std::io::Result<String> {
    let mut key = json!({"calendar_id": note.calendar_id});
    match note.scope {
        NoteScope::Event => {
            key["event_id"] = json!(note.event_id.as_deref().unwrap_or(""));
        }
        NoteScope::Series => {
            key["series"] = json!(note.series.as_deref().unwrap_or(""));
        }
    }
    let mut value = json!({
        "v": 1,
        "ts": pref_journal::event_timestamp()
            .to_rfc3339_opts(chrono::SecondsFormat::Secs, false),
        "host": host,
        "kind": "gcal.note",
        "account": note.account,
        "key": key,
        "scope": note.scope.as_str(),
        "text": note.text,
    });
    if let Some(seen) = &note.seen {
        value["seen"] = json!({
            "title": seen.title,
            "start": seen.start,
            "ical_uid": seen.ical_uid,
            "original_start": seen.original_start,
        });
    }
    let line = serde_json::to_string(&value)
        .unwrap_or_else(|_| "{}".to_string())
        + "\n";
    pref_journal::append_line(vault, NAMESPACE, host, &line)?;
    Ok(line)
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
                && parse_reset(&event).is_none()
                && parse_note_record(&event).is_none()
            {
                count += 1;
            }
        }
    }
    count
}

/// Parse one `gcal.note` record without folding it. Text larger than
/// [`MAX_NOTE_BYTES`], an unknown scope, or a missing key part is
/// malformed.
fn parse_note_record(event: &StoredEvent) -> Option<FoldedNote> {
    if event.kind.as_str() != "gcal.note" {
        return None;
    }
    let object = event.value.as_object()?;
    let account = object.get("account")?.as_str()?.to_string();
    let scope = NoteScope::parse(object.get("scope")?.as_str()?)?;
    let key = object.get("key")?.as_object()?;
    let calendar_id = key.get("calendar_id")?.as_str()?.to_string();
    if calendar_id.is_empty() {
        return None;
    }
    let key_part = match scope {
        NoteScope::Event => key.get("event_id")?.as_str()?.to_string(),
        NoteScope::Series => key.get("series")?.as_str()?.to_string(),
    };
    if key_part.is_empty() {
        return None;
    }
    let text = object.get("text")?.as_str()?;
    if text.len() > MAX_NOTE_BYTES {
        return None;
    }
    let seen =
        object
            .get("seen")
            .and_then(Value::as_object)
            .map(|seen| NoteSeen {
                title: seen
                    .get("title")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                start: seen
                    .get("start")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                ical_uid: seen
                    .get("ical_uid")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                original_start: seen
                    .get("original_start")
                    .and_then(Value::as_str)
                    .map(str::to_string),
            });
    Some(FoldedNote {
        account,
        scope,
        calendar_id,
        key: key_part,
        text: text.to_string(),
        cleared: text.is_empty(),
        seen,
        host: event.host.clone(),
        ts: event.ts,
    })
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

/// Parse one `gcal.row` note provenance record: absent when missing or
/// `null`, with a `{hash, fields}` object otherwise. Any malformed value
/// parses as absent so the row stays valid; older binaries ignore it.
fn parse_note_provenance(value: Option<&Value>) -> Option<NoteProvenance> {
    let value = value?;
    if value.is_null() {
        return None;
    }
    let object = value.as_object()?;
    let hash = object.get("hash").and_then(Value::as_str)?;
    if hash.len() != 16 || !hash.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let fields = object.get("fields").and_then(Value::as_array)?;
    let mut out = Vec::new();
    for field in fields {
        let name = field.as_str()?;
        if !matches!(name, "root" | "clock" | "before" | "after" | "bullets") {
            return None;
        }
        if !out.contains(&name.to_string()) {
            out.push(name.to_string());
        }
    }
    if out.is_empty() {
        return None;
    }
    Some(NoteProvenance {
        hash: hash.to_string(),
        fields: out,
    })
}

/// Parse one `gcal.row` description record: absent, a `{hash, lines}`
/// object, or `null`. Any malformed value parses as hands-off (never
/// touched) rather than as absent, so the row stays valid and never
/// triggers a backfill.
fn parse_description_state(value: Option<&Value>) -> DescriptionState {
    let Some(value) = value else {
        return DescriptionState::Absent;
    };
    if value.is_null() {
        return DescriptionState::HandsOff;
    }
    let object = value.as_object();
    let hash = object
        .and_then(|object| object.get("hash"))
        .and_then(Value::as_str);
    let lines = object
        .and_then(|object| object.get("lines"))
        .and_then(Value::as_u64);
    match (hash, lines) {
        (Some(hash), Some(lines)) => DescriptionState::Tracked {
            hash: hash.to_string(),
            lines: usize::try_from(lines).unwrap_or(usize::MAX),
        },
        _ => DescriptionState::HandsOff,
    }
}

/// Append one `gcal.row` record with a single `O_APPEND` write.
pub(crate) fn append_row(
    vault: &Path,
    host: &str,
    row: &NewRow,
) -> std::io::Result<String> {
    let mut value = json!({
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
    });
    match &row.description {
        DescriptionState::Absent => {}
        DescriptionState::HandsOff => {
            value["description"] = Value::Null;
        }
        DescriptionState::Tracked { hash, lines } => {
            value["description"] = json!({"hash": hash, "lines": lines});
        }
    }
    if let Some(note) = &row.note {
        value["note"] = json!({"hash": note.hash, "fields": note.fields});
    }
    let line = serde_json::to_string(&value)
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

/// Append a key-less reset marker that clears one day's folded sync state.
pub(crate) fn append_reset(
    vault: &Path,
    host: &str,
    day: &str,
) -> std::io::Result<String> {
    let value = json!({
        "v": 1,
        "ts": pref_journal::event_timestamp()
            .to_rfc3339_opts(chrono::SecondsFormat::Secs, false),
        "host": host,
        "kind": "gcal.reset",
        "day": day,
        "reason": "force",
    });
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
    pub links_refiled: usize,
    pub error: Option<String>,
    /// Waiting reason code for a `waiting` outcome (additive).
    pub reason: Option<String>,
    /// Whether the last sync explicitly reset the day.
    pub forced: bool,
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
        "links_refiled": status.links_refiled,
        "error": status.error,
        "reason": status.reason,
        "forced": status.forced,
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
        links_refiled: object
            .get("links_refiled")
            .and_then(Value::as_u64)
            .unwrap_or(0) as usize,
        error: object
            .get("error")
            .and_then(Value::as_str)
            .map(str::to_string),
        reason: object
            .get("reason")
            .and_then(Value::as_str)
            .map(str::to_string),
        forced: object
            .get("forced")
            .and_then(Value::as_bool)
            .unwrap_or(false),
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
                description: DescriptionState::Absent,
                note: None,
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

    #[test]
    fn reset_clears_only_its_day_and_later_records_survive() {
        let dir = tempfile::tempdir().expect("vault");
        let host = "mac";
        let append_row_for = |day: &str, block_id: &str| {
            append_row(
                dir.path(),
                host,
                &NewRow {
                    calendar_id: "primary".to_string(),
                    event_id: "e1".to_string(),
                    ical_uid: None,
                    series_id: None,
                    day: day.to_string(),
                    path: "2026/20261030.md".to_string(),
                    block_id: block_id.to_string(),
                    url: None,
                    row_kind: "task".to_string(),
                    spans: BTreeMap::new(),
                    root: None,
                    source: None,
                    description: DescriptionState::Absent,
                    note: None,
                },
            )
            .expect("row")
        };
        append_row_for("2026-10-30", "old");
        append_mark(
            dir.path(),
            host,
            "gcal.link",
            &NewMark {
                calendar_id: "primary".to_string(),
                event_id: "e1".to_string(),
                day: "2026-10-30".to_string(),
                block_id: Some("old".to_string()),
                reason: None,
            },
        )
        .expect("link before reset");
        append_mark(
            dir.path(),
            host,
            "gcal.flag",
            &NewMark {
                calendar_id: "primary".to_string(),
                event_id: "e1".to_string(),
                day: "2026-10-30".to_string(),
                block_id: None,
                reason: Some("kept_edit".to_string()),
            },
        )
        .expect("flag before reset");
        append_row_for("2026-10-31", "next-day");
        append_series_root(
            dir.path(),
            host,
            "primary",
            "series",
            "body",
            "2026-10-30",
            "old",
        )
        .expect("series root");

        append_reset(dir.path(), host, "2026-10-30").expect("reset");
        append_row_for("2026-10-30", "restored");
        append_mark(
            dir.path(),
            host,
            "gcal.link",
            &NewMark {
                calendar_id: "primary".to_string(),
                event_id: "e1".to_string(),
                day: "2026-10-30".to_string(),
                block_id: Some("restored".to_string()),
                reason: None,
            },
        )
        .expect("link after reset");

        let (state, warnings, _) = load_state(dir.path()).expect("fold");
        assert!(warnings.is_empty(), "{warnings:?}");
        let day = state
            .get(&(occurrence_key("primary", "e1"), "2026-10-30".to_string()))
            .expect("day state after reset");
        assert_eq!(day.row.as_ref().expect("row").block_id, "restored");
        assert!(day.link);
        assert!(day.flags.is_empty());
        let next_day = state
            .get(&(occurrence_key("primary", "e1"), "2026-10-31".to_string()))
            .expect("other day remains");
        assert_eq!(next_day.row.as_ref().expect("row").block_id, "next-day");
        assert_eq!(load_series_roots(dir.path()).expect("series").len(), 1);
        assert_eq!(malformed_count(dir.path()), 0);
    }

    fn raw_record(dir: &Path, ts: &str, kind: &str, extra: serde_json::Value) {
        let mut value = json!({
            "v": 1,
            "ts": ts,
            "host": "mac",
            "kind": kind,
            "key": {"calendar_id": "primary", "event_id": "e1"},
            "day": "2026-10-30",
        });
        for (name, field) in extra.as_object().expect("object") {
            value[name] = field.clone();
        }
        let line = serde_json::to_string(&value).expect("json") + "\n";
        pref_journal::append_line(dir, NAMESPACE, "mac", &line)
            .expect("append");
    }

    fn row_extra() -> serde_json::Value {
        json!({
            "block_id": "haircut",
            "row": "task",
            "spans": {"title": span_hash("Haircut")},
            "path": "2026/20261030.md",
        })
    }

    fn folded(dir: &Path) -> OccurrenceState {
        let (state, warnings, _) = load_state(dir).expect("fold");
        assert!(warnings.is_empty(), "{warnings:?}");
        state
            .get(&(occurrence_key("primary", "e1"), "2026-10-30".to_string()))
            .cloned()
            .expect("occurrence state")
    }

    #[test]
    fn tombstone_keeps_its_first_time_and_reason() {
        let dir = tempfile::tempdir().expect("vault");
        raw_record(
            dir.path(),
            "2026-10-30T05:24:20-04:00",
            "gcal.row",
            row_extra(),
        );
        raw_record(
            dir.path(),
            "2026-10-30T05:28:01-04:00",
            "gcal.gone",
            json!({"reason": "line_removed"}),
        );
        raw_record(
            dir.path(),
            "2026-10-30T05:40:00-04:00",
            "gcal.gone",
            json!({"reason": "Declined in Google Calendar"}),
        );
        let entry = folded(dir.path());
        assert!(entry.gone);
        assert_eq!(entry.gone_reason.as_deref(), Some("line_removed"));
        assert_eq!(
            entry
                .gone_at
                .expect("first tombstone")
                .format("%H:%M")
                .to_string(),
            "05:28"
        );
        assert!(entry.row.is_some(), "the tombstone keeps the row record");
    }

    #[test]
    fn row_after_gone_revives_the_occurrence() {
        let dir = tempfile::tempdir().expect("vault");
        raw_record(
            dir.path(),
            "2026-10-30T05:24:20-04:00",
            "gcal.row",
            row_extra(),
        );
        raw_record(
            dir.path(),
            "2026-10-30T05:28:01-04:00",
            "gcal.gone",
            json!({"reason": "line_removed"}),
        );
        raw_record(
            dir.path(),
            "2026-10-30T06:00:00-04:00",
            "gcal.row",
            row_extra(),
        );
        let entry = folded(dir.path());
        assert!(!entry.gone, "a later row revives the occurrence");
        assert_eq!(entry.gone_at, None);
        assert_eq!(entry.gone_reason, None);
        assert_eq!(entry.row.as_ref().expect("row").block_id, "haircut");
    }

    #[test]
    fn gone_after_row_stays_gone_and_a_later_tombstone_restarts() {
        let dir = tempfile::tempdir().expect("vault");
        raw_record(
            dir.path(),
            "2026-10-30T05:24:20-04:00",
            "gcal.row",
            row_extra(),
        );
        raw_record(
            dir.path(),
            "2026-10-30T05:28:01-04:00",
            "gcal.gone",
            json!({"reason": "line_removed"}),
        );
        assert!(folded(dir.path()).gone);
        // Revived, then removed again: the clock restarts.
        raw_record(
            dir.path(),
            "2026-10-30T06:00:00-04:00",
            "gcal.row",
            row_extra(),
        );
        raw_record(
            dir.path(),
            "2026-10-30T07:15:00-04:00",
            "gcal.gone",
            json!({"reason": "line_removed"}),
        );
        let entry = folded(dir.path());
        assert!(entry.gone);
        assert_eq!(
            entry
                .gone_at
                .expect("second tombstone")
                .format("%H:%M")
                .to_string(),
            "07:15"
        );
    }

    fn note_record(dir: &Path, host: &str, ts: &str, extra: serde_json::Value) {
        let mut value = json!({
            "v": 1,
            "ts": ts,
            "host": host,
            "kind": "gcal.note",
            "account": "you@example.com",
            "key": {"calendar_id": "primary", "event_id": "deerwood"},
            "scope": "event",
            "text": "@family",
        });
        for (name, field) in extra.as_object().expect("object") {
            value[name] = field.clone();
        }
        let line = serde_json::to_string(&value).expect("json") + "\n";
        pref_journal::append_line(dir, NAMESPACE, host, &line).expect("append");
    }

    #[test]
    fn notes_fold_last_writer_wins_and_clears() {
        let dir = tempfile::tempdir().expect("vault");
        note_record(
            dir.path(),
            "athena",
            "2026-11-09T10:00:00-05:00",
            json!({"text": "@job"}),
        );
        note_record(
            dir.path(),
            "mac",
            "2026-11-09T11:00:00-05:00",
            json!({"text": "@family\n-30 Drive"}),
        );
        let (book, warnings) = load_notes(dir.path()).expect("notes");
        assert!(warnings.is_empty(), "{warnings:?}");
        let note = book.event("primary", "deerwood").expect("event note");
        assert_eq!(note.text, "@family\n-30 Drive");
        assert!(!note.cleared);
        assert_eq!(note.host, "mac");

        note_record(
            dir.path(),
            "mac",
            "2026-11-09T12:00:00-05:00",
            json!({"text": ""}),
        );
        let (book, warnings) = load_notes(dir.path()).expect("cleared");
        assert!(warnings.is_empty(), "{warnings:?}");
        let note = book.event("primary", "deerwood").expect("cleared note");
        assert!(note.cleared);
        assert!(note.text.is_empty());
    }

    #[test]
    fn notes_do_not_move_newest_or_count_as_malformed() {
        let dir = tempfile::tempdir().expect("vault");
        raw_record(
            dir.path(),
            "2026-10-30T05:24:20-04:00",
            "gcal.row",
            row_extra(),
        );
        note_record(dir.path(), "mac", "2026-10-30T06:00:00-04:00", json!({}));
        let (state, warnings, newest) = load_state(dir.path()).expect("fold");
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(
            newest.expect("newest").0.format("%H:%M").to_string(),
            "05:24"
        );
        assert!(state
            .get(&(
                occurrence_key("primary", "deerwood"),
                "2026-10-30".to_string()
            ))
            .is_none());
        assert_eq!(malformed_count(dir.path()), 0);
    }

    #[test]
    fn oversized_or_unknown_scope_note_is_malformed() {
        let dir = tempfile::tempdir().expect("vault");
        note_record(
            dir.path(),
            "mac",
            "2026-10-30T06:00:00-04:00",
            json!({"scope": "this-and-following"}),
        );
        note_record(
            dir.path(),
            "mac",
            "2026-10-30T06:01:00-04:00",
            json!({"text": "x".repeat(MAX_NOTE_BYTES + 1)}),
        );
        let (book, warnings) = load_notes(dir.path()).expect("notes");
        assert!(book.event("primary", "deerwood").is_none());
        assert!(!warnings.is_empty(), "{warnings:?}");
        assert_eq!(malformed_count(dir.path()), 2);
    }
}
