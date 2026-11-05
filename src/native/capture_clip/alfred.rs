//! Read-only adapter for Alfred 5's clipboard history.
//!
//! The store lives at `~/Library/Application Support/Alfred/Databases/
//! clipboard.alfdb` and uses a rollback journal, so a concurrent Alfred
//! write briefly locks readers: the connection sets a ~250 ms busy
//! timeout and lock failures surface as retryable [`ProviderError::Busy`].
//! The connection is always read-only and never copied, so bob can never
//! checkpoint or delete Alfred's journal.
//!
//! Observed schema (stable since at least 2020):
//! `clipboard(item, ts decimal, app, apppath, dataType integer, dataHash)`
//! indexed on `ts`. `ts` counts seconds since 2001-01-01 UTC (the Cocoa
//! epoch) and may be stored integer or real. Alfred de-duplicates by
//! content and bumps `ts` on a re-copy, so the manager's own
//! most-recently-used view is `ORDER BY ts DESC, rowid DESC`. The ordering
//! promise is that view, never "the last N copy keystrokes".
//!
//! [`ProviderError::Busy`]: super::history::ProviderError::Busy

use std::{
    collections::HashMap,
    path::Path,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use rusqlite::{Connection, Error, OpenFlags};

use super::history::{EntryKind, HistoryEntry, ProviderError};

/// Seconds between the Unix epoch and the Cocoa epoch (2001-01-01 UTC).
const COCOA_EPOCH_OFFSET: f64 = 978_307_200.0;

/// How long a read waits on Alfred's rollback-journal write lock.
const BUSY_TIMEOUT: Duration = Duration::from_millis(250);

/// Alfred records plain text with `dataType = 0`; anything else is a file
/// or image row whose format bob has not verified.
const TEXT_DATA_TYPE: i64 = 0;

pub(super) fn read_alfred_history(
    database: &Path,
    count: usize,
) -> Result<Vec<HistoryEntry>, ProviderError> {
    if !database.is_file() {
        return Err(ProviderError::Missing(format!(
            "Alfred clipboard history database was not found at {}; enable Alfred's clipboard history or set capture.clipboard_history",
            database.display()
        )));
    }
    let connection = Connection::open_with_flags(
        database,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|error| {
        ProviderError::Failed(format!(
            "open Alfred clipboard history database {} read-only: {error}",
            database.display()
        ))
    })?;
    connection.busy_timeout(BUSY_TIMEOUT).map_err(|error| {
        ProviderError::Failed(format!(
            "set Alfred clipboard history busy timeout: {error}"
        ))
    })?;
    validate_alfred_schema(&connection)?;

    let mut statement = connection
        .prepare(
            "SELECT item, ts, app, dataType FROM clipboard \
             ORDER BY ts DESC, rowid DESC LIMIT ?1",
        )
        .map_err(|error| {
            map_sqlite_error(error, "query Alfred clipboard history")
        })?;
    let rows = statement
        .query_map([i64::try_from(count).unwrap_or(i64::MAX)], |row| {
            Ok((
                row.get::<_, Option<String>>(0)?,
                row.get::<_, f64>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, i64>(3)?,
            ))
        })
        .map_err(|error| {
            map_sqlite_error(error, "query Alfred clipboard history")
        })?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| {
            map_sqlite_error(error, "decode Alfred clipboard history rows")
        })?;

    Ok(rows
        .into_iter()
        .map(|(item, ts, app, data_type)| {
            let at = cocoa_to_system_time(ts);
            let app = app.filter(|app| !app.is_empty());
            if data_type == TEXT_DATA_TYPE {
                match item {
                    Some(text) => HistoryEntry::text(text, at, app),
                    None => HistoryEntry {
                        text: None,
                        kind: EntryKind::NotText("file or image".to_string()),
                        at,
                        app,
                    },
                }
            } else {
                HistoryEntry::not_text("file or image".to_string(), at, app)
            }
        })
        .collect())
}

/// `ts` counts seconds since 2001-01-01 UTC; forgetting the offset yields
/// 1995 dates. Out-of-range values keep no timestamp instead of failing
/// the whole read.
fn cocoa_to_system_time(ts: f64) -> Option<SystemTime> {
    if !ts.is_finite() {
        return None;
    }
    let unix = ts + COCOA_EPOCH_OFFSET;
    if unix < 0.0 {
        return None;
    }
    UNIX_EPOCH.checked_add(Duration::from_secs_f64(unix))
}

fn validate_alfred_schema(
    connection: &Connection,
) -> Result<(), ProviderError> {
    let mut statement = connection
        .prepare("PRAGMA table_info(\"clipboard\")")
        .map_err(|error| {
            map_sqlite_error(error, "inspect Alfred table clipboard")
        })?;
    let columns = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?.to_ascii_uppercase(),
            ))
        })
        .map_err(|error| {
            map_sqlite_error(error, "inspect Alfred table clipboard")
        })?
        .collect::<Result<HashMap<_, _>, _>>()
        .map_err(|error| {
            map_sqlite_error(error, "inspect Alfred table clipboard")
        })?;
    let required = ["item", "ts", "app", "dataType"];
    let missing = required
        .iter()
        .filter(|column| !columns.contains_key(**column))
        .collect::<Vec<_>>();
    if !missing.is_empty() {
        let missing = missing
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(", ");
        return Err(ProviderError::Unsupported(format!(
            "unsupported Alfred clipboard database: table clipboard is missing required column(s): {missing}"
        )));
    }
    Ok(())
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
