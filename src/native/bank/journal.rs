//! Per-host bank journal: `.bob/bank/<host>.jsonl`.

use std::{collections::BTreeMap, fs, io, path::Path};

use chrono::{DateTime, FixedOffset, NaiveDate};
use serde_json::{json, Map, Value};

use crate::native::pref_journal::{
    self, parse_envelope, sort_by_writer_order, StoredEvent,
};

use super::{
    config::is_alias,
    identity::{mask_digits, merchant_key},
    source::SourceKind,
};

pub(crate) const NAMESPACE: pref_journal::Namespace = pref_journal::Namespace {
    dir: ".bob/bank",
    noun: "bank",
};

const IMPORT_KIND: &str = "bank.import";
const TXN_KIND: &str = "bank.txn";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TxnOutcome {
    Written,
    Matched,
    BeforeStart,
}

impl TxnOutcome {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Written => "written",
            Self::Matched => "matched",
            Self::BeforeStart => "before_start",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        match value {
            "written" => Some(Self::Written),
            "matched" => Some(Self::Matched),
            "before_start" => Some(Self::BeforeStart),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ImportCounts {
    pub(crate) new: u32,
    pub(crate) matched: u32,
    pub(crate) seen: u32,
    pub(crate) ambiguous: u32,
    pub(crate) before_start: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ImportEvent {
    pub(crate) ts: DateTime<FixedOffset>,
    pub(crate) host: String,
    pub(crate) path: String,
    pub(crate) line: u32,
    pub(crate) account: String,
    pub(crate) source: SourceKind,
    pub(crate) file_sha256: String,
    pub(crate) start: NaiveDate,
    pub(crate) end: NaiveDate,
    pub(crate) balance_cents: Option<i64>,
    pub(crate) balance_asof: Option<NaiveDate>,
    pub(crate) counts: ImportCounts,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TxnEvent {
    pub(crate) ts: DateTime<FixedOffset>,
    pub(crate) host: String,
    pub(crate) path: String,
    pub(crate) line: u32,
    pub(crate) account: String,
    pub(crate) key: String,
    pub(crate) id: String,
    pub(crate) posted: NaiveDate,
    pub(crate) cents: i64,
    pub(crate) desc: String,
    pub(crate) merchant: String,
    pub(crate) outcome: TxnOutcome,
    pub(crate) note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MalformedLine {
    pub(crate) path: String,
    pub(crate) line: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct JournalContradiction {
    pub(crate) key: String,
    pub(crate) field: &'static str,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct JournalSnapshot {
    pub(crate) imports: Vec<ImportEvent>,
    pub(crate) transactions: BTreeMap<String, TxnEvent>,
    pub(crate) ids: BTreeMap<String, String>,
    pub(crate) contradictions: Vec<JournalContradiction>,
    pub(crate) malformed: Vec<MalformedLine>,
    pub(crate) preimages: BTreeMap<String, Vec<u8>>,
    /// Host journal filenames observed while loading the directory.
    pub(crate) directory_entries: Vec<String>,
    /// Read failures make import unsafe because an unseen identity could be
    /// present in the unreadable host file.
    pub(crate) io_errors: Vec<String>,
}

pub(crate) fn load(vault: &Path) -> io::Result<JournalSnapshot> {
    let directory = pref_journal::journal_dir(vault, NAMESPACE);
    let entries = match fs::read_dir(&directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(JournalSnapshot::default());
        }
        Err(error) => return Err(error),
    };
    let mut paths = Vec::new();
    let mut directory_entries = Vec::new();
    let mut io_errors = Vec::new();
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                io_errors.push(format!("{}: {error}", NAMESPACE.dir));
                continue;
            }
        };
        let path = entry.path();
        if path.extension().is_some_and(|ext| ext == "jsonl") {
            directory_entries
                .push(entry.file_name().to_string_lossy().into_owned());
            if path.is_file() {
                paths.push(path);
            } else {
                io_errors.push(format!(
                    "{}/{}: expected a regular journal file",
                    NAMESPACE.dir,
                    entry.file_name().to_string_lossy()
                ));
            }
        }
    }
    paths.sort();
    directory_entries.sort();

    let mut stored = Vec::new();
    let mut malformed = Vec::new();
    let mut preimages = BTreeMap::new();
    for path in paths {
        let bytes = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) => {
                io_errors.push(format!(
                    "{}/{}: {error}",
                    NAMESPACE.dir,
                    path.file_name().unwrap_or_default().to_string_lossy()
                ));
                continue;
            }
        };
        let filename = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        let relative = format!("{}/{filename}", NAMESPACE.dir);
        preimages.insert(relative.clone(), bytes.clone());
        for (index, line) in bytes.split(|byte| *byte == b'\n').enumerate() {
            if line.is_empty()
                || line.iter().all(|byte| byte.is_ascii_whitespace())
            {
                continue;
            }
            let line_number = index + 1;
            let Ok(text) = std::str::from_utf8(line) else {
                malformed.push(MalformedLine {
                    path: relative.clone(),
                    line: line_number,
                });
                continue;
            };
            let Some(event) = parse_envelope(
                text,
                &filename,
                u32::try_from(line_number).unwrap_or(u32::MAX),
            ) else {
                malformed.push(MalformedLine {
                    path: relative.clone(),
                    line: line_number,
                });
                continue;
            };
            stored.push((relative.clone(), event));
        }
    }

