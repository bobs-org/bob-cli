use std::{collections::HashMap, io::Cursor, path::Path};

use rusqlite::{Connection, OpenFlags};

pub(super) fn read_clipy_history(
    database: &Path,
    count: usize,
) -> Result<Vec<String>, String> {
    if !database.is_file() {
        return Err(format!(
            "Clipy clipboard history database was not found at {}; install and run Clipy or set BOB_CLIPBOARD_HISTORY_CMD",
            database.display()
        ));
    }
    let connection = Connection::open_with_flags(
        database,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|error| {
        format!(
            "open Clipy clipboard history database {} read-only: {error}",
            database.display()
        )
    })?;
    validate_clipy_schema(&connection)?;

    let mut statement = connection
        .prepare(
            "SELECT id FROM pasteboardHistories \
             ORDER BY updateAt DESC, id DESC LIMIT ?1",
        )
        .map_err(|error| format!("query Clipy clipboard history: {error}"))?;
    let ids = statement
        .query_map([i64::try_from(count).unwrap_or(i64::MAX)], |row| {
            row.get::<_, String>(0)
        })
        .map_err(|error| format!("query Clipy clipboard history: {error}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| {
            format!("decode Clipy clipboard history rows: {error}")
        })?;

    ids.iter()
        .enumerate()
        .map(|(index, id)| decode_clipy_entry(&connection, id, index + 1))
        .collect()
}

fn validate_clipy_schema(connection: &Connection) -> Result<(), String> {
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
) -> Result<(), String> {
    let sql = format!("PRAGMA table_info(\"{table}\")");
    let mut statement = connection
        .prepare(&sql)
        .map_err(|error| format!("inspect Clipy table {table}: {error}"))?;
    let columns = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?.to_ascii_uppercase(),
            ))
        })
        .map_err(|error| format!("inspect Clipy table {table}: {error}"))?
        .collect::<Result<HashMap<_, _>, _>>()
        .map_err(|error| format!("inspect Clipy table {table}: {error}"))?;
    let missing = required
        .iter()
        .filter(|(column, _)| !columns.contains_key(*column))
        .map(|(column, _)| *column)
        .collect::<Vec<_>>();
    if !missing.is_empty() {
        return Err(format!(
            "unsupported or unmigrated Clipy database: table {table} is missing required column(s): {}",
            missing.join(", ")
        ));
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
        return Err(format!(
            "unsupported or unmigrated Clipy database: table {table} has incompatible column type(s): {}",
            wrong_types.join(", ")
        ));
    }
    Ok(())
}

fn decode_clipy_entry(
    connection: &Connection,
    history_id: &str,
    entry_index: usize,
) -> Result<String, String> {
    let mut statement = connection
        .prepare(
            "SELECT pasteboardType, data FROM pasteboardHistoryAssets \
             WHERE pasteboardHistoryID = ?1 ORDER BY \"index\" ASC, id ASC",
        )
        .map_err(|error| {
            format!("query Clipy history entry {entry_index}: {error}")
        })?;
    let assets = statement
        .query_map([history_id], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, Vec<u8>>(1)?))
        })
        .map_err(|error| {
            format!("query Clipy history entry {entry_index}: {error}")
        })?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| {
            format!("decode Clipy history entry {entry_index} assets: {error}")
        })?;

    let mut values = Vec::new();
    let mut unsupported = Vec::new();
    for (pasteboard_type, data) in assets {
        match decode_clipy_asset(&pasteboard_type, &data).map_err(|error| {
            format!(
                "Clipy history entry {entry_index} has invalid {pasteboard_type} data: {error}"
            )
        })? {
            Some(mut decoded) => values.append(&mut decoded),
            None => unsupported.push(pasteboard_type),
        }
    }
    if values.is_empty() {
        unsupported.sort();
        unsupported.dedup();
        let types = if unsupported.is_empty() {
            "no stored assets".to_string()
        } else {
            unsupported.join(", ")
        };
        return Err(format!(
            "Clipy history entry {entry_index} has only unsupported binary representations ({types}); copy text or file/URL content instead"
        ));
    }
    Ok(values.join("\n"))
}

fn decode_clipy_asset(
    pasteboard_type: &str,
    data: &[u8],
) -> Result<Option<Vec<String>>, String> {
    match pasteboard_type {
        "public.utf8-plain-text"
        | "NSStringPboardType"
        | "public.url"
        | "NSURLPboardType"
        | "public.file-url" => {
            decode_utf8_asset(data).map(|value| Some(vec![value]))
        }
        "NSFilenamesPboardType" => decode_filenames_asset(data).map(Some),
        _ => Ok(None),
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
