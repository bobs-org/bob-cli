//! Captured vault inputs for archive planning.
//!
//! Every planning decision reads from one captured input set: source notes,
//! repair-scope notes, absent archive destinations, and Tasks settings. Disk
//! fallbacks are not allowed downstream: readers take the captured contents
//! map, and an unstable capture surfaces as a retryable, path-bearing error
//! so a later command can rebuild the plan instead of overwriting the vault.
use super::*;

use super::super::task_status_hooks::{parse_tasks_settings, TasksSettings};
use super::super::task_status_hooks_write::{
    capture_optional, capture_required, CaptureError, InputKind, InputSnapshot,
};

pub(super) struct CapturedInputs {
    pub(super) vault: PathBuf,
    pub(super) vault_canonical: PathBuf,
    pub(super) source_paths: Vec<PathBuf>,
    pub(super) repair_paths: Vec<PathBuf>,
    pub(super) snapshots: Vec<InputSnapshot>,
    pub(super) contents: BTreeMap<PathBuf, String>,
    pub(super) settings: TasksSettings,
    pub(super) daily_path: PathBuf,
    pub(super) daily_canonical: Option<PathBuf>,
}

impl CapturedInputs {
    pub(super) fn contents(&self, relative_path: &Path) -> io::Result<&str> {
        self.contents
            .get(relative_path)
            .map(String::as_str)
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!(
                        "missing captured contents for {}",
                        relative_path.display()
                    ),
                )
            })
    }
}

/// Capture every archive-planning input exactly once.
///
/// Source paths come from the existing source walker (which excludes
/// `done/`); repair paths come from the wider link-repair walker (which
/// includes `done/`). Snapshots are deduplicated by absolute path. Absent
/// canonical archive destinations (`done/<parent>/<stem>_done.md`) and Tasks
/// settings are captured as optional inputs so a file appearing underfoot
/// invalidates the plan instead of being overwritten. The pair index is built
/// from this census; a later rename phase reuses the already-captured
/// canonical slot.
pub(super) fn capture_inputs(vault: &Path) -> io::Result<CapturedInputs> {
    let vault_canonical = vault
        .canonicalize()
        .map_err(|error| fs_error("canonicalize", vault, error))?;
    let source_paths = markdown_files(vault)?;
    let repair_paths = link_repair_markdown_files(vault)?;
    let daily_path = super::super::pomodoro::day_file_for(vault);

    let mut snapshots = Vec::new();
    let mut contents = BTreeMap::new();

    for path in source_paths.iter().chain(repair_paths.iter()) {
        if snapshots
            .iter()
            .any(|snapshot: &InputSnapshot| snapshot.path == *path)
        {
            continue;
        }
        let relative_path = vault_relative_path(vault, path, "note")?;
        let kind = input_kind_for(&relative_path);
        let snapshot = capture_required(path, kind).map_err(capture_error)?;
        if let Some(text) = snapshot.utf8_contents().map_err(|error| {
            io::Error::new(io::ErrorKind::InvalidData, error.message())
        })? {
            contents.insert(relative_path, text);
        }
        if !snapshots
            .iter()
            .any(|existing: &InputSnapshot| existing.path == snapshot.path)
        {
            snapshots.push(snapshot);
        }
    }

    for path in &source_paths {
        let relative_source_path = vault_relative_path(vault, path, "note")?;
        let relative_archive_path =
            archive_relative_path(&relative_source_path)?;
        let archive_path = vault.join(&relative_archive_path);
        if snapshots
            .iter()
            .any(|snapshot: &InputSnapshot| snapshot.path == archive_path)
        {
            continue;
        }
        let snapshot = capture_optional(&archive_path, InputKind::Archive)
            .map_err(capture_error)?;
        if let Some(text) = snapshot.utf8_contents().map_err(|error| {
            io::Error::new(io::ErrorKind::InvalidData, error.message())
        })? {
            contents.insert(relative_archive_path, text);
        }
        if !snapshots
            .iter()
            .any(|existing: &InputSnapshot| existing.path == snapshot.path)
        {
            snapshots.push(snapshot);
        }
    }

    let settings_path = vault
        .join(super::super::task_status_hooks::tasks_settings_relative_path());
    let settings_snapshot =
        capture_optional(&settings_path, InputKind::TasksSettings)
            .map_err(capture_error)?;
    let settings_contents = settings_snapshot
        .utf8_contents()
        .map_err(|error| {
            io::Error::new(io::ErrorKind::InvalidData, error.message())
        })?
        .map(|text| text.to_string());
    let settings =
        parse_tasks_settings(&settings_path, settings_contents.as_deref());
    if !snapshots
        .iter()
        .any(|existing: &InputSnapshot| existing.path == settings_snapshot.path)
    {
        snapshots.push(settings_snapshot);
    }

    Ok(CapturedInputs {
        vault: vault.to_path_buf(),
        vault_canonical,
        source_paths,
        repair_paths,
        snapshots,
        contents,
        settings,
        daily_path: daily_path.clone(),
        daily_canonical: fs::canonicalize(&daily_path).ok(),
    })
}

/// The note membership the planner scans: the source walker plus the wider
/// link-repair walker, sorted and deduplicated.
///
/// The guarded apply compares this same scope against the captured plan, so a
/// note discovered after planning invalidates the plan instead of being
/// silently missed.
pub(super) fn scan_membership(vault: &Path) -> io::Result<Vec<PathBuf>> {
    let mut paths = markdown_files(vault)?;
    paths.extend(link_repair_markdown_files(vault)?);
    paths.sort();
    paths.dedup();
    Ok(paths)
}

fn input_kind_for(relative_path: &Path) -> InputKind {
    if super::super::task_dependencies::is_archive_path(relative_path) {
        InputKind::Archive
    } else {
        InputKind::Note
    }
}

/// Map a capture failure to a path-bearing `io::Error`.
///
/// `Unstable` (the file changed while it was read) maps to
/// `ErrorKind::Interrupted`, the retryable signal: the plan must be rebuilt,
/// never applied. Every message carries the offending path.
pub(super) fn capture_error(error: CaptureError) -> io::Error {
    let kind = match &error {
        CaptureError::Unstable(_) => io::ErrorKind::Interrupted,
        CaptureError::Unsupported { .. } => io::ErrorKind::InvalidInput,
        CaptureError::NotFound(_) => io::ErrorKind::InvalidData,
        CaptureError::InvalidUtf8(_) => io::ErrorKind::InvalidData,
        CaptureError::Io { path, error } => {
            return io::Error::new(
                error.kind(),
                format!("{}: {error}", path.display()),
            );
        }
    };
    io::Error::new(kind, error.message())
}