    let mut parsed = Vec::new();
    for (relative, event) in stored {
        match parse_payload(&event) {
            Some(payload) => parsed.push(payload),
            None => malformed.push(MalformedLine {
                path: relative,
                line: event.line as usize,
            }),
        }
    }
    sort_by_writer_order(&mut parsed, |payload| match payload {
        Payload::Import(event) => (
            event.ts,
            event.host.as_str(),
            event.path.as_str(),
            event.line,
        ),
        Payload::Txn(event) => (
            event.ts,
            event.host.as_str(),
            event.path.as_str(),
            event.line,
        ),
    });

    let mut snapshot = JournalSnapshot {
        malformed,
        preimages,
        directory_entries,
        io_errors,
        ..JournalSnapshot::default()
    };
    for payload in parsed {
        match payload {
            Payload::Import(event) => snapshot.imports.push(event),
            Payload::Txn(event) => fold_txn(&mut snapshot, event),
        }
    }
    Ok(snapshot)
}

fn fold_txn(snapshot: &mut JournalSnapshot, event: TxnEvent) {
    if let Some(owner) = snapshot.ids.get(&event.id) {
        if owner != &event.key {
            snapshot.contradictions.push(JournalContradiction {
                key: event.key.clone(),
                field: "id",
            });
            return;
        }
    } else {
        snapshot.ids.insert(event.id.clone(), event.key.clone());
    }
    match snapshot.transactions.get(&event.key) {
        None => {
            snapshot.transactions.insert(event.key.clone(), event);
        }
        Some(existing) => {
            if existing.account != event.account {
                snapshot.contradictions.push(JournalContradiction {
                    key: event.key.clone(),
                    field: "account",
                });
            }
            if existing.cents != event.cents {
                snapshot.contradictions.push(JournalContradiction {
                    key: event.key.clone(),
                    field: "cents",
                });
            }
            if existing.posted != event.posted {
                snapshot.contradictions.push(JournalContradiction {
                    key: event.key.clone(),
                    field: "posted",
                });
            }
        }
    }
}

enum Payload {
    Import(ImportEvent),
    Txn(TxnEvent),
}

fn parse_payload(event: &StoredEvent) -> Option<Payload> {
    let object = event.value.as_object()?;
    match event.kind.as_str() {
        IMPORT_KIND => Some(Payload::Import(parse_import(event, object)?)),
        TXN_KIND => Some(Payload::Txn(parse_txn(event, object)?)),
        _ => None,
    }
}

