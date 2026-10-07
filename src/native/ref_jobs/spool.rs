//! Durable ref-job spool under `bob_cli_state_dir()/ref/jobs/`.
//!
//! Directories are mode 0700 and files 0600. Every install goes
//! through a temp file, fsync, rename, and directory fsync, so a
//! crash never leaves a half-written job.

use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use chrono::{DateTime, Local, SecondsFormat};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::native::env::bob_cli_state_dir;

/// Job-file schema version, also used by `done.jsonl` records.
pub(crate) const SCHEMA_VERSION: u32 = 1;
/// Rewrite `done.jsonl` atomically, keeping this many lines, once it
/// grows past [`DONE_TRIM_THRESHOLD`].
const DONE_TRIM_THRESHOLD: usize = 2000;
const DONE_TRIM_KEEP: usize = 1000;
/// Default `list` window: terminal outcomes from the last 7 days.
/// Pending, clipping, and stuck jobs always show.
pub(crate) const DEFAULT_WINDOW_DAYS: i64 = 7;

static ID_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Spool root: `bob_cli_state_dir()/ref/jobs`.
pub(crate) fn jobs_dir() -> PathBuf {
    bob_cli_state_dir().join("ref").join("jobs")
}

/// One queued clip request, as stored in
/// `pending/<id>.json`, `running/<id>.json`, or `stuck/<id>.json`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct JobFile {
    pub(crate) schema_version: u32,
    pub(crate) id: String,
    /// RFC 3339 local-offset timestamp, e.g. `2026-10-07T14:30:12-04:00`.
    pub(crate) created_at: String,
    /// Stamped when the worker claims the job; `None` while pending.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) started_at: Option<String>,
    /// Entry point that queued the job; today always `capture`.
    pub(crate) source: String,
    pub(crate) bob_dir: PathBuf,
    /// URL as typed (the capture item text).
    pub(crate) url: String,
    /// Cleaned URL the ingest clips.
    pub(crate) cleaned_url: String,
    /// Create's exact dedupe key.
    pub(crate) dedupe_key: String,
    /// Short display form, e.g. `example.com/post`.
    pub(crate) display: String,
    /// Fetch hint: `article`, `pdf`, or `arxiv`.
    pub(crate) route_hint: String,
    /// Completed worker attempts; a stale `running/` file comes back
    /// with `attempts + 1` and fails at 2 without clipping again.
    #[serde(default)]
    pub(crate) attempts: u32,
    pub(crate) fallback: JobFallback,
    /// Terminal error, set when the job parks in `stuck/`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) error: Option<StoredError>,
    /// The fallback write's own error, set alongside `error`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) fallback_error: Option<String>,
}

/// Where the fallback task goes when the clip fails.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct JobFallback {
    /// Vault-relative target, e.g. `mac_inbox.md`.
    pub(crate) relative_target: String,
    /// Exact line capture would have written with routing off.
    pub(crate) task_line: String,
}

/// Typed error kept on stuck jobs and `done.jsonl` records.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct StoredError {
    pub(crate) kind: String,
    pub(crate) message: String,
    pub(crate) retryable: bool,
}

/// Fields capture stages for one new job.
pub(crate) struct NewJob {
    pub(crate) source: String,
    pub(crate) bob_dir: PathBuf,
    pub(crate) url: String,
    pub(crate) cleaned_url: String,
    pub(crate) dedupe_key: String,
    pub(crate) display: String,
    pub(crate) route_hint: String,
    pub(crate) fallback: JobFallback,
}

/// A terminal outcome in `done.jsonl`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DoneOutcome {
    Created,
    AlreadyInLibrary,
    AlreadyQueued,
    FellBack,
}

impl DoneOutcome {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            DoneOutcome::Created => "created",
            DoneOutcome::AlreadyInLibrary => "already_in_library",
            DoneOutcome::AlreadyQueued => "already_queued",
            DoneOutcome::FellBack => "fell_back",
        }
    }
}

