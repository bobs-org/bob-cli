//! Read-only adapter for Clipy's SQLite clipboard history.
//!
//! Clipy's `pasteboardHistories.id` is a content hash or UUID, so it must
//! never order same-second copies: ties break on `rowid`. Skipped rows
//! (concealed or transient pasteboard types) do not count toward the
//! requested count, so ordered rows are decoded lazily until `count`
//! entries are kept. Each kept entry decodes one canonical flavor — the
//! first type present in preference order — so a Safari URL copy (text
//! plus `public.url`) yields only its text instead of two joined lines.

use std::{
    collections::HashMap,
    io::Cursor,
    path::Path,
    time::{Duration, UNIX_EPOCH},
};

use rusqlite::{Connection, Error, OpenFlags};

use super::history::{HistoryEntry, ProviderError};

/// How long a read waits on Clipy's WAL write lock before reporting
/// [`ProviderError::Busy`].
const BUSY_TIMEOUT: Duration = Duration::from_millis(250);

/// Pasteboard types whose rows must never enter the vault.
const HIDDEN_TYPES: [&str; 2] = [
    "org.nspasteboard.ConcealedType",
    "org.nspasteboard.TransientType",
];

/// Canonical flavor preference: the first type present on a row wins and
/// only its assets are joined, in `index` order.
const FLAVOR_PREFERENCE: [&str; 6] = [
    "public.utf8-plain-text",
    "NSStringPboardType",
    "public.file-url",
    "NSFilenamesPboardType",
    "public.url",
    "NSURLPboardType",
];

pub(super) fn read_clipy_history(
    database: &Path,
    count: usize,
) -> Result<Vec<HistoryEntry>, ProviderError> {
    if !database.is_file() {
        return Err(ProviderError::Missing(format!(
            "Clipy clipboard history database was not found at {}; install and run Clipy or set BOB_CLIPBOARD_HISTORY_CMD",
            database.display()
        )));
    }
    let connection = Connection::open_with_flags(
        database,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|error| {
        ProviderError::Failed(format!(
            "open Clipy clipboard history database {} read-only: {error}",
            database.display()
        ))
    })?;
    connection.busy_timeout(BUSY_TIMEOUT).map_err(|error| {
        ProviderError::Failed(format!(
            "set Clipy clipboard history busy timeout: {error}"
        ))
    })?;
    validate_clipy_schema(&connection)?;

    // `id` is a hash/UUID, so same-second copies order randomly on it;
    // `rowid` is the stable insertion tie-break.
    let mut statement = connection
        .prepare(
            "SELECT id, updateAt FROM pasteboardHistories \
             ORDER BY updateAt DESC, rowid DESC",
        )
        .map_err(|error| {
            map_sqlite_error(error, "query Clipy clipboard history")
        })?;
    let ordered = statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })
        .map_err(|error| {
            map_sqlite_error(error, "query Clipy clipboard history")
        })?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| {
            map_sqlite_error(error, "decode Clipy clipboard history rows")
        })?;

    let mut entries = Vec::new();
    for (entry_number, (id, update_at)) in ordered.iter().enumerate() {
        if entries.len() >= count {
            break;
        }
        let Some(entry) =
            decode_clipy_entry(&connection, id, update_at, entry_number + 1)?
        else {
            // A concealed or transient row: skipped, never counted.
            continue;
        };
        entries.push(entry);
    }
    Ok(entries)
}

/// Decode one ordered Clipy row. Returns `None` for concealed/transient
/// rows, which callers skip without counting.
fn decode_clipy_entry(
    connection: &Connection,
    history_id: &str,
    update_at: &i64,
    entry_number: usize,
) -> Result<Option<HistoryEntry>, ProviderError> {
    let mut statement = connection
        .prepare(
            "SELECT pasteboardType, data FROM pasteboardHistoryAssets \
             WHERE pasteboardHistoryID = ?1 ORDER BY \"index\" ASC, id ASC",
        )
        .map_err(|error| {
            map_sqlite_error(
                error,
                &format!("query Clipy history entry {entry_number}"),
            )
        })?;
    let assets = statement
        .query_map([history_id], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, Vec<u8>>(1)?))
        })
        .map_err(|error| {
            map_sqlite_error(
                error,
                &format!("query Clipy history entry {entry_number}"),
            )
        })?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| {
            map_sqlite_error(
                error,
                &format!("decode Clipy history entry {entry_number} assets"),
            )
        })?;

    if assets.iter().any(|(pasteboard_type, _)| {
        HIDDEN_TYPES.contains(&pasteboard_type.as_str())
    }) {
        return Ok(None);
    }

    let at = u64::try_from(*update_at)
        .ok()
        .and_then(|secs| UNIX_EPOCH.checked_add(Duration::from_secs(secs)));
    let found = FLAVOR_PREFERENCE.iter().find_map(|flavor| {
        let matching = assets
            .iter()
            .filter(|(pasteboard_type, _)| pasteboard_type == flavor)
            .collect::<Vec<_>>();
        (!matching.is_empty()).then_some((*flavor, matching))
    });
    let Some((flavor, matching)) = found else {
        let mut unsupported = assets
            .iter()
            .map(|(pasteboard_type, _)| pasteboard_type.clone())
            .collect::<Vec<_>>();
        unsupported.sort();
        unsupported.dedup();
        let types = if unsupported.is_empty() {
            "no stored assets".to_string()
        } else {
            unsupported.join(", ")
        };
        return Ok(Some(HistoryEntry::not_text(types, at, None)));
    };

    let mut values = Vec::new();
    for (_, data) in matching {
        let decoded = decode_clipy_asset(flavor, data).map_err(|error| {
            ProviderError::Failed(format!(
                "Clipy history entry {entry_number} has invalid {flavor} data: {error}"
            ))
        })?;
        values.extend(decoded);
    }
    Ok(Some(HistoryEntry::text(values.join("\n"), at, None)))
}