fn parse_import(
    event: &StoredEvent,
    object: &Map<String, Value>,
) -> Option<ImportEvent> {
    let account = required_alias(object, "account")?;
    let source = SourceKind::parse(required_str(object, "source")?)?;
    let file_sha256 = required_digest(object, "file_sha256")?;
    let start = required_date(object, "start")?;
    let end = required_date(object, "end")?;
    let balance_cents = optional_i64(object, "balance_cents")?;
    let balance_asof = optional_date(object, "balance_asof")?;
    let counts = parse_counts(object.get("counts")?)?;
    Some(ImportEvent {
        ts: event.ts,
        host: event.host.clone(),
        path: event.path.clone(),
        line: event.line,
        account,
        source,
        file_sha256,
        start,
        end,
        balance_cents,
        balance_asof,
        counts,
    })
}

fn parse_txn(
    event: &StoredEvent,
    object: &Map<String, Value>,
) -> Option<TxnEvent> {
    let account = required_alias(object, "account")?;
    let key = required_digest(object, "key")?;
    let id = required_str(object, "id")?;
    if !is_row_id(id, &key) {
        return None;
    }
    let posted = required_date(object, "posted")?;
    let cents = required_i64(object, "cents")?;
    let desc = object.get("desc")?.as_str()?.to_owned();
    let merchant = required_str(object, "merchant")?.to_owned();
    let outcome = TxnOutcome::parse(required_str(object, "outcome")?)?;
    let note = optional_note(object)?;
    if outcome == TxnOutcome::BeforeStart && note.is_some() {
        return None;
    }
    if outcome != TxnOutcome::BeforeStart && note.is_none() {
        return None;
    }
    Some(TxnEvent {
        ts: event.ts,
        host: event.host.clone(),
        path: event.path.clone(),
        line: event.line,
        account,
        key,
        id: id.to_owned(),
        posted,
        cents,
        desc,
        merchant,
        outcome,
        note,
    })
}

fn parse_counts(value: &Value) -> Option<ImportCounts> {
    let object = value.as_object()?;
    Some(ImportCounts {
        new: optional_u32(object, "new")?,
        matched: optional_u32(object, "matched")?,
        seen: optional_u32(object, "seen")?,
        ambiguous: optional_u32(object, "ambiguous")?,
        before_start: optional_u32(object, "before_start")?,
    })
}

fn optional_u32(object: &Map<String, Value>, field: &str) -> Option<u32> {
    match object.get(field) {
        None => Some(0),
        Some(value) => {
            let number = value.as_u64()?;
            u32::try_from(number).ok()
        }
    }
}

fn required_str<'a>(
    object: &'a Map<String, Value>,
    field: &str,
) -> Option<&'a str> {
    let value = object.get(field)?.as_str()?;
    (!value.is_empty()).then_some(value)
}

fn required_alias(object: &Map<String, Value>, field: &str) -> Option<String> {
    let alias = required_str(object, field)?;
    is_alias(alias).then(|| alias.to_owned())
}

fn required_digest(object: &Map<String, Value>, field: &str) -> Option<String> {
    let value = required_str(object, field)?;
    (value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .then(|| value.to_ascii_lowercase())
}

fn required_date(
    object: &Map<String, Value>,
    field: &str,
) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(required_str(object, field)?, "%Y-%m-%d").ok()
}

fn optional_date(
    object: &Map<String, Value>,
    field: &str,
) -> Option<Option<NaiveDate>> {
    match object.get(field) {
        None | Some(Value::Null) => Some(None),
        Some(value) => Some(Some(
            NaiveDate::parse_from_str(value.as_str()?, "%Y-%m-%d").ok()?,
        )),
    }
}

fn required_i64(object: &Map<String, Value>, field: &str) -> Option<i64> {
    object.get(field)?.as_i64()
}

fn optional_i64(
    object: &Map<String, Value>,
    field: &str,
) -> Option<Option<i64>> {
    match object.get(field) {
        None | Some(Value::Null) => Some(None),
        Some(value) => Some(Some(value.as_i64()?)),
    }
}

fn optional_note(object: &Map<String, Value>) -> Option<Option<String>> {
    match object.get("note") {
        None | Some(Value::Null) => Some(None),
        Some(value) => {
            let note = value.as_str()?;
            is_relative_note(note).then(|| Some(note.to_owned()))
        }
    }
}

fn is_row_id(id: &str, key: &str) -> bool {
    (id.len() == 8 || id.len() == 12)
        && id.bytes().all(|byte| byte.is_ascii_hexdigit())
        && key.starts_with(id)
}