/// One `done.jsonl` line.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct DoneRecord {
    pub(crate) schema_version: u32,
    pub(crate) id: String,
    pub(crate) url: String,
    pub(crate) cleaned_url: String,
    pub(crate) display: String,
    pub(crate) outcome: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) pdf: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) note: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) error: Option<StoredError>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) fallback: Option<DoneFallback>,
    pub(crate) created_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) started_at: Option<String>,
    pub(crate) finished_at: String,
}

/// Fallback target kept on `fell_back` records.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct DoneFallback {
    pub(crate) relative_target: String,
}

/// Where a listed job lives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum JobState {
    Pending,
    Clipping,
    Clipped,
    InLibrary,
    AlreadyQueued,
    FellBack,
    Stuck,
}

impl JobState {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            JobState::Pending => "pending",
            JobState::Clipping => "clipping",
            JobState::Clipped => "clipped",
            JobState::InLibrary => "in_library",
            JobState::AlreadyQueued => "already_queued",
            JobState::FellBack => "fell_back",
            JobState::Stuck => "stuck",
        }
    }
}

/// One row of the `list` view, whatever its backing store.
#[derive(Debug, Clone)]
pub(crate) struct ListedJob {
    pub(crate) id: String,
    pub(crate) state: JobState,
    pub(crate) url: String,
    pub(crate) display: String,
    pub(crate) created_at: String,
    pub(crate) started_at: Option<String>,
    pub(crate) finished_at: Option<String>,
    pub(crate) pdf: Option<String>,
    pub(crate) note: Option<String>,
    pub(crate) error: Option<StoredError>,
    pub(crate) fallback_target: Option<String>,
    /// Why the fallback write failed; set on `stuck/` job files only.
    pub(crate) fallback_error: Option<String>,
    /// Spool path for file-backed states; `None` for `done.jsonl` rows.
    pub(crate) path: Option<PathBuf>,
}

/// The whole `list` view plus per-state counts.
#[derive(Debug, Clone, Default)]
pub(crate) struct JobsView {
    pub(crate) jobs: Vec<ListedJob>,
    pub(crate) counts: HashMap<&'static str, usize>,
}

impl JobsView {
    fn count(&mut self, state: JobState) {
        *self.counts.entry(state.as_str()).or_insert(0) += 1;
    }
}

/// Current local-offset timestamp in RFC 3339 seconds precision.
pub(crate) fn now_rfc3339() -> String {
    Local::now().to_rfc3339_opts(SecondsFormat::Secs, false)
}

/// Parse an RFC 3339 timestamp; `None` when the spool holds garbage.
pub(crate) fn parse_time(value: &str) -> Option<DateTime<Local>> {
    DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|fixed| fixed.with_timezone(&Local))
}

/// Local `YYYYMMDDTHHMMSS` stamp plus 6 hex chars of SHA-256 over
/// (url, pid, nanos, a per-process counter).
fn new_job_id(url: &str) -> String {
    let now = Local::now();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or(0);
    let sequence = ID_COUNTER.fetch_add(1, Ordering::Relaxed);
    let mut hasher = Sha256::new();
    hasher.update(url.as_bytes());
    hasher.update(std::process::id().to_le_bytes());
    hasher.update(nanos.to_le_bytes());
    hasher.update(sequence.to_le_bytes());
    let digest = hex::encode(hasher.finalize());
    format!("{}-{}", now.format("%Y%m%dT%H%M%S"), &digest[..6])
}

