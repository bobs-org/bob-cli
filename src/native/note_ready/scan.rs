//! Vault scan for the per-note Ready cap: one freshness snapshot
//! plus the typed-note walk, evaluated by the pure [`super`]
//! evaluator.

use std::path::Path;

use chrono::NaiveDate;

use super::{evaluate, CapSource, LaneRow, NoteInput, Report};
use crate::native::{
    config::{self, ConfigError},
    freshness::scan::{row_bucket, scan},
    projects::{walk_typed_notes, ProjectStatus},
};

/// Scan failures: I/O-like errors exit 1, usage errors (invalid
/// config, unsupported task format) exit 2.
#[derive(Debug)]
pub(crate) enum ScanError {
    Io(String),
    Usage(String),
}

impl ScanError {
    pub(crate) fn message(&self) -> &str {
        match self {
            Self::Io(message) | Self::Usage(message) => message,
        }
    }

    pub(crate) fn exit_code(&self) -> i32 {
        match self {
            Self::Io(_) => 1,
            Self::Usage(_) => 2,
        }
    }
}

/// Evaluated per-note report plus scan metadata.
pub(crate) struct ScanReport {
    pub(crate) report: Report,
    pub(crate) today: NaiveDate,
    pub(crate) default_cap: u32,
    pub(crate) default_source: CapSource,
}

/// Scan the vault once and evaluate the per-note Ready cap.
pub(crate) fn scan_note_ready(bob_dir: &Path) -> Result<ScanReport, ScanError> {
    let plan_config = config::load_plan_config(&config::config_path())
        .map_err(|error| match error {
            ConfigError::Read(message) => ScanError::Io(message),
            ConfigError::Invalid(message) => ScanError::Usage(message),
        })?;
    let default_cap = plan_config.max_ready_per_note();
    let default_source = default_cap_source();

    let snapshot = scan(bob_dir).map_err(|error| match error {
        crate::native::freshness::scan::ScanError::Io(message) => {
            ScanError::Io(message)
        }
        crate::native::freshness::scan::ScanError::Usage(message) => {
            ScanError::Usage(message)
        }
    })?;

    let typed = walk_typed_notes(bob_dir);
    let notes: Vec<NoteInput> = typed
        .iter()
        .map(|note| {
            let is_area = note.kind == "area";
            NoteInput {
                path: note.path.clone(),
                name: note.stem.clone(),
                kind: note.kind.clone(),
                status: note.status.label().to_string(),
                is_area,
                is_terminal: note.status.is_terminal(),
                parent: note.parent.clone(),
                ready_cap_raw: note.ready_cap_raw.clone(),
            }
        })
        .collect();

    let rows: Vec<LaneRow> = snapshot
        .ready
        .iter()
        .map(|row| {
            let (bucket, _) = row_bucket(row, snapshot.today, &snapshot.config);
            LaneRow {
                path: row.task.path.clone(),
                is_recurring: row.task.is_recurring,
                block_id: row.task.block_id.clone(),
                bucket: bucket.map(str::to_string),
            }
        })
        .collect();

    let report = evaluate(&notes, &rows, default_cap, default_source, true);
    let _ = ProjectStatus::Wip;
    Ok(ScanReport {
        report,
        today: snapshot.today,
        default_cap,
        default_source,
    })
}

/// Where the default per-note cap came from: an explicit
/// `plan.max_ready_per_note` (`config`) or the built-in 5
/// (`default`).
pub(crate) fn default_cap_source() -> CapSource {
    let path = config::config_path();
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(_) => return CapSource::Default,
    };
    let value: Result<serde_yaml::Value, _> = serde_yaml::from_str(&text);
    let Ok(serde_yaml::Value::Mapping(top)) = value else {
        return CapSource::Default;
    };
    let key = serde_yaml::Value::String("plan".to_string());
    let Some(serde_yaml::Value::Mapping(plan)) = top.get(&key).cloned() else {
        return CapSource::Default;
    };
    let cap_key = serde_yaml::Value::String("max_ready_per_note".to_string());
    match plan.get(&cap_key) {
        Some(value) if !value.is_null() => CapSource::Config,
        _ => CapSource::Default,
    }
}
