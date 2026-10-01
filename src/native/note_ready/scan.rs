//! Vault scan for the per-note Ready cap: one freshness snapshot
//! plus the typed-note walk, evaluated by the pure [`super`]
//! evaluator.

use std::{collections::HashMap, path::Path};

use chrono::NaiveDate;

use super::{evaluate, CapSource, LaneRow, NoteInput, Report};
use crate::native::{
    config::{self, ConfigError},
    dataview::{self, DataviewError},
    env as bob_env,
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

/// One counted Ready-lane row for the `bob ready NOTE` worklist,
/// in vault scan order (callers sort by line).
#[derive(Debug, Clone)]
pub(crate) struct ReadyDetail {
    pub(crate) path: String,
    pub(crate) line: u32,
    pub(crate) text: String,
    pub(crate) block_id: Option<String>,
    pub(crate) bucket: Option<String>,
    pub(crate) fresh_on: Option<NaiveDate>,
}

/// Evaluated per-note report plus scan metadata.
pub(crate) struct ScanReport {
    pub(crate) report: Report,
    pub(crate) today: NaiveDate,
    pub(crate) default_cap: u32,
    pub(crate) default_source: CapSource,
    /// Counted Ready-lane rows with worklist detail.
    pub(crate) ready_details: Vec<ReadyDetail>,
    /// Whole-vault NEXT/PENDING lane rows per note path, for the
    /// worklist `also here` line (same engine queries as `bob plan`).
    pub(crate) next_counts: HashMap<String, u32>,
    pub(crate) pending_counts: HashMap<String, u32>,
    /// Open blocked tasks per note path, for the worklist
    /// `also here` line.
    pub(crate) blocked_counts: HashMap<String, u32>,
}

/// Scan the vault once and evaluate the per-note Ready cap,
/// replacing only the configured default with `preview` when given.
/// Note overrides and exemptions still apply; affected entries
/// report `preview` as their cap source.
pub(crate) fn scan_note_ready_with_preview(
    bob_dir: &Path,
    preview: Option<u32>,
) -> Result<ScanReport, ScanError> {
    scan_inner(bob_dir, preview, true)
}

/// Scan the vault for the overview only: no worklist row details
/// and no NEXT/PENDING lane passes, so the overview costs one
/// freshness scan plus the typed-note walk.
pub(crate) fn scan_note_ready_overview(
    bob_dir: &Path,
    preview: Option<u32>,
) -> Result<ScanReport, ScanError> {
    scan_inner(bob_dir, preview, false)
}

fn scan_inner(
    bob_dir: &Path,
    preview: Option<u32>,
    details: bool,
) -> Result<ScanReport, ScanError> {
    let plan_config = config::load_plan_config(&config::config_path())
        .map_err(|error| match error {
            ConfigError::Read(message) => ScanError::Io(message),
            ConfigError::Invalid(message) => ScanError::Usage(message),
        })?;
    let (default_cap, default_source) = match preview {
        Some(cap) => (cap, CapSource::Preview),
        None => (plan_config.max_ready_per_note(), default_cap_source()),
    };

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

    let mut rows = Vec::with_capacity(snapshot.ready.len());
    let mut ready_details = Vec::new();
    for row in &snapshot.ready {
        let (bucket, fresh_on) =
            row_bucket(row, snapshot.today, &snapshot.config);
        let bucket = bucket.map(str::to_string);
        rows.push(LaneRow {
            path: row.task.path.clone(),
            is_recurring: row.task.is_recurring,
            block_id: row.task.block_id.clone(),
            bucket: bucket.clone(),
        });
        // The worklist lists counted rows only: recurring and `^prj`
        // rows are excluded from the lane count and from the list.
        if !row.task.is_recurring && row.task.block_id.as_deref() != Some("prj")
        {
            ready_details.push(ReadyDetail {
                path: row.task.path.clone(),
                line: row.task.line,
                text: row.task.text.clone(),
                block_id: row.task.block_id.clone(),
                bucket,
                fresh_on,
            });
        }
    }

    let report = evaluate(&notes, &rows, default_cap, default_source, true);
    let _ = ProjectStatus::Wip;

    // NEXT/PENDING lane rows per note for the worklist `also here`
    // line. Blocked rows come from the snapshot's open pass so no
    // extra vault read is needed for them. The overview skips all
    // three so it costs no extra vault passes.
    let (next_counts, pending_counts, blocked_counts) = if details {
        let now = bob_env::current_datetime();
        let next_counts =
            count_lane_by_path(bob_dir, dataview::NEXT_QUERY, now, &report)?;
        let pending_counts =
            count_lane_by_path(bob_dir, dataview::PENDING_QUERY, now, &report)?;
        let mut blocked_counts: HashMap<String, u32> = HashMap::new();
        for row in &snapshot.open {
            if row.task.is_blocked {
                *blocked_counts.entry(row.task.path.clone()).or_default() += 1;
            }
        }
        (next_counts, pending_counts, blocked_counts)
    } else {
        (HashMap::new(), HashMap::new(), HashMap::new())
    };

    Ok(ScanReport {
        report,
        today: snapshot.today,
        default_cap,
        default_source,
        ready_details,
        next_counts,
        pending_counts,
        blocked_counts,
    })
}

/// Count one engine lane query per note path. Only notes present in
/// the per-note report keep a count; lane rows elsewhere (daily
/// notes, untyped notes) never reach the worklist.
fn count_lane_by_path(
    bob_dir: &Path,
    query: &str,
    now: chrono::NaiveDateTime,
    report: &Report,
) -> Result<HashMap<String, u32>, ScanError> {
    use std::collections::HashSet;
    let known: HashSet<&str> =
        report.notes.iter().map(|note| note.path.as_str()).collect();
    let tasks = dataview::query_rich_tasks(bob_dir, query, now)
        .map_err(|error| ScanError::Io(dataview_message(&error)))?;
    let mut counts: HashMap<String, u32> = HashMap::new();
    for task in &tasks {
        if known.contains(task.path.as_str()) {
            *counts.entry(task.path.clone()).or_default() += 1;
        }
    }
    Ok(counts)
}

fn dataview_message(error: &DataviewError) -> String {
    match error {
        DataviewError::TasksQuery { message }
        | DataviewError::NativeQuery { message }
        | DataviewError::DataviewQuery { message }
        | DataviewError::DataviewMissing { message } => message.clone(),
        DataviewError::NativeVaultRead { path, error } => {
            format!("read {}: {error}", path.display())
        }
        DataviewError::TasksSettingsRead { path, error } => {
            format!("read Tasks settings {}: {error}", path.display())
        }
        DataviewError::TasksSettingsParse { path, error } => {
            format!("parse Tasks settings {}: {error}", path.display())
        }
        other => format!("{other:?}"),
    }
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