pub(crate) fn ensure_spool_dirs(root: &Path) -> Result<(), String> {
    for child in ["pending", "running", "stuck"] {
        let dir = root.join(child);
        if let Err(error) = fs::create_dir_all(&dir) {
            return Err(format!(
                "create spool directory {}: {error}",
                dir.display()
            ));
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
            let mut builder = fs::DirBuilder::new();
            builder.mode(0o700);
            let _ = builder.create(&dir);
            let _ =
                fs::set_permissions(&dir, fs::Permissions::from_mode(0o700));
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(root, fs::Permissions::from_mode(0o700));
    }
    Ok(())
}

/// Atomically install `bytes` at `dest`: temp file, fsync, rename,
/// directory fsync. The temp file is mode 0600.
fn atomic_install(dest: &Path, bytes: &[u8]) -> Result<(), String> {
    let file_name = dest.file_name().ok_or_else(|| {
        format!("spool path has no file name: {}", dest.display())
    })?;
    let mut temp_name = std::ffi::OsString::from(".");
    temp_name.push(file_name);
    temp_name.push(format!(".{}.tmp", std::process::id()));
    let temp = dest.with_file_name(temp_name);
    let _ = fs::remove_file(&temp);
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp)
        .map_err(|error| {
            format!("create spool temp file {}: {error}", temp.display())
        })?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = file.set_permissions(fs::Permissions::from_mode(0o600));
    }
    use std::io::Write as _;
    if let Err(error) = file.write_all(bytes) {
        let _ = fs::remove_file(&temp);
        return Err(format!(
            "write spool temp file {}: {error}",
            temp.display()
        ));
    }
    if let Err(error) = file.sync_all() {
        let _ = fs::remove_file(&temp);
        return Err(format!(
            "sync spool temp file {}: {error}",
            temp.display()
        ));
    }
    drop(file);
    if let Err(error) = fs::rename(&temp, dest) {
        let _ = fs::remove_file(&temp);
        return Err(format!("install spool file {}: {error}", dest.display()));
    }
    if let Some(parent) = dest.parent() {
        let _ = fs::File::open(parent).and_then(|dir| dir.sync_all());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(dest, fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

/// Queue one job; returns its spool path so capture can delete it on
/// rollback.
pub(crate) fn enqueue(root: &Path, job: &NewJob) -> Result<PathBuf, String> {
    enqueue_at(root, job, now_rfc3339())
}

fn enqueue_at(
    root: &Path,
    job: &NewJob,
    created_at: String,
) -> Result<PathBuf, String> {
    ensure_spool_dirs(root)?;
    let id = new_job_id(&job.url);
    let file = JobFile {
        schema_version: SCHEMA_VERSION,
        id: id.clone(),
        created_at,
        started_at: None,
        source: job.source.clone(),
        bob_dir: job.bob_dir.clone(),
        url: job.url.clone(),
        cleaned_url: job.cleaned_url.clone(),
        dedupe_key: job.dedupe_key.clone(),
        display: job.display.clone(),
        route_hint: job.route_hint.clone(),
        attempts: 0,
        fallback: job.fallback.clone(),
        error: None,
        fallback_error: None,
    };
    let bytes = serde_json::to_vec_pretty(&file)
        .map_err(|error| format!("encode ref job {id}: {error}"))?;
    let dest = root.join("pending").join(format!("{id}.json"));
    atomic_install(&dest, &bytes)?;
    Ok(dest)
}

/// Delete staged job files, e.g. after a capture rollback.
pub(crate) fn remove_created(paths: &[PathBuf]) -> Vec<String> {
    let mut failures = Vec::new();
    for path in paths {
        if let Err(error) = fs::remove_file(path) {
            if error.kind() != std::io::ErrorKind::NotFound {
                failures.push(format!(
                    "remove staged ref job {}: {error}",
                    path.display()
                ));
            }
        }
    }
    failures
}

/// Read one job file; `Err` carries the display path plus the cause.
pub(crate) fn read_job_file(path: &Path) -> Result<JobFile, String> {
    let bytes = fs::read(path)
        .map_err(|error| format!("read ref job {}: {error}", path.display()))?;
    serde_json::from_slice(&bytes)
        .map_err(|error| format!("parse ref job {}: {error}", path.display()))
}

/// Sorted rows of one spool directory: the parsed job plus its
/// path, or the raw error text for an unreadable file. Unreadable
/// files surface as `Err` entries so the worker can park them
/// instead of looping on them forever.
pub(crate) fn dir_rows(
    root: &Path,
    child: &str,
) -> Vec<Result<(JobFile, PathBuf), String>> {
    let mut rows = Vec::new();
    let Ok(entries) = fs::read_dir(root.join(child)) else {
        return rows;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_none_or(|ext| ext != "json") {
            continue;
        }
        match read_job_file(&path) {
            Ok(job) => rows.push(Ok((job, path))),
            Err(error) => {
                rows.push(Err(format!("{}: {error}", path.display())));
            }
        }
    }
    rows.sort_by(|left, right| match (left, right) {
        (Ok((left, _)), Ok((right, _))) => {
            (left.created_at.clone(), left.id.clone())
                .cmp(&(right.created_at.clone(), right.id.clone()))
        }
        (Ok(_), Err(_)) => std::cmp::Ordering::Less,
        (Err(_), Ok(_)) => std::cmp::Ordering::Greater,
        (Err(left), Err(right)) => left.cmp(right),
    });
    rows
}

/// Dedupe keys of pending and running jobs, for the `clipping` verdict.
pub(crate) fn pending_keys(root: &Path) -> HashSet<String> {
    let mut keys = HashSet::new();
    for child in ["pending", "running"] {
        let Ok(entries) = fs::read_dir(root.join(child)) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_none_or(|ext| ext != "json") {
                continue;
            }
            if let Ok(job) = read_job_file(&path) {
                keys.insert(job.dedupe_key.clone());
            }
        }
    }
    keys
}

/// Append one terminal outcome to `done.jsonl` (fsynced), trimming to
/// the last 1000 lines whenever it exceeds 2000.
#[allow(clippy::too_many_arguments)]
pub(crate) fn append_done(
    root: &Path,
    job: &JobFile,
    outcome: DoneOutcome,
    pdf: Option<String>,
    note: Option<String>,
    error: Option<StoredError>,
    fallback_target: Option<String>,
) -> Result<(), String> {
    let _ = fs::create_dir_all(root);
    let record = DoneRecord {
        schema_version: SCHEMA_VERSION,
        id: job.id.clone(),
        url: job.url.clone(),
        cleaned_url: job.cleaned_url.clone(),
        display: job.display.clone(),
        outcome: outcome.as_str().to_string(),
        pdf,
        note,
        error,
        fallback: fallback_target
            .map(|relative_target| DoneFallback { relative_target }),
        created_at: job.created_at.clone(),
        started_at: job.started_at.clone(),
        finished_at: now_rfc3339(),
    };
    let mut line = serde_json::to_string(&record)
        .map_err(|error| format!("encode done record {}: {error}", job.id))?;
    line.push('\n');
    let path = root.join("done.jsonl");
    let file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(|error| {
            format!("open done log {}: {error}", path.display())
        })?;
    {
        use std::io::Write as _;
        let mut file = file;
        file.write_all(line.as_bytes()).map_err(|error| {
            format!("append done log {}: {error}", path.display())
        })?;
        file.sync_all().map_err(|error| {
            format!("sync done log {}: {error}", path.display())
        })?;
    }
    trim_done_log(root)?;
    Ok(())
}

/// Keep the last 1000 `done.jsonl` lines once it exceeds 2000,
/// rewriting atomically under the caller's worker lock.
pub(crate) fn trim_done_log(root: &Path) -> Result<(), String> {
    let path = root.join("done.jsonl");
    let contents = match fs::read_to_string(&path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(());
        }
        Err(error) => {
            return Err(format!("read done log {}: {error}", path.display()));
        }
    };
    let lines: Vec<&str> = contents
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect();
    if lines.len() <= DONE_TRIM_THRESHOLD {
        return Ok(());
    }
    let kept = lines[lines.len() - DONE_TRIM_KEEP..].join("\n") + "\n";
    atomic_install(&path, kept.as_bytes())
}