fn validate_clipy_schema(connection: &Connection) -> Result<(), ProviderError> {
    validate_clipy_table(
        connection,
        "pasteboardHistories",
        &[("id", "TEXT"), ("updateAt", "INTEGER")],
    )?;
    validate_clipy_table(
        connection,
        "pasteboardHistoryAssets",
        &[
            ("id", "TEXT"),
            ("pasteboardHistoryID", "TEXT"),
            ("index", "INTEGER"),
            ("pasteboardType", "TEXT"),
            ("data", "BLOB"),
        ],
    )
}

fn validate_clipy_table(
    connection: &Connection,
    table: &str,
    required: &[(&str, &str)],
) -> Result<(), ProviderError> {
    let sql = format!("PRAGMA table_info(\"{table}\")");
    let mut statement = connection.prepare(&sql).map_err(|error| {
        map_sqlite_error(error, &format!("inspect Clipy table {table}"))
    })?;
    let columns = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?.to_ascii_uppercase(),
            ))
        })
        .map_err(|error| {
            map_sqlite_error(error, &format!("inspect Clipy table {table}"))
        })?
        .collect::<Result<HashMap<_, _>, _>>()
        .map_err(|error| {
            map_sqlite_error(error, &format!("inspect Clipy table {table}"))
        })?;
    let missing = required
        .iter()
        .filter(|(column, _)| !columns.contains_key(*column))
        .map(|(column, _)| *column)
        .collect::<Vec<_>>();
    if !missing.is_empty() {
        return Err(ProviderError::Unsupported(format!(
            "unsupported or unmigrated Clipy database: table {table} is missing required column(s): {}",
            missing.join(", ")
        )));
    }
    let wrong_types = required
        .iter()
        .filter_map(|(column, expected)| {
            let actual = columns.get(*column)?;
            (actual != expected)
                .then(|| format!("{column} is {actual}, expected {expected}"))
        })
        .collect::<Vec<_>>();
    if !wrong_types.is_empty() {
        return Err(ProviderError::Unsupported(format!(
            "unsupported or unmigrated Clipy database: table {table} has incompatible column type(s): {}",
            wrong_types.join(", ")
        )));
    }
    Ok(())
}

fn decode_clipy_asset(
    pasteboard_type: &str,
    data: &[u8],
) -> Result<Vec<String>, String> {
    match pasteboard_type {
        "public.utf8-plain-text"
        | "NSStringPboardType"
        | "public.file-url"
        | "public.url"
        | "NSURLPboardType" => decode_utf8_asset(data).map(|value| vec![value]),
        "NSFilenamesPboardType" => decode_filenames_asset(data),
        _ => Ok(vec![]),
    }
}

fn decode_utf8_asset(data: &[u8]) -> Result<String, String> {
    if let Ok(value) = String::from_utf8(data.to_vec()) {
        return Ok(value);
    }
    plist::Value::from_reader(Cursor::new(data))
        .ok()
        .and_then(plist::Value::into_string)
        .ok_or_else(|| "value is not valid UTF-8 text".to_string())
}

fn decode_filenames_asset(data: &[u8]) -> Result<Vec<String>, String> {
    let value = plist::Value::from_reader(Cursor::new(data))
        .map_err(|error| format!("invalid filenames property list: {error}"))?;
    let filenames = value
        .into_array()
        .ok_or_else(|| "filenames property list is not an array".to_string())?;
    filenames
        .into_iter()
        .enumerate()
        .map(|(index, value)| {
            value.into_string().ok_or_else(|| {
                format!("filename {} is not a string", index + 1)
            })
        })
        .collect()
}

fn map_sqlite_error(error: Error, context: &str) -> ProviderError {
    if let Error::SqliteFailure(failure, _) = &error
        && matches!(
            failure.code,
            rusqlite::ErrorCode::DatabaseBusy
                | rusqlite::ErrorCode::DatabaseLocked
        )
    {
        return ProviderError::Busy(format!("{context}: {error}"));
    }
    ProviderError::Failed(format!("{context}: {error}"))
}