fn is_relative_note(path: &str) -> bool {
    !path.is_empty()
        && !path.starts_with('/')
        && !path.contains('\\')
        && !path.split('/').any(|part| part == ".." || part.is_empty())
}

#[cfg_attr(not(test), allow(dead_code))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NewImport {
    pub(crate) account: String,
    pub(crate) source: SourceKind,
    pub(crate) file_sha256: String,
    pub(crate) start: NaiveDate,
    pub(crate) end: NaiveDate,
    pub(crate) balance_cents: Option<i64>,
    pub(crate) balance_asof: Option<NaiveDate>,
    pub(crate) counts: ImportCounts,
}

#[cfg_attr(not(test), allow(dead_code))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NewTxn {
    pub(crate) account: String,
    pub(crate) key: String,
    pub(crate) id: String,
    pub(crate) posted: NaiveDate,
    pub(crate) cents: i64,
    pub(crate) desc: String,
    pub(crate) outcome: TxnOutcome,
    pub(crate) note: Option<String>,
}

#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn serialize_import(
    ts: DateTime<FixedOffset>,
    host: &str,
    event: &NewImport,
) -> String {
    let ts = ts.to_rfc3339_opts(chrono::SecondsFormat::Secs, false);
    let mut value = json!({
        "v": 1,
        "ts": ts,
        "host": host,
        "kind": IMPORT_KIND,
        "account": event.account,
        "source": event.source.as_str(),
        "file_sha256": event.file_sha256,
        "start": event.start.format("%Y-%m-%d").to_string(),
        "end": event.end.format("%Y-%m-%d").to_string(),
        "counts": {
            "new": event.counts.new,
            "matched": event.counts.matched,
            "seen": event.counts.seen,
            "ambiguous": event.counts.ambiguous,
            "before_start": event.counts.before_start,
        },
    });
    let object = value.as_object_mut().expect("object");
    if let Some(cents) = event.balance_cents {
        object.insert("balance_cents".to_owned(), json!(cents));
    }
    if let Some(asof) = event.balance_asof {
        object.insert(
            "balance_asof".to_owned(),
            json!(asof.format("%Y-%m-%d").to_string()),
        );
    }
    let mut line =
        serde_json::to_string(&value).unwrap_or_else(|_| "{}".to_owned());
    line.push('\n');
    line
}