/// Every `done.jsonl` record, oldest first; corrupt lines are skipped.
pub(crate) fn read_done_records(root: &Path) -> Vec<DoneRecord> {
    let contents =
        fs::read_to_string(root.join("done.jsonl")).unwrap_or_default();
    contents
        .lines()
        .filter(|line| !line.trim().is_empty())
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect()
}

/// The whole `list` view. Without `all`, terminal outcomes older than
/// 7 days are hidden; pending, clipping, and stuck jobs always show.
pub(crate) fn list_jobs(root: &Path, all: bool) -> JobsView {
    let mut view = JobsView::default();
    let cutoff = Local::now() - chrono::Duration::days(DEFAULT_WINDOW_DAYS);
    for entry in dir_rows(root, "pending") {
        match entry {
            Ok((job, path)) => {
                view.count(JobState::Pending);
                view.jobs.push(ListedJob {
                    id: job.id.clone(),
                    state: JobState::Pending,
                    url: job.url.clone(),
                    display: job.display.clone(),
                    created_at: job.created_at.clone(),
                    started_at: None,
                    finished_at: None,
                    pdf: None,
                    note: None,
                    error: None,
                    fallback_target: Some(job.fallback.relative_target.clone()),
                    fallback_error: None,
                    path: Some(path),
                });
            }
            Err(message) => {
                view.count(JobState::Stuck);
                view.jobs.push(unreadable_row(message));
            }
        }
    }
    for entry in dir_rows(root, "running") {
        match entry {
            Ok((job, path)) => {
                view.count(JobState::Clipping);
                view.jobs.push(ListedJob {
                    id: job.id.clone(),
                    state: JobState::Clipping,
                    url: job.url.clone(),
                    display: job.display.clone(),
                    created_at: job.created_at.clone(),
                    started_at: job.started_at.clone(),
                    finished_at: None,
                    pdf: None,
                    note: None,
                    error: None,
                    fallback_target: Some(job.fallback.relative_target.clone()),
                    fallback_error: None,
                    path: Some(path),
                });
            }
            Err(message) => {
                view.count(JobState::Stuck);
                view.jobs.push(unreadable_row(message));
            }
        }
    }
    for entry in dir_rows(root, "stuck") {
        match entry {
            Ok((job, path)) => {
                view.count(JobState::Stuck);
                view.jobs.push(ListedJob {
                    id: job.id.clone(),
                    state: JobState::Stuck,
                    url: job.url.clone(),
                    display: job.display.clone(),
                    created_at: job.created_at.clone(),
                    started_at: job.started_at.clone(),
                    finished_at: None,
                    pdf: None,
                    note: None,
                    error: job.error.clone(),
                    fallback_target: Some(job.fallback.relative_target.clone()),
                    fallback_error: job.fallback_error.clone(),
                    path: Some(path),
                });
            }
            Err(message) => {
                view.count(JobState::Stuck);
                view.jobs.push(unreadable_row(message));
            }
        }
    }
    for record in read_done_records(root) {
        let state = match record.outcome.as_str() {
            "created" => JobState::Clipped,
            "already_in_library" => JobState::InLibrary,
            "already_queued" => JobState::AlreadyQueued,
            _ => JobState::FellBack,
        };
        if !all
            && let Some(finished) = parse_time(&record.finished_at)
            && finished < cutoff
        {
            continue;
        }
        view.count(state);
        view.jobs.push(ListedJob {
            id: record.id.clone(),
            state,
            url: record.url.clone(),
            display: record.display.clone(),
            created_at: record.created_at.clone(),
            started_at: record.started_at.clone(),
            finished_at: Some(record.finished_at.clone()),
            pdf: record.pdf.clone(),
            note: record.note.clone(),
            error: record.error.clone(),
            fallback_target: record
                .fallback
                .map(|fallback| fallback.relative_target),
            fallback_error: None,
            path: None,
        });
    }
    sort_view(&mut view);
    view
}

fn unreadable_row(message: String) -> ListedJob {
    let id = message
        .rsplit('/')
        .next()
        .unwrap_or(&message)
        .trim_end_matches(".json: ")
        .to_string();
    ListedJob {
        id,
        state: JobState::Stuck,
        url: String::new(),
        display: "unreadable job file".to_string(),
        created_at: String::new(),
        started_at: None,
        finished_at: None,
        pdf: None,
        note: None,
        error: Some(StoredError {
            kind: "internal".to_string(),
            message,
            retryable: false,
        }),
        fallback_target: None,
        fallback_error: None,
        path: None,
    }
}

fn state_order(state: JobState) -> u8 {
    match state {
        JobState::Pending => 0,
        JobState::Clipping => 1,
        JobState::Clipped => 2,
        JobState::InLibrary => 3,
        JobState::AlreadyQueued => 4,
        JobState::FellBack => 5,
        JobState::Stuck => 6,
    }
}

fn sort_view(view: &mut JobsView) {
    view.jobs.sort_by(|left, right| {
        state_order(left.state)
            .cmp(&state_order(right.state))
            .then_with(|| left.created_at.cmp(&right.created_at))
            .then_with(|| left.id.cmp(&right.id))
    });
}

/// Move a parsed job file between spool directories, optionally
/// stamping `started_at` and bumping `attempts`.
pub(crate) fn move_job(
    from: &Path,
    to: &Path,
    stamp_started: bool,
    bump_attempts: bool,
) -> Result<JobFile, String> {
    let mut job = read_job_file(from)?;
    if stamp_started {
        job.started_at = Some(now_rfc3339());
    }
    if bump_attempts {
        job.attempts += 1;
    }
    let bytes = serde_json::to_vec_pretty(&job)
        .map_err(|error| format!("encode ref job {}: {error}", job.id))?;
    atomic_install(to, &bytes)?;
    fs::remove_file(from).map_err(|error| {
        format!("remove claimed ref job {}: {error}", from.display())
    })?;
    Ok(job)
}