#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn serialize_txn(
    ts: DateTime<FixedOffset>,
    host: &str,
    event: &NewTxn,
) -> String {
    let ts = ts.to_rfc3339_opts(chrono::SecondsFormat::Secs, false);
    let desc = mask_digits(&event.desc);
    let merchant = merchant_key(&event.desc);
    let value = json!({
        "v": 1,
        "ts": ts,
        "host": host,
        "kind": TXN_KIND,
        "account": event.account,
        "key": event.key,
        "id": event.id,
        "posted": event.posted.format("%Y-%m-%d").to_string(),
        "cents": event.cents,
        "desc": desc,
        "merchant": merchant,
        "outcome": event.outcome.as_str(),
        "note": event.note,
    });
    let mut line =
        serde_json::to_string(&value).unwrap_or_else(|_| "{}".to_owned());
    line.push('\n');
    line
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::native::bank::identity::{sha256_hex, transaction_key};
    use crate::native::pref_journal;

    fn ts() -> DateTime<FixedOffset> {
        DateTime::parse_from_rfc3339("2026-11-12T12:00:00-05:00").unwrap()
    }

    fn date(day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 11, day).unwrap()
    }

    fn temp_vault(label: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "bob-bank-journal-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        fs::create_dir_all(&root).expect("vault");
        root
    }

    fn append(vault: &Path, host: &str, line: &str) {
        pref_journal::append_line(vault, NAMESPACE, host, line).unwrap();
    }

    fn sample_txn(desc: &str) -> NewTxn {
        let key = transaction_key("checking", "fitid-secret-9999");
        NewTxn {
            account: "checking".to_owned(),
            id: key.chars().take(8).collect(),
            key,
            posted: date(12),
            cents: -1371,
            desc: desc.to_owned(),
            outcome: TxnOutcome::Written,
            note: Some("bank/2026-11.md".to_owned()),
        }
    }

    fn sample_import() -> NewImport {
        NewImport {
            account: "checking".to_owned(),
            source: SourceKind::Ofx,
            file_sha256: sha256_hex(b"file-bytes"),
            start: date(1),
            end: date(12),
            balance_cents: Some(100_000),
            balance_asof: Some(date(12)),
            counts: ImportCounts {
                new: 1,
                ..ImportCounts::default()
            },
        }
    }

    #[test]
    fn serialized_events_mask_and_omit_raw_identifiers() {
        let txn = sample_txn("SMASHBURGER CARD 1234567890 FITID");
        let line = serialize_txn(ts(), "athena", &txn);
        assert!(!line.contains("fitid-secret-9999"));
        assert!(!line.contains("1234567890"));
        assert!(line.contains("••••"));
        assert!(line.contains("\"merchant\":\"smashburger card •••• fitid\""));
        let import = serialize_import(ts(), "athena", &sample_import());
        assert!(!import.contains("fitid"));
        assert!(!import.contains("1234567890"));
        assert!(import.contains("\"source\":\"ofx\""));
    }

    #[test]
    fn multi_host_replay_unions_and_exposes_money_contradictions() {
        let vault = temp_vault("hosts");
        let txn = sample_txn("Coffee");
        append(&vault, "apollo", &serialize_txn(ts(), "apollo", &txn));
        append(
            &vault,
            "athena",
            &serialize_import(ts(), "athena", &sample_import()),
        );
        let mut replay = txn.clone();
        replay.outcome = TxnOutcome::Matched;
        append(&vault, "athena", &serialize_txn(ts(), "athena", &replay));
        let snapshot = load(&vault).unwrap();
        assert_eq!(snapshot.transactions.len(), 1);
        assert_eq!(
            snapshot.transactions[&txn.key].outcome,
            TxnOutcome::Written
        );
        assert_eq!(snapshot.imports.len(), 1);
        assert_eq!(
            snapshot.preimages.len(),
            2,
            "{:?}",
            snapshot.preimages.keys().collect::<Vec<_>>()
        );

        let mut conflict = txn.clone();
        conflict.cents = -2400;
        append(&vault, "mac", &serialize_txn(ts(), "mac", &conflict));
        let snapshot = load(&vault).unwrap();
        assert_eq!(snapshot.transactions[&txn.key].cents, -1371);
        assert!(snapshot
            .contradictions
            .iter()
            .any(|item| item.field == "cents"));
    }

    #[test]
    fn malformed_lines_are_bounded_and_omit_raw_payloads() {
        let vault = temp_vault("malformed");
        let mut body = String::new();
        for index in 0..12 {
            body.push_str(&format!("{{not json {index} secret-fitid-9999}}\n"));
        }
        let path = pref_journal::host_journal_path(&vault, NAMESPACE, "athena");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, body).unwrap();
        let snapshot = load(&vault).unwrap();
        assert_eq!(snapshot.malformed.len(), 12);
        assert!(snapshot.transactions.is_empty());
        let serialized = format!("{snapshot:?}");
        assert!(!serialized.contains("secret-fitid-9999"));
        assert!(!snapshot.malformed.iter().any(|item| {
            item.path.contains("secret") || item.path.is_empty()
        }));
    }

    #[test]
    fn payload_validation_rejects_bad_digest_and_note() {
        let vault = temp_vault("payload");
        append(
            &vault,
            "athena",
            "{\"v\":1,\"ts\":\"2026-11-12T12:00:00-05:00\",\"host\":\"athena\",\"kind\":\"bank.txn\",\"account\":\"checking\",\"key\":\"abcd\",\"id\":\"abcd\",\"posted\":\"2026-11-12\",\"cents\":-1,\"desc\":\"x\",\"merchant\":\"x\",\"outcome\":\"written\",\"note\":\"/abs\"}\n",
        );
        let snapshot = load(&vault).unwrap();
        assert_eq!(snapshot.malformed.len(), 1);
        assert!(snapshot.transactions.is_empty());
    }
}