/// Oldest pending age in whole seconds, for the doctor row.
pub(crate) fn oldest_pending_age_secs(root: &Path) -> Option<i64> {
    let now = Local::now();
    dir_rows(root, "pending")
        .into_iter()
        .filter_map(|entry| entry.ok())
        .filter_map(|(job, _)| parse_time(&job.created_at))
        .map(|created| (now - created).num_seconds().max(0))
        .max()
}

/// Count pending and stuck jobs, for the doctor row.
pub(crate) fn pending_and_stuck_counts(root: &Path) -> (usize, usize) {
    let pending_rows = dir_rows(root, "pending");
    let pending = pending_rows.iter().filter(|entry| entry.is_ok()).count();
    let stuck = dir_rows(root, "stuck").len()
        + pending_rows.iter().filter(|entry| entry.is_err()).count()
        + dir_rows(root, "running")
            .iter()
            .filter(|entry| entry.is_err())
            .count();
    (pending, stuck)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "bob-cli-ref-jobs-test-{name}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    fn sample_job() -> NewJob {
        NewJob {
            source: "capture".to_string(),
            bob_dir: PathBuf::from("/home/bryan/bob"),
            url: "https://example.com/post?utm_source=x".to_string(),
            cleaned_url: "https://example.com/post".to_string(),
            dedupe_key: "https://example.com/post".to_string(),
            display: "example.com/post".to_string(),
            route_hint: "article".to_string(),
            fallback: JobFallback {
                relative_target: "mac_inbox.md".to_string(),
                task_line: "- [ ] #task https://example.com/post?utm_source=x [created::2026-10-07]".to_string(),
            },
        }
    }

    #[test]
    fn job_json_round_trips_with_all_fields() {
        let root = temp_root("round-trip").join("jobs");
        let path = enqueue(&root, &sample_job()).expect("enqueue job");
        assert!(path.starts_with(root.join("pending")));
        let job = read_job_file(&path).expect("read job");
        assert_eq!(job.schema_version, 1);
        assert_eq!(job.attempts, 0);
        assert!(job.started_at.is_none());
        assert!(job.error.is_none());
        let stem = path
            .file_stem()
            .expect("file stem")
            .to_string_lossy()
            .into_owned();
        assert_eq!(job.id, stem);
        assert_eq!(job.source, "capture");
        assert_eq!(job.fallback.relative_target, "mac_inbox.md");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode =
                fs::metadata(&path).expect("stat job").permissions().mode()
                    & 0o777;
            assert_eq!(mode, 0o600, "job file mode");
        }
    }

    #[test]
    fn job_ids_are_unique_and_shaped() {
        let root = temp_root("ids").join("jobs");
        let first = enqueue(&root, &sample_job()).expect("first");
        let second = enqueue(&root, &sample_job()).expect("second");
        assert_ne!(first, second);
        for path in [first, second] {
            let name = path
                .file_name()
                .expect("name")
                .to_string_lossy()
                .into_owned();
            assert_eq!(
                name.len(),
                "20261007T143012-abcdef.json".len(),
                "{name}"
            );
            assert!(name.ends_with(".json"));
            assert!(name.contains('T') && name.contains('-'));
        }
    }

    #[test]
    fn pending_keys_covers_pending_and_running() {
        let root = temp_root("keys").join("jobs");
        enqueue(&root, &sample_job()).expect("enqueue");
        let mut other = sample_job();
        other.cleaned_url = "https://example.com/other".to_string();
        other.dedupe_key = "https://example.com/other".to_string();
        other.url = "https://example.com/other".to_string();
        enqueue(&root, &other).expect("enqueue other");
        let keys = pending_keys(&root);
        assert_eq!(keys.len(), 2);
        assert!(keys.contains("https://example.com/post"));
        let entries = dir_rows(&root, "pending");
        assert_eq!(entries.len(), 2);
        if let Ok((_, path)) = &entries[0] {
            let running =
                root.join("running").join(path.file_name().expect("name"));
            move_job(path, &running, true, false).expect("claim");
        }
        let keys = pending_keys(&root);
        assert_eq!(keys.len(), 2, "running jobs still count");
    }

    #[test]
    fn done_log_trims_to_last_thousand() {
        let root = temp_root("trim").join("jobs");
        fs::create_dir_all(&root).expect("create spool");
        let mut contents = String::new();
        for index in 0..2001 {
            contents.push_str(&format!(
                "{{\"schema_version\":1,\"id\":\"job-{index:04}\",\"url\":\"https://example.com/{index}\",\"cleaned_url\":\"https://example.com/{index}\",\"display\":\"example.com/{index}\",\"outcome\":\"created\",\"created_at\":\"2026-10-01T00:00:00-04:00\",\"finished_at\":\"2026-10-07T00:00:00-04:00\"}}\n",
            ));
        }
        fs::write(root.join("done.jsonl"), &contents).expect("seed log");
        trim_done_log(&root).expect("trim");
        let records = read_done_records(&root);
        assert_eq!(records.len(), 1000);
        assert_eq!(records.first().expect("first").id, "job-1001");
        assert_eq!(records.last().expect("last").id, "job-2000");
    }

    #[test]
    fn corrupt_done_lines_are_skipped() {
        let root = temp_root("corrupt").join("jobs");
        fs::create_dir_all(&root).expect("create spool");
        fs::write(
            root.join("done.jsonl"),
            "not json\n{\"schema_version\":1,\"id\":\"ok\",\"url\":\"u\",\"cleaned_url\":\"u\",\"display\":\"d\",\"outcome\":\"created\",\"created_at\":\"2026-10-01T00:00:00-04:00\",\"finished_at\":\"2026-10-07T00:00:00-04:00\"}\n",
        )
        .expect("seed log");
        let records = read_done_records(&root);
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].id, "ok");
    }
}
