//! Guarded note writes for `bob task-status-hooks`.
//!
//! Snapshot the planning read-set, stage replacements as uniquely created
//! temporaries, retain recoverable originals, and refuse to overwrite a vault
//! that changed underfoot. Scoped to this command; other writers are unchanged.

#![allow(clippy::result_large_err, clippy::type_complexity)]

use std::{
    collections::BTreeSet,
    ffi::{OsStr, OsString},
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{env as bob_env, ob};

const TOOL: &str = "task-status-hooks";
const SCHEMA_VERSION: u32 = 1;
const STATE_SUBDIR: &str = "task-status-hooks";
pub(crate) const QUIET_PERIOD: Duration = Duration::from_secs(2);
pub(crate) const RETENTION: Duration = Duration::from_secs(30 * 24 * 60 * 60);

#[cfg(target_os = "linux")]
const O_NOFOLLOW: i32 = 0o400000;

static RUN_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReasonCode {
    LockContention,
    VaultChanged,
    QuietPeriod,
    PartialApply,
    RecoveryFailed,
    UnstableRead,
    UnsupportedFile,
    Io,
}

impl ReasonCode {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::LockContention => "lock_contention",
            Self::VaultChanged => "vault_changed",
            Self::QuietPeriod => "quiet_period",
            Self::PartialApply => "partial_apply",
            Self::RecoveryFailed => "recovery_failed",
            Self::UnstableRead => "unstable_read",
            Self::UnsupportedFile => "unsupported_file",
            Self::Io => "io",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum InputKind {
    Note,
    Daily,
    PreviousDaily,
    Archive,
    TasksSettings,
}

#[derive(Debug, Clone)]
pub(crate) struct FileIdentity {
    pub canonical_path: PathBuf,
    pub dev: u64,
    pub ino: u64,
    pub nlink: u64,
    pub mode: u32,
    pub len: u64,
    pub mtime: Option<SystemTime>,
}

impl PartialEq for FileIdentity {
    fn eq(&self, other: &Self) -> bool {
        self.canonical_path == other.canonical_path
            && self.dev == other.dev
            && self.ino == other.ino
            && self.nlink == other.nlink
            && self.mode == other.mode
            && self.len == other.len
    }
}

impl Eq for FileIdentity {}

#[derive(Debug, Clone)]
pub(crate) enum InputState {
    Missing,
    Present {
        identity: FileIdentity,
        bytes: Vec<u8>,
    },
}

#[derive(Debug, Clone)]
pub(crate) struct InputSnapshot {
    pub path: PathBuf,
    pub kind: InputKind,
    pub state: InputState,
}

impl InputSnapshot {
    pub(crate) fn missing(path: impl Into<PathBuf>, kind: InputKind) -> Self {
        Self {
            path: path.into(),
            kind,
            state: InputState::Missing,
        }
    }

    pub(crate) fn is_missing(&self) -> bool {
        matches!(self.state, InputState::Missing)
    }

    pub(crate) fn bytes(&self) -> Option<&[u8]> {
        match &self.state {
            InputState::Present { bytes, .. } => Some(bytes),
            InputState::Missing => None,
        }
    }

    pub(crate) fn identity(&self) -> Option<&FileIdentity> {
        match &self.state {
            InputState::Present { identity, .. } => Some(identity),
            InputState::Missing => None,
        }
    }

    pub(crate) fn utf8_contents(&self) -> Result<Option<String>, CaptureError> {
        match &self.state {
            InputState::Missing => Ok(None),
            InputState::Present { bytes, .. } => {
                String::from_utf8(bytes.clone())
                    .map(Some)
                    .map_err(|_| CaptureError::InvalidUtf8(self.path.clone()))
            }
        }
    }

    fn matches(&self, other: &Self) -> bool {
        match (&self.state, &other.state) {
            (InputState::Missing, InputState::Missing) => true,
            (
                InputState::Present {
                    identity: left_id,
                    bytes: left_bytes,
                },
                InputState::Present {
                    identity: right_id,
                    bytes: right_bytes,
                },
            ) => left_id == right_id && left_bytes == right_bytes,
            _ => false,
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct PlannedWrite {
    pub path: PathBuf,
    pub original_bytes: Vec<u8>,
    pub proposed_bytes: Vec<u8>,
    pub identity: FileIdentity,
    /// Integration sets this for status-group rearrangements so the quiet
    /// interval applies without duplicating application logic.
    pub structural_regrouping: bool,
}

#[derive(Debug, Clone)]
pub(crate) struct WritePlan {
    pub vault_canonical: PathBuf,
    pub inputs: Vec<InputSnapshot>,
    pub scan_paths: Vec<PathBuf>,
    pub outputs: Vec<PlannedWrite>,
}

pub(crate) struct ApplySession {
    pub state_home: PathBuf,
    pub quiet_period: Duration,
    pub retention: Duration,
    pub run_id: String,
    pub now_system: Box<dyn Fn() -> SystemTime>,
    pub sleep: Box<dyn Fn(Duration)>,
    pub rescan: Box<dyn Fn() -> io::Result<Vec<PathBuf>>>,
    pub before_preflight: Option<Box<dyn Fn()>>,
    pub after_staging: Option<Box<dyn Fn()>>,
    pub before_replace: Option<Box<dyn Fn(&Path)>>,
    pub fail_staging: Option<Box<dyn Fn(&Path) -> io::Result<()>>>,
}

impl ApplySession {
    pub(crate) fn production(
        rescan: Box<dyn Fn() -> io::Result<Vec<PathBuf>>>,
    ) -> Self {
        Self {
            state_home: bob_env::state_home(),
            quiet_period: QUIET_PERIOD,
            retention: RETENTION,
            run_id: new_run_id(),
            now_system: Box::new(SystemTime::now),
            sleep: Box::new(std::thread::sleep),
            rescan,
            before_preflight: None,
            after_staging: None,
            before_replace: None,
            fail_staging: None,
        }
    }

    fn now(&self) -> SystemTime {
        (self.now_system)()
    }
}

#[derive(Debug)]
pub(crate) enum CaptureError {
    NotFound(PathBuf),
    Unstable(PathBuf),
    Unsupported { path: PathBuf, message: String },
    InvalidUtf8(PathBuf),
    Io { path: PathBuf, error: io::Error },
}

impl CaptureError {
    fn io(path: &Path, error: io::Error) -> Self {
        Self::Io {
            path: path.to_path_buf(),
            error,
        }
    }

    pub(crate) fn message(&self) -> String {
        match self {
            Self::NotFound(path) => {
                format!("file does not exist: {}", path.display())
            }
            Self::Unstable(path) => format!(
                "file changed while it was being read: {}",
                path.display()
            ),
            Self::Unsupported { path, message } => {
                format!("{}: {message}", path.display())
            }
            Self::InvalidUtf8(path) => {
                format!("note is not valid UTF-8: {}", path.display())
            }
            Self::Io { path, error } => {
                format!("{}: {error}", path.display())
            }
        }
    }
}

#[derive(Debug)]
pub(crate) struct ApplyError {
    pub reason: ReasonCode,
    pub message: String,
    pub applied_files: Vec<PathBuf>,
    pub deferred_files: Vec<PathBuf>,
    pub recovery_directory: Option<PathBuf>,
}

impl ApplyError {
    fn lock_contention() -> Self {
        Self {
            reason: ReasonCode::LockContention,
            message: "another Bob vault maintenance run is already active; rerun later"
                .to_string(),
            applied_files: Vec::new(),
            deferred_files: Vec::new(),
            recovery_directory: None,
        }
    }

    fn lock_io(path: &Path, error: io::Error) -> Self {
        Self {
            reason: ReasonCode::Io,
            message: format!(
                "failed to acquire maintenance lock {}: {error}",
                path.display()
            ),
            applied_files: Vec::new(),
            deferred_files: Vec::new(),
            recovery_directory: None,
        }
    }

    fn capture(reason: ReasonCode, error: CaptureError) -> Self {
        Self {
            reason,
            message: error.message(),
            applied_files: Vec::new(),
            deferred_files: Vec::new(),
            recovery_directory: None,
        }
    }

    fn io(
        message: String,
        applied: Vec<PathBuf>,
        remaining: Vec<PathBuf>,
        recovery: Option<PathBuf>,
    ) -> Self {
        let reason = if applied.is_empty() {
            ReasonCode::Io
        } else {
            ReasonCode::PartialApply
        };
        Self {
            reason,
            message,
            deferred_files: remaining,
            applied_files: applied,
            recovery_directory: recovery,
        }
    }

    fn vault_changed(
        plan: &WritePlan,
        applied: &[PathBuf],
        recovery: Option<PathBuf>,
        detail: impl Into<String>,
    ) -> Self {
        let remaining = remaining_outputs(plan, applied);
        let reason = if applied.is_empty() {
            ReasonCode::VaultChanged
        } else {
            ReasonCode::PartialApply
        };
        let message = if applied.is_empty() {
            format!("the vault changed ({}); rerun the command", detail.into())
        } else {
            format!(
                "applied {} note(s) then stopped because the vault changed ({}); remaining notes were not written",
                applied.len(),
                detail.into()
            )
        };
        Self {
            reason,
            message,
            deferred_files: remaining,
            applied_files: applied.to_vec(),
            recovery_directory: recovery,
        }
    }

    fn quiet_period(plan: &WritePlan, detail: impl Into<String>) -> Self {
        let remaining = remaining_outputs(plan, &[]);
        Self {
            reason: ReasonCode::QuietPeriod,
            message: format!(
                "a status-group target was still being saved ({}); rerun the command",
                detail.into()
            ),
            applied_files: Vec::new(),
            deferred_files: remaining,
            recovery_directory: None,
        }
    }
}

#[derive(Debug)]
pub(crate) enum ApplyOutcome {
    NoOp,
    Applied {
        applied_files: Vec<PathBuf>,
        recovery_directory: PathBuf,
    },
}

#[derive(Debug, Serialize, Deserialize)]
struct RecoveryManifest {
    tool: String,
    schema_version: u32,
    vault: String,
    vault_hash: String,
    run_id: String,
    started_at: String,
    started_at_unix: u64,
    completed_at: Option<String>,
    completed_at_unix: Option<u64>,
    outcome: String,
    notes: Vec<RecoveryNote>,
}

#[derive(Debug, Serialize, Deserialize)]
struct RecoveryNote {
    path: String,
    original_hash: String,
    proposed_hash: String,
    state: String,
    original_file: String,
    proposed_file: String,
}

struct StagedWrite {
    dest: PathBuf,
    temp: PathBuf,
    proposed_bytes: Vec<u8>,
}

struct AppliedWrite {
    path: PathBuf,
    proposed_bytes: Vec<u8>,
}

pub(crate) fn new_run_id() -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    format!(
        "{now:x}-{:x}-{:x}",
        std::process::id(),
        RUN_COUNTER.fetch_add(1, Ordering::Relaxed)
    )
}

pub(crate) fn acquire_maintenance_lock() -> Result<File, ApplyError> {
    match ob::try_acquire_lock() {
        Ok(file) => Ok(file),
        Err(ob::LockAcquireError::Contended) => {
            Err(ApplyError::lock_contention())
        }
        Err(ob::LockAcquireError::Open { path, error })
        | Err(ob::LockAcquireError::Acquire { path, error }) => {
            Err(ApplyError::lock_io(&path, error))
        }
    }
}

pub(crate) fn capture_required(
    path: &Path,
    kind: InputKind,
) -> Result<InputSnapshot, CaptureError> {
    match capture_optional(path, kind)? {
        snapshot if snapshot.is_missing() => {
            Err(CaptureError::NotFound(path.to_path_buf()))
        }
        snapshot => Ok(snapshot),
    }
}

pub(crate) fn capture_optional(
    path: &Path,
    kind: InputKind,
) -> Result<InputSnapshot, CaptureError> {
    match capture_present(path, kind) {
        Ok(snapshot) => Ok(snapshot),
        Err(CaptureError::NotFound(_)) => {
            Ok(InputSnapshot::missing(path, kind))
        }
        Err(error) => Err(error),
    }
}

pub(crate) fn planned_write(
    path: PathBuf,
    original: &InputSnapshot,
    proposed_bytes: Vec<u8>,
    structural_regrouping: bool,
) -> Result<PlannedWrite, CaptureError> {
    let (identity, original_bytes) = match &original.state {
        InputState::Present { identity, bytes } => {
            (identity.clone(), bytes.clone())
        }
        InputState::Missing => {
            return Err(CaptureError::NotFound(path));
        }
    };
    if identity.nlink != 1 {
        return Err(CaptureError::Unsupported {
            path,
            message: "refusing to replace a multiply linked file".to_string(),
        });
    }
    Ok(PlannedWrite {
        path,
        original_bytes,
        proposed_bytes,
        identity,
        structural_regrouping,
    })
}

pub(crate) fn snapshot_for_path<'a>(
    inputs: &'a [InputSnapshot],
    path: &Path,
) -> Option<&'a InputSnapshot> {
    let canonical = path.canonicalize().ok();
    inputs.iter().find(|input| {
        if input.path == path {
            return true;
        }
        match (&canonical, input.identity()) {
            (Some(canonical), Some(identity)) => {
                identity.canonical_path == *canonical
            }
            _ => false,
        }
    })
}

pub(crate) fn apply_plan(
    plan: &WritePlan,
    session: &ApplySession,
) -> Result<ApplyOutcome, ApplyError> {
    if plan.outputs.is_empty() {
        return Ok(ApplyOutcome::NoOp);
    }

    wait_quiet_period(plan, session)?;
    if let Some(callback) = &session.before_preflight {
        callback();
    }
    preflight(plan, session, &[])?;

    let recovery_dir = create_recovery(plan, session)?;
    let staged = stage_outputs(plan, session, &recovery_dir)?;
    if let Some(callback) = &session.after_staging {
        callback();
    }
    if let Err(mut error) = preflight(plan, session, &[]) {
        cleanup_temps(&staged);
        if error.recovery_directory.is_none() {
            error.recovery_directory = Some(recovery_dir);
        }
        return Err(error);
    }

    let mut applied = Vec::new();
    let mut remaining_temps = staged;
    while !remaining_temps.is_empty() {
        let next = remaining_temps.remove(0);
        if let Some(callback) = &session.before_replace {
            callback(&next.dest);
        }
        if let Err(error) = preflight_for_replacement(plan, &next, &applied) {
            cleanup_temps(&remaining_temps);
            let _ = update_manifest(
                &recovery_dir,
                plan,
                session,
                "partial",
                &applied,
            );
            let remaining = remaining_outputs(
                plan,
                &applied
                    .iter()
                    .map(|item| item.path.clone())
                    .collect::<Vec<_>>(),
            );
            return Err(ApplyError {
                reason: ReasonCode::PartialApply,
                message: format!(
                    "applied {} note(s) then stopped ({}); remaining notes were not written",
                    applied.len(),
                    error.message
                ),
                applied_files: applied.iter().map(|item| item.path.clone()).collect(),
                deferred_files: remaining,
                    recovery_directory: Some(recovery_dir),
            });
        }
        if let Err(error) = fs::rename(&next.temp, &next.dest) {
            cleanup_temps(&remaining_temps);
            let _ = fs::remove_file(&next.temp);
            let _ = update_manifest(
                &recovery_dir,
                plan,
                session,
                "partial",
                &applied,
            );
            let applied_paths = applied
                .iter()
                .map(|item| item.path.clone())
                .collect::<Vec<_>>();
            let remaining = remaining_outputs(plan, &applied_paths);
            return Err(ApplyError::io(
                format!("failed to replace {}: {error}", next.dest.display()),
                applied_paths,
                remaining,
                Some(recovery_dir),
            ));
        }
        applied.push(AppliedWrite {
            path: next.dest.clone(),
            proposed_bytes: next.proposed_bytes.clone(),
        });
        if let Err(error) =
            update_manifest(&recovery_dir, plan, session, "partial", &applied)
        {
            eprintln!(
                "bob task-status-hooks: warning: failed to update recovery manifest {}: {error}",
                recovery_dir.display()
            );
        }
    }

    if let Err(error) =
        update_manifest(&recovery_dir, plan, session, "applied", &applied)
    {
        eprintln!(
            "bob task-status-hooks: warning: failed to finalize recovery manifest {}: {error}",
            recovery_dir.display()
        );
    }
    if let Err(error) = prune_completed(
        &session.state_home.join("bob-cli").join(STATE_SUBDIR),
        session.now(),
        session.retention,
    ) {
        eprintln!(
            "bob task-status-hooks: warning: failed to prune old recovery records: {error}"
        );
    }

    Ok(ApplyOutcome::Applied {
        applied_files: applied.into_iter().map(|item| item.path).collect(),
        recovery_directory: recovery_dir,
    })
}

fn remaining_outputs(plan: &WritePlan, applied: &[PathBuf]) -> Vec<PathBuf> {
    plan.outputs
        .iter()
        .map(|output| output.path.clone())
        .filter(|path| !applied.iter().any(|applied| applied == path))
        .collect()
}

fn wait_quiet_period(
    plan: &WritePlan,
    session: &ApplySession,
) -> Result<(), ApplyError> {
    let structural = plan
        .outputs
        .iter()
        .filter(|output| output.structural_regrouping)
        .collect::<Vec<_>>();
    if structural.is_empty() {
        return Ok(());
    }

    let now = session.now();
    let mut wait = Duration::ZERO;
    for output in &structural {
        let metadata = fs::symlink_metadata(&output.path).map_err(|error| {
            ApplyError::quiet_period(
                plan,
                format!("could not read {}: {error}", output.path.display()),
            )
        })?;
        let mtime = metadata.modified().ok().or(output.identity.mtime);
        let Some(mtime) = mtime else {
            return Err(ApplyError::quiet_period(
                plan,
                format!(
                    "modification time of {} is uncertain",
                    output.path.display()
                ),
            ));
        };
        match now.duration_since(mtime) {
            Ok(age) => {
                wait = wait.max(session.quiet_period.saturating_sub(age));
            }
            Err(_) => {
                return Err(ApplyError::quiet_period(
                    plan,
                    format!(
                        "modification time of {} is in the future",
                        output.path.display()
                    ),
                ));
            }
        }
    }
    if wait > session.quiet_period {
        wait = session.quiet_period;
    }
    if wait > Duration::ZERO {
        (session.sleep)(wait);
    }
    Ok(())
}

fn preflight(
    plan: &WritePlan,
    session: &ApplySession,
    applied: &[AppliedWrite],
) -> Result<(), ApplyError> {
    let applied_paths = applied
        .iter()
        .map(|item| item.path.clone())
        .collect::<Vec<_>>();
    for input in &plan.inputs {
        let current = recapture(input).map_err(|error| {
            let reason = match error {
                CaptureError::Unstable(_) => ReasonCode::UnstableRead,
                CaptureError::Unsupported { .. } => ReasonCode::UnsupportedFile,
                _ => ReasonCode::VaultChanged,
            };
            if applied.is_empty() {
                ApplyError::capture(reason, error)
            } else {
                ApplyError::vault_changed(
                    plan,
                    &applied_paths,
                    None,
                    error.message(),
                )
            }
        })?;
        if let Some(applied_item) = applied.iter().find(|item| {
            item.path == input.path
                || current.identity().is_some_and(|identity| {
                    fs::canonicalize(&item.path).ok().as_ref()
                        == Some(&identity.canonical_path)
                })
        }) {
            if current.bytes() != Some(applied_item.proposed_bytes.as_slice()) {
                return Err(ApplyError::vault_changed(
                    plan,
                    &applied_paths,
                    None,
                    format!(
                        "already-written {} changed after replacement",
                        applied_item.path.display()
                    ),
                ));
            }
            continue;
        }
        if !input.matches(&current) {
            return Err(ApplyError::vault_changed(
                plan,
                &applied_paths,
                None,
                format!("{} changed", input.path.display()),
            ));
        }
    }

    let scanned = (session.rescan)().map_err(|error| {
        ApplyError::io(
            format!("failed to rescan vault: {error}"),
            applied_paths.clone(),
            remaining_outputs(plan, &applied_paths),
            None,
        )
    })?;
    let original: BTreeSet<&Path> =
        plan.scan_paths.iter().map(PathBuf::as_path).collect();
    let current: BTreeSet<&Path> =
        scanned.iter().map(PathBuf::as_path).collect();
    if original != current {
        return Err(ApplyError::vault_changed(
            plan,
            &applied_paths,
            None,
            "the set of scanned notes changed",
        ));
    }

    for output in &plan.outputs {
        if applied.iter().any(|item| item.path == output.path) {
            continue;
        }
        check_unwritten_output(output).map_err(|error| {
            ApplyError::vault_changed(
                plan,
                &applied_paths,
                None,
                error.message.clone(),
            )
        })?;
    }
    Ok(())
}

fn preflight_for_replacement(
    plan: &WritePlan,
    next: &StagedWrite,
    applied: &[AppliedWrite],
) -> Result<(), ApplyError> {
    for item in applied {
        let current =
            capture_required(&item.path, InputKind::Note).map_err(|error| {
                ApplyError::capture(ReasonCode::VaultChanged, error)
            })?;
        if current.bytes() != Some(item.proposed_bytes.as_slice()) {
            return Err(ApplyError::vault_changed(
                plan,
                &applied
                    .iter()
                    .map(|item| item.path.clone())
                    .collect::<Vec<_>>(),
                None,
                format!(
                    "already-written {} changed after replacement",
                    item.path.display()
                ),
            ));
        }
    }
    let output = plan
        .outputs
        .iter()
        .find(|output| output.path == next.dest)
        .expect("staged path is part of the plan");
    check_unwritten_output(output)
}

fn recapture(input: &InputSnapshot) -> Result<InputSnapshot, CaptureError> {
    match input.state {
        InputState::Missing => capture_optional(&input.path, input.kind),
        InputState::Present { .. } => capture_required(&input.path, input.kind),
    }
}

fn check_unwritten_output(output: &PlannedWrite) -> Result<(), ApplyError> {
    let current =
        capture_required(&output.path, InputKind::Note).map_err(|error| {
            let reason = match error {
                CaptureError::Unstable(_) => ReasonCode::UnstableRead,
                CaptureError::Unsupported { .. } => ReasonCode::UnsupportedFile,
                CaptureError::NotFound(_) => ReasonCode::VaultChanged,
                _ => ReasonCode::Io,
            };
            ApplyError::capture(reason, error)
        })?;
    let identity = current.identity().ok_or_else(|| {
        ApplyError::capture(
            ReasonCode::VaultChanged,
            CaptureError::NotFound(output.path.clone()),
        )
    })?;
    if identity.nlink != 1 {
        return Err(ApplyError::capture(
            ReasonCode::UnsupportedFile,
            CaptureError::Unsupported {
                path: output.path.clone(),
                message: "refusing to replace a multiply linked file"
                    .to_string(),
            },
        ));
    }
    if identity != &output.identity {
        return Err(ApplyError::capture(
            ReasonCode::VaultChanged,
            CaptureError::Unstable(output.path.clone()),
        ));
    }
    if current.bytes() != Some(output.original_bytes.as_slice()) {
        return Err(ApplyError::capture(
            ReasonCode::VaultChanged,
            CaptureError::Unstable(output.path.clone()),
        ));
    }
    Ok(())
}

fn capture_present(
    path: &Path,
    kind: InputKind,
) -> Result<InputSnapshot, CaptureError> {
    let first = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Err(CaptureError::NotFound(path.to_path_buf()));
        }
        Err(error) => return Err(CaptureError::io(path, error)),
    };
    reject_non_regular(path, &first)?;
    let bytes = read_regular_file(path)?;
    let second = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Err(CaptureError::Unstable(path.to_path_buf()));
        }
        Err(error) => return Err(CaptureError::io(path, error)),
    };
    reject_non_regular(path, &second)?;
    if metadata_fingerprint(&first) != metadata_fingerprint(&second) {
        return Err(CaptureError::Unstable(path.to_path_buf()));
    }
    let identity = file_identity(path, &second)?;
    Ok(InputSnapshot {
        path: path.to_path_buf(),
        kind,
        state: InputState::Present { identity, bytes },
    })
}

fn reject_non_regular(
    path: &Path,
    metadata: &fs::Metadata,
) -> Result<(), CaptureError> {
    if metadata.file_type().is_symlink() {
        return Err(CaptureError::Unsupported {
            path: path.to_path_buf(),
            message: "path is a symlink".to_string(),
        });
    }
    if !metadata.file_type().is_file() {
        return Err(CaptureError::Unsupported {
            path: path.to_path_buf(),
            message: "path is not a regular file".to_string(),
        });
    }
    Ok(())
}

fn read_regular_file(path: &Path) -> Result<Vec<u8>, CaptureError> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(O_NOFOLLOW);
    }
    let mut file = options.open(path).map_err(|error| {
        if error.kind() == io::ErrorKind::NotFound {
            CaptureError::Unstable(path.to_path_buf())
        } else {
            CaptureError::io(path, error)
        }
    })?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)
        .map_err(|error| CaptureError::io(path, error))?;
    Ok(bytes)
}

fn file_identity(
    path: &Path,
    metadata: &fs::Metadata,
) -> Result<FileIdentity, CaptureError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let canonical_path = fs::canonicalize(path)
            .map_err(|error| CaptureError::io(path, error))?;
        Ok(FileIdentity {
            canonical_path,
            dev: metadata.dev(),
            ino: metadata.ino(),
            nlink: metadata.nlink(),
            mode: metadata.mode() & 0o7777,
            len: metadata.len(),
            mtime: metadata.modified().ok(),
        })
    }
    #[cfg(not(unix))]
    {
        let _ = metadata;
        Err(CaptureError::Unsupported {
            path: path.to_path_buf(),
            message: "guarded writes require Unix file identity".to_string(),
        })
    }
}

fn metadata_fingerprint(
    metadata: &fs::Metadata,
) -> (u64, u64, u64, u32, u64, Option<SystemTime>) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        (
            metadata.dev(),
            metadata.ino(),
            metadata.nlink(),
            metadata.mode(),
            metadata.len(),
            metadata.modified().ok(),
        )
    }
    #[cfg(not(unix))]
    {
        (0, 0, 0, 0, metadata.len(), metadata.modified().ok())
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn vault_hash(canonical_vault: &Path) -> String {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        sha256_hex(canonical_vault.as_os_str().as_bytes())
    }
    #[cfg(not(unix))]
    {
        sha256_hex(canonical_vault.to_string_lossy().as_bytes())
    }
}

fn format_system_time(time: SystemTime) -> String {
    match time.duration_since(UNIX_EPOCH) {
        Ok(duration) => format!("{}", duration.as_secs()),
        Err(_) => "0".to_string(),
    }
}

fn unix_secs(time: SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

fn create_recovery(
    plan: &WritePlan,
    session: &ApplySession,
) -> Result<PathBuf, ApplyError> {
    let hash = vault_hash(&plan.vault_canonical);
    let recovery_dir = session
        .state_home
        .join("bob-cli")
        .join(STATE_SUBDIR)
        .join(hash)
        .join(&session.run_id);
    ensure_private_dir(&session.state_home.join("bob-cli").join(STATE_SUBDIR))
        .and_then(|_| {
            ensure_private_dir(recovery_dir.parent().unwrap_or(&recovery_dir))
        })
        .and_then(|_| ensure_private_dir(&recovery_dir))
        .map_err(|error| ApplyError {
            reason: ReasonCode::RecoveryFailed,
            message: format!(
                "failed to create recovery directory {}: {error}",
                recovery_dir.display()
            ),
            applied_files: Vec::new(),
            deferred_files: remaining_outputs(plan, &[]),
            recovery_directory: None,
        })?;

    for (index, output) in plan.outputs.iter().enumerate() {
        let original_name = format!("{index:04}.original");
        let proposed_name = format!("{index:04}.proposed");
        write_private_file(
            &recovery_dir.join(&original_name),
            &output.original_bytes,
        )
        .and_then(|_| {
            write_private_file(
                &recovery_dir.join(&proposed_name),
                &output.proposed_bytes,
            )
        })
        .map_err(|error| ApplyError {
            reason: ReasonCode::RecoveryFailed,
            message: format!(
                "failed to write recovery copies in {}: {error}",
                recovery_dir.display()
            ),
            applied_files: Vec::new(),
            deferred_files: remaining_outputs(plan, &[]),
            recovery_directory: None,
        })?;
    }

    write_manifest(&recovery_dir, plan, session, "planned", &[])
        .and_then(|_| sync_dir(&recovery_dir))
        .map_err(|error| ApplyError {
            reason: ReasonCode::RecoveryFailed,
            message: format!(
                "failed to persist recovery manifest {}: {error}",
                recovery_dir.display()
            ),
            applied_files: Vec::new(),
            deferred_files: remaining_outputs(plan, &[]),
            recovery_directory: None,
        })?;
    Ok(recovery_dir)
}

fn recovery_manifest(
    plan: &WritePlan,
    session: &ApplySession,
    outcome: &str,
    applied: &[AppliedWrite],
) -> RecoveryManifest {
    let now = session.now();
    let completed = matches!(outcome, "applied");
    RecoveryManifest {
        tool: TOOL.to_string(),
        schema_version: SCHEMA_VERSION,
        vault: plan.vault_canonical.display().to_string(),
        vault_hash: vault_hash(&plan.vault_canonical),
        run_id: session.run_id.clone(),
        started_at: format_system_time(now),
        started_at_unix: unix_secs(now),
        completed_at: completed.then(|| format_system_time(now)),
        completed_at_unix: completed.then(|| unix_secs(now)),
        outcome: outcome.to_string(),
        notes: plan
            .outputs
            .iter()
            .enumerate()
            .map(|(index, output)| {
                let state =
                    if applied.iter().any(|item| item.path == output.path)
                        || outcome == "applied"
                    {
                        "applied"
                    } else {
                        "planned"
                    };
                RecoveryNote {
                    path: output.path.display().to_string(),
                    original_hash: sha256_hex(&output.original_bytes),
                    proposed_hash: sha256_hex(&output.proposed_bytes),
                    state: state.to_string(),
                    original_file: format!("{index:04}.original"),
                    proposed_file: format!("{index:04}.proposed"),
                }
            })
            .collect(),
    }
}

fn write_manifest(
    recovery_dir: &Path,
    plan: &WritePlan,
    session: &ApplySession,
    outcome: &str,
    applied: &[AppliedWrite],
) -> io::Result<()> {
    let manifest = recovery_manifest(plan, session, outcome, applied);
    let encoded =
        serde_json::to_vec_pretty(&manifest).map_err(io::Error::other)?;
    let temp =
        recovery_dir.join(format!("manifest.{}.json.tmp", session.run_id));
    write_private_file(&temp, &encoded)?;
    fs::rename(&temp, recovery_dir.join("manifest.json"))?;
    Ok(())
}

fn update_manifest(
    recovery_dir: &Path,
    plan: &WritePlan,
    session: &ApplySession,
    outcome: &str,
    applied: &[AppliedWrite],
) -> io::Result<()> {
    write_manifest(recovery_dir, plan, session, outcome, applied)?;
    sync_dir(recovery_dir)
}

fn stage_outputs(
    plan: &WritePlan,
    session: &ApplySession,
    recovery_dir: &Path,
) -> Result<Vec<StagedWrite>, ApplyError> {
    let mut staged = Vec::new();
    for (index, output) in plan.outputs.iter().enumerate() {
        match stage_one(output, &session.run_id, index) {
            Ok(item) => {
                if let Some(fail) = &session.fail_staging
                    && let Err(error) = fail(&output.path)
                {
                    staged.push(item);
                    cleanup_temps(&staged);
                    return Err(ApplyError::io(
                        format!(
                            "failed to stage {}: {error}",
                            output.path.display()
                        ),
                        Vec::new(),
                        remaining_outputs(plan, &[]),
                        Some(recovery_dir.to_path_buf()),
                    ));
                }
                staged.push(item);
            }
            Err(error) => {
                cleanup_temps(&staged);
                return Err(ApplyError::io(
                    format!(
                        "failed to stage {}: {error}",
                        output.path.display()
                    ),
                    Vec::new(),
                    remaining_outputs(plan, &[]),
                    Some(recovery_dir.to_path_buf()),
                ));
            }
        }
    }
    Ok(staged)
}

fn stage_one(
    output: &PlannedWrite,
    run_id: &str,
    index: usize,
) -> io::Result<StagedWrite> {
    let parent = output.path.parent().unwrap_or_else(|| Path::new("."));
    let file_name = output.path.file_name().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("path has no file name: {}", output.path.display()),
        )
    })?;
    let mut nonce = 0_u32;
    let temp = loop {
        let temp_name = staged_temp_name(file_name, run_id, index, nonce);
        let temp = parent.join(temp_name);
        match create_exclusive_temp(
            &temp,
            &output.proposed_bytes,
            output.identity.mode,
        ) {
            Ok(()) => break temp,
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                nonce = nonce.checked_add(1).ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::AlreadyExists,
                        "exhausted exclusive temporary names",
                    )
                })?;
            }
            Err(error) => return Err(error),
        }
    };
    if let Err(error) = copy_copied_metadata(&output.path, &temp) {
        let _ = fs::remove_file(&temp);
        return Err(error);
    }
    Ok(StagedWrite {
        dest: output.path.clone(),
        temp,
        proposed_bytes: output.proposed_bytes.clone(),
    })
}

fn staged_temp_name(
    file_name: &OsStr,
    run_id: &str,
    index: usize,
    nonce: u32,
) -> OsString {
    let mut name = OsString::from(".");
    name.push(file_name);
    name.push(format!(".bob-tsh.{run_id}.{index}.{nonce}.tmp"));
    name
}

fn create_exclusive_temp(
    path: &Path,
    bytes: &[u8],
    mode: u32,
) -> io::Result<()> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    drop(file);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(mode))?;
    }
    let _ = mode;
    Ok(())
}

fn copy_copied_metadata(from: &Path, to: &Path) -> io::Result<()> {
    copy_xattrs(from, to)
}

fn cleanup_temps(staged: &[StagedWrite]) {
    for item in staged {
        if let Err(error) = fs::remove_file(&item.temp)
            && error.kind() != io::ErrorKind::NotFound
        {
            eprintln!(
                "bob task-status-hooks: warning: failed to remove staging file {}: {error}",
                item.temp.display()
            );
        }
    }
}

fn ensure_private_dir(path: &Path) -> io::Result<()> {
    fs::create_dir_all(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

fn write_private_file(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut options = OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}

fn sync_dir(path: &Path) -> io::Result<()> {
    File::open(path)?.sync_all()
}

fn prune_completed(
    root: &Path,
    now: SystemTime,
    retention: Duration,
) -> io::Result<()> {
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    for vault_entry in entries {
        let vault_entry = vault_entry?;
        if !vault_entry.file_type()?.is_dir() {
            continue;
        }
        let run_entries = match fs::read_dir(vault_entry.path()) {
            Ok(entries) => entries,
            Err(_) => continue,
        };
        for run_entry in run_entries {
            let run_entry = run_entry?;
            if !run_entry.file_type()?.is_dir() {
                continue;
            }
            let manifest_path = run_entry.path().join("manifest.json");
            let Ok(text) = fs::read_to_string(&manifest_path) else {
                continue;
            };
            let Ok(manifest) = serde_json::from_str::<RecoveryManifest>(&text)
            else {
                continue;
            };
            if manifest.tool != TOOL
                || manifest.schema_version != SCHEMA_VERSION
            {
                continue;
            }
            if manifest.outcome != "applied" {
                continue;
            }
            let Some(completed) = manifest.completed_at_unix else {
                continue;
            };
            let completed = UNIX_EPOCH + Duration::from_secs(completed);
            let expired = now
                .duration_since(completed)
                .map(|age| age >= retention)
                .unwrap_or(false);
            if expired {
                fs::remove_dir_all(run_entry.path())?;
            }
        }
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn copy_xattrs(from: &Path, to: &Path) -> io::Result<()> {
    use std::os::{
        raw::{c_char, c_int},
        unix::ffi::OsStrExt,
    };

    unsafe extern "C" {
        fn llistxattr(
            path: *const c_char,
            list: *mut c_char,
            size: usize,
        ) -> isize;
        fn lgetxattr(
            path: *const c_char,
            name: *const c_char,
            value: *mut u8,
            size: usize,
        ) -> isize;
        fn lsetxattr(
            path: *const c_char,
            name: *const c_char,
            value: *const u8,
            size: usize,
            flags: c_int,
        ) -> c_int;
    }

    const ENOTSUP: i32 = 95;
    const ENOSYS: i32 = 38;

    fn cstring(path: &Path) -> io::Result<std::ffi::CString> {
        std::ffi::CString::new(path.as_os_str().as_bytes()).map_err(|_| {
            io::Error::new(io::ErrorKind::InvalidInput, "path contains NUL")
        })
    }

    fn copyable(name: &str) -> bool {
        name.starts_with("user.")
            || name.starts_with("trusted.")
            || name == "system.posix_acl_access"
            || name == "system.posix_acl_default"
    }

    let from_c = cstring(from)?;
    let to_c = cstring(to)?;
    let size = unsafe { llistxattr(from_c.as_ptr(), std::ptr::null_mut(), 0) };
    if size < 0 {
        let error = io::Error::last_os_error();
        return match error.raw_os_error() {
            Some(ENOTSUP | ENOSYS) => Ok(()),
            _ => Err(error),
        };
    }
    if size == 0 {
        return Ok(());
    }
    let mut list = vec![0_u8; size as usize];
    let written = unsafe {
        llistxattr(
            from_c.as_ptr(),
            list.as_mut_ptr() as *mut c_char,
            list.len(),
        )
    };
    if written < 0 {
        return Err(io::Error::last_os_error());
    }
    list.truncate(written as usize);
    for name in list
        .split(|byte| *byte == 0)
        .filter(|name| !name.is_empty())
    {
        let name_c = std::ffi::CString::new(name).map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "xattr name contains NUL",
            )
        })?;
        let name_text = name_c.to_string_lossy();
        if !copyable(&name_text) {
            continue;
        }
        let value_size = unsafe {
            lgetxattr(from_c.as_ptr(), name_c.as_ptr(), std::ptr::null_mut(), 0)
        };
        if value_size < 0 {
            return Err(io::Error::last_os_error());
        }
        let mut value = vec![0_u8; value_size as usize];
        let got = unsafe {
            lgetxattr(
                from_c.as_ptr(),
                name_c.as_ptr(),
                value.as_mut_ptr(),
                value.len(),
            )
        };
        if got < 0 {
            return Err(io::Error::last_os_error());
        }
        value.truncate(got as usize);
        let set = unsafe {
            lsetxattr(
                to_c.as_ptr(),
                name_c.as_ptr(),
                value.as_ptr(),
                value.len(),
                0,
            )
        };
        if set < 0 {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(())
}

#[cfg(not(target_os = "linux"))]
fn copy_xattrs(_from: &Path, _to: &Path) -> io::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        cell::{Cell, RefCell},
        rc::Rc,
        sync::atomic::AtomicUsize,
    };

    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    static TEMP_COUNTER: AtomicUsize = AtomicUsize::new(0);

    struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        fn new(prefix: &str) -> Self {
            let nonce = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock")
                .as_nanos();
            let sequence = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "{prefix}-{}-{nonce}-{sequence}",
                std::process::id()
            ));
            fs::create_dir_all(&path).expect("create temp dir");
            Self { path }
        }

        fn path(&self) -> &Path {
            &self.path
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    fn write_file(path: &Path, contents: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("create parent");
        }
        fs::write(path, contents).expect("write file");
    }

    fn fixture() -> (TempDir, PathBuf, PathBuf, PathBuf) {
        let temp = TempDir::new("bob-cli-guarded-write");
        let vault = temp.path().join("vault");
        fs::create_dir_all(&vault).expect("vault");
        let first = vault.join("one.md");
        let second = vault.join("two.md");
        write_file(&first, "alpha\n");
        write_file(&second, "bravo\n");
        (temp, vault, first, second)
    }

    fn snapshot(path: &Path, kind: InputKind) -> InputSnapshot {
        capture_required(path, kind).unwrap_or_else(|error| {
            panic!("capture {}: {}", path.display(), error.message())
        })
    }

    fn plan_for(
        vault: &Path,
        outputs: Vec<(&Path, &str, bool)>,
        extra_inputs: Vec<InputSnapshot>,
        scan_paths: Vec<PathBuf>,
    ) -> WritePlan {
        let mut inputs = extra_inputs;
        let mut planned = Vec::new();
        for (path, proposed, structural) in outputs {
            let original = snapshot(path, InputKind::Note);
            if !inputs.iter().any(|input| input.path == original.path) {
                inputs.push(original.clone());
            }
            planned.push(
                planned_write(
                    path.to_path_buf(),
                    &original,
                    proposed.as_bytes().to_vec(),
                    structural,
                )
                .expect("planned write"),
            );
        }
        WritePlan {
            vault_canonical: vault.canonicalize().expect("canonical vault"),
            inputs,
            scan_paths,
            outputs: planned,
        }
    }

    fn make_session(
        temp: &TempDir,
        scan_paths: Vec<PathBuf>,
        run_id: &str,
    ) -> ApplySession {
        let scan = Rc::new(RefCell::new(scan_paths));
        ApplySession {
            state_home: temp.path().join("state"),
            quiet_period: QUIET_PERIOD,
            retention: RETENTION,
            run_id: run_id.to_string(),
            now_system: Box::new(SystemTime::now),
            sleep: Box::new(|_| {}),
            rescan: Box::new({
                let scan = Rc::clone(&scan);
                move || Ok(scan.borrow().clone())
            }),
            before_preflight: None,
            after_staging: None,
            before_replace: None,
            fail_staging: None,
        }
    }

    fn live_scan_session(
        temp: &TempDir,
        vault: &Path,
        run_id: &str,
    ) -> ApplySession {
        let vault = vault.to_path_buf();
        ApplySession {
            state_home: temp.path().join("state"),
            quiet_period: QUIET_PERIOD,
            retention: RETENTION,
            run_id: run_id.to_string(),
            now_system: Box::new(SystemTime::now),
            sleep: Box::new(|_| {}),
            rescan: Box::new(move || list_markdown(&vault)),
            before_preflight: None,
            after_staging: None,
            before_replace: None,
            fail_staging: None,
        }
    }

    fn list_markdown(vault: &Path) -> io::Result<Vec<PathBuf>> {
        let mut files = Vec::new();
        for entry in fs::read_dir(vault)? {
            let path = entry?.path();
            if path.extension().and_then(OsStr::to_str) == Some("md") {
                files.push(path);
            }
        }
        files.sort();
        Ok(files)
    }

    fn apply_ok(plan: &WritePlan, session: &ApplySession) -> ApplyOutcome {
        apply_plan(plan, session).unwrap_or_else(|error| {
            panic!("{} ({})", error.message, error.reason.as_str())
        })
    }

    fn set_mtime(path: &Path, time: SystemTime) {
        File::options()
            .write(true)
            .open(path)
            .expect("open for mtime")
            .set_modified(time)
            .expect("set mtime");
    }

    #[test]
    fn unchanged_read_set_applies_and_records_recovery_bytes() {
        let (temp, vault, first, second) = fixture();
        let scan = vec![first.clone(), second.clone()];
        let plan = plan_for(
            &vault,
            vec![
                (first.as_path(), "alpha-new\n", false),
                (second.as_path(), "bravo-new\n", false),
            ],
            Vec::new(),
            scan.clone(),
        );
        let session = make_session(&temp, scan, "apply-ok");
        let outcome = apply_ok(&plan, &session);
        let ApplyOutcome::Applied {
            applied_files,
            recovery_directory,
        } = outcome
        else {
            panic!("expected applied outcome");
        };
        assert_eq!(applied_files.len(), 2);
        assert_eq!(fs::read_to_string(&first).unwrap(), "alpha-new\n");
        assert_eq!(fs::read_to_string(&second).unwrap(), "bravo-new\n");
        let original0 =
            fs::read(recovery_directory.join("0000.original")).unwrap();
        let proposed0 =
            fs::read(recovery_directory.join("0001.proposed")).unwrap();
        assert_eq!(original0, b"alpha\n");
        assert_eq!(proposed0, b"bravo-new\n");
        let manifest: RecoveryManifest = serde_json::from_str(
            &fs::read_to_string(recovery_directory.join("manifest.json"))
                .unwrap(),
        )
        .unwrap();
        assert_eq!(manifest.tool, TOOL);
        assert_eq!(manifest.outcome, "applied");
        assert_eq!(manifest.notes[0].original_hash, sha256_hex(b"alpha\n"));
        assert_eq!(manifest.notes[1].proposed_hash, sha256_hex(b"bravo-new\n"));
        assert_eq!(manifest.notes[0].state, "applied");
    }

    #[test]
    fn noop_creates_no_recovery_or_staging() {
        let (temp, vault, first, second) = fixture();
        let scan = vec![first, second];
        let plan = WritePlan {
            vault_canonical: vault.canonicalize().unwrap(),
            inputs: Vec::new(),
            scan_paths: scan.clone(),
            outputs: Vec::new(),
        };
        let session = make_session(&temp, scan, "noop");
        assert!(matches!(apply_ok(&plan, &session), ApplyOutcome::NoOp));
        assert!(!temp.path().join("state/bob-cli/task-status-hooks").exists());
        let foreign = vault.join(".one.md.999.tmp");
        write_file(&foreign, "leave-me");
        assert_eq!(fs::read_to_string(&foreign).unwrap(), "leave-me");
    }

    #[test]
    fn equal_length_change_with_restored_mtime_prevents_write() {
        let (temp, vault, first, second) = fixture();
        let scan = vec![first.clone(), second.clone()];
        let plan = plan_for(
            &vault,
            vec![(first.as_path(), "alpha-new\n", false)],
            vec![snapshot(&second, InputKind::Note)],
            scan.clone(),
        );
        let mtime = fs::metadata(&first).unwrap().modified().unwrap();
        let mut session = make_session(&temp, scan, "mtime-restore");
        session.before_preflight = Some(Box::new({
            let first = first.clone();
            move || {
                fs::write(&first, "ALPHA\n").unwrap();
                set_mtime(&first, mtime);
            }
        }));
        let error = apply_plan(&plan, &session).expect_err("defer");
        assert_eq!(error.reason, ReasonCode::VaultChanged);
        assert_eq!(fs::read_to_string(&first).unwrap(), "ALPHA\n");
        assert!(error.applied_files.is_empty());
    }

    #[test]
    fn replacement_inode_prevents_write() {
        let (temp, vault, first, second) = fixture();
        let scan = vec![first.clone(), second.clone()];
        let plan = plan_for(
            &vault,
            vec![(first.as_path(), "alpha-new\n", false)],
            Vec::new(),
            scan.clone(),
        );
        let mut session = make_session(&temp, scan, "inode");
        session.before_preflight = Some(Box::new({
            let first = first.clone();
            move || {
                fs::remove_file(&first).unwrap();
                fs::write(&first, "alpha\n").unwrap();
            }
        }));
        let error = apply_plan(&plan, &session).expect_err("defer");
        assert_eq!(error.reason, ReasonCode::VaultChanged);
        assert_eq!(fs::read_to_string(&first).unwrap(), "alpha\n");
    }

    #[test]
    fn deletion_prevents_write() {
        let (temp, vault, first, second) = fixture();
        let scan = vec![first.clone(), second.clone()];
        let plan = plan_for(
            &vault,
            vec![(first.as_path(), "alpha-new\n", false)],
            Vec::new(),
            scan.clone(),
        );
        let mut session = make_session(&temp, scan, "delete");
        session.before_preflight = Some(Box::new({
            let first = first.clone();
            move || fs::remove_file(&first).unwrap()
        }));
        let error = apply_plan(&plan, &session).expect_err("defer");
        assert_eq!(error.reason, ReasonCode::VaultChanged);
        assert!(!first.exists());
    }

    #[cfg(unix)]
    #[test]
    fn symlink_substitution_prevents_write() {
        let (temp, vault, first, second) = fixture();
        let scan = vec![first.clone(), second.clone()];
        let plan = plan_for(
            &vault,
            vec![(first.as_path(), "alpha-new\n", false)],
            Vec::new(),
            scan.clone(),
        );
        let mut session = make_session(&temp, scan, "symlink");
        session.before_preflight = Some(Box::new({
            let first = first.clone();
            let second = second.clone();
            move || {
                fs::remove_file(&first).unwrap();
                std::os::unix::fs::symlink(&second, &first).unwrap();
            }
        }));
        let error = apply_plan(&plan, &session).expect_err("defer");
        assert!(matches!(
            error.reason,
            ReasonCode::UnsupportedFile | ReasonCode::VaultChanged
        ));
        assert_eq!(fs::read_to_string(&second).unwrap(), "bravo\n");
    }

    #[test]
    fn changed_tasks_settings_prevent_write() {
        let (temp, vault, first, second) = fixture();
        let settings =
            vault.join(".obsidian/plugins/obsidian-tasks-plugin/data.json");
        write_file(&settings, "{\"globalFilter\":\"#task\"}\n");
        let scan = vec![first.clone(), second.clone()];
        let plan = plan_for(
            &vault,
            vec![(first.as_path(), "alpha-new\n", false)],
            vec![snapshot(&settings, InputKind::TasksSettings)],
            scan.clone(),
        );
        let mut session = make_session(&temp, scan, "settings");
        session.before_preflight = Some(Box::new({
            let settings = settings.clone();
            move || fs::write(&settings, "{\"globalFilter\":\"\"}\n").unwrap()
        }));
        let error = apply_plan(&plan, &session).expect_err("defer");
        assert_eq!(error.reason, ReasonCode::VaultChanged);
        assert_eq!(fs::read_to_string(&first).unwrap(), "alpha\n");
    }

    #[test]
    fn changed_previous_daily_prevents_write() {
        let (temp, vault, first, second) = fixture();
        let previous = vault.join("2026/20260101.md");
        write_file(&previous, "## Pomodoros\n");
        let scan = vec![first.clone(), second.clone(), previous.clone()];
        let plan = plan_for(
            &vault,
            vec![(first.as_path(), "alpha-new\n", false)],
            vec![snapshot(&previous, InputKind::PreviousDaily)],
            scan.clone(),
        );
        let mut session = make_session(&temp, scan, "previous");
        session.before_preflight = Some(Box::new({
            let previous = previous.clone();
            move || fs::write(&previous, "## Pomodoros\n\n- extra\n").unwrap()
        }));
        let error = apply_plan(&plan, &session).expect_err("defer");
        assert_eq!(error.reason, ReasonCode::VaultChanged);
        assert_eq!(fs::read_to_string(&first).unwrap(), "alpha\n");
    }

    #[test]
    fn new_or_deleted_scan_candidate_invalidates_plan() {
        let (temp, vault, first, second) = fixture();
        let scan = vec![first.clone(), second.clone()];
        let plan = plan_for(
            &vault,
            vec![(first.as_path(), "alpha-new\n", false)],
            Vec::new(),
            scan.clone(),
        );
        let live = Rc::new(RefCell::new(scan.clone()));
        let mut session = make_session(&temp, scan.clone(), "scan-new");
        session.rescan = Box::new({
            let live = Rc::clone(&live);
            move || Ok(live.borrow().clone())
        });
        session.before_preflight = Some(Box::new({
            let live = Rc::clone(&live);
            let extra = vault.join("extra.md");
            move || {
                fs::write(&extra, "new\n").unwrap();
                live.borrow_mut().push(extra.clone());
            }
        }));
        let error = apply_plan(&plan, &session).expect_err("new file");
        assert_eq!(error.reason, ReasonCode::VaultChanged);
        assert_eq!(fs::read_to_string(&first).unwrap(), "alpha\n");

        live.borrow_mut().clone_from(&scan);
        let mut session = make_session(&temp, scan.clone(), "scan-deleted");
        session.rescan = Box::new({
            let live = Rc::clone(&live);
            move || Ok(live.borrow().clone())
        });
        session.before_preflight = Some(Box::new({
            let live = Rc::clone(&live);
            let second = second.clone();
            move || {
                live.borrow_mut().retain(|path| path != &second);
            }
        }));
        let error = apply_plan(&plan, &session).expect_err("deleted file");
        assert_eq!(error.reason, ReasonCode::VaultChanged);
    }

    #[test]
    fn edit_between_staging_and_revalidate_survives() {
        let (temp, vault, first, second) = fixture();
        let scan = vec![first.clone(), second.clone()];
        let plan = plan_for(
            &vault,
            vec![(first.as_path(), "alpha-new\n", false)],
            Vec::new(),
            scan.clone(),
        );
        let mut session = make_session(&temp, scan, "after-stage");
        session.after_staging = Some(Box::new({
            let first = first.clone();
            move || fs::write(&first, "user-save\n").unwrap()
        }));
        let error = apply_plan(&plan, &session).expect_err("defer");
        assert_eq!(error.reason, ReasonCode::VaultChanged);
        assert_eq!(fs::read_to_string(&first).unwrap(), "user-save\n");
        assert!(error.applied_files.is_empty());
        assert!(error.recovery_directory.is_some());
    }

    #[test]
    fn edit_before_later_replacement_reports_partial() {
        let (temp, vault, first, second) = fixture();
        let scan = vec![first.clone(), second.clone()];
        let plan = plan_for(
            &vault,
            vec![
                (first.as_path(), "alpha-new\n", false),
                (second.as_path(), "bravo-new\n", false),
            ],
            Vec::new(),
            scan.clone(),
        );
        let mut session = make_session(&temp, scan, "partial");
        let calls = Rc::new(Cell::new(0_u32));
        session.before_replace = Some(Box::new({
            let second = second.clone();
            let calls = Rc::clone(&calls);
            move |path| {
                let count = calls.get();
                calls.set(count + 1);
                if count == 1 && path == second {
                    fs::write(&second, "keep-me\n").unwrap();
                }
            }
        }));
        let error = apply_plan(&plan, &session).expect_err("partial");
        assert_eq!(error.reason, ReasonCode::PartialApply);
        assert_eq!(error.applied_files, vec![first.clone()]);
        assert_eq!(error.deferred_files, vec![second.clone()]);
        assert_eq!(fs::read_to_string(&first).unwrap(), "alpha-new\n");
        assert_eq!(fs::read_to_string(&second).unwrap(), "keep-me\n");
        let recovery = error.recovery_directory.expect("recovery");
        let manifest: RecoveryManifest = serde_json::from_str(
            &fs::read_to_string(recovery.join("manifest.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(manifest.outcome, "partial");
        assert_eq!(
            fs::read(recovery.join("0001.original")).unwrap(),
            b"bravo\n"
        );
        assert_eq!(
            fs::read(recovery.join("0001.proposed")).unwrap(),
            b"bravo-new\n"
        );
    }

    #[cfg(unix)]
    #[test]
    fn exclusive_temp_and_mode_and_foreign_temps() {
        let (temp, vault, first, second) = fixture();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&first, fs::Permissions::from_mode(0o640))
                .unwrap();
        }
        let scan = vec![first.clone(), second.clone()];
        let plan = plan_for(
            &vault,
            vec![(first.as_path(), "alpha-new\n", false)],
            Vec::new(),
            scan.clone(),
        );
        let foreign = vault.join(".one.md.99999.tmp");
        let colliding = vault.join(".one.md.bob-tsh.mode.0.0.tmp");
        write_file(&foreign, "foreign\n");
        write_file(&colliding, "collision\n");
        let session = make_session(&temp, scan, "mode");
        apply_ok(&plan, &session);
        assert_eq!(fs::read_to_string(&first).unwrap(), "alpha-new\n");
        assert_eq!(fs::read_to_string(&foreign).unwrap(), "foreign\n");
        assert_eq!(fs::read_to_string(&colliding).unwrap(), "collision\n");
        let mode = fs::metadata(&first).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o640);
    }

    #[test]
    fn staging_failure_preserves_notes_and_foreign_temps() {
        let (temp, vault, first, second) = fixture();
        let scan = vec![first.clone(), second.clone()];
        let plan = plan_for(
            &vault,
            vec![
                (first.as_path(), "alpha-new\n", false),
                (second.as_path(), "bravo-new\n", false),
            ],
            Vec::new(),
            scan.clone(),
        );
        let foreign = vault.join(".two.md.123.tmp");
        write_file(&foreign, "foreign\n");
        let mut session = make_session(&temp, scan, "stage-fail");
        session.fail_staging = Some(Box::new({
            let second = second.clone();
            move |path| {
                if path == second {
                    Err(io::Error::other("injected staging failure"))
                } else {
                    Ok(())
                }
            }
        }));
        let error = apply_plan(&plan, &session).expect_err("stage fail");
        assert_eq!(error.reason, ReasonCode::Io);
        assert!(error.applied_files.is_empty());
        assert_eq!(fs::read_to_string(&first).unwrap(), "alpha\n");
        assert_eq!(fs::read_to_string(&second).unwrap(), "bravo\n");
        assert_eq!(fs::read_to_string(&foreign).unwrap(), "foreign\n");
        let leftover = fs::read_dir(&vault)
            .unwrap()
            .filter_map(|entry| entry.ok())
            .any(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .contains(".bob-tsh.stage-fail.")
            });
        assert!(!leftover, "run staging files should be removed");
    }

    #[test]
    fn retention_keeps_incomplete_and_prunes_old_completed() {
        let (temp, vault, first, second) = fixture();
        let scan = vec![first.clone(), second.clone()];
        let plan = plan_for(
            &vault,
            vec![(first.as_path(), "alpha-new\n", false)],
            Vec::new(),
            scan.clone(),
        );
        let hash = vault_hash(&vault.canonicalize().unwrap());
        let root = temp
            .path()
            .join("state/bob-cli/task-status-hooks")
            .join(&hash);
        let old_complete = root.join("old-complete");
        let old_partial = root.join("old-partial");
        let other = root.join("other-state.txt");
        fs::create_dir_all(&old_complete).unwrap();
        fs::create_dir_all(&old_partial).unwrap();
        write_file(&other, "leave");
        let now = SystemTime::now();
        let old = unix_secs(now).saturating_sub(31 * 24 * 60 * 60);
        write_file(
            &old_complete.join("manifest.json"),
            &format!(
                r#"{{"tool":"{TOOL}","schema_version":1,"vault":"x","vault_hash":"{hash}","run_id":"old-complete","started_at":"{old}","started_at_unix":{old},"completed_at":"{old}","completed_at_unix":{old},"outcome":"applied","notes":[]}}"#
            ),
        );
        write_file(
            &old_partial.join("manifest.json"),
            &format!(
                r#"{{"tool":"{TOOL}","schema_version":1,"vault":"x","vault_hash":"{hash}","run_id":"old-partial","started_at":"{old}","started_at_unix":{old},"completed_at":null,"completed_at_unix":null,"outcome":"partial","notes":[]}}"#
            ),
        );
        let session = make_session(&temp, scan, "retain");
        apply_ok(&plan, &session);
        assert!(!old_complete.exists());
        assert!(old_partial.exists());
        assert_eq!(fs::read_to_string(&other).unwrap(), "leave");
    }

    #[test]
    fn quiet_period_skips_wait_for_stable_files_and_status_only() {
        let (temp, vault, first, second) = fixture();
        let scan = vec![first.clone(), second.clone()];
        let slept = Rc::new(Cell::new(None));
        let now = SystemTime::now();
        set_mtime(&first, now - Duration::from_secs(3));
        let plan = plan_for(
            &vault,
            vec![(first.as_path(), "alpha-new\n", true)],
            Vec::new(),
            scan.clone(),
        );
        let mut session = make_session(&temp, scan.clone(), "quiet-stable");
        session.now_system = Box::new(move || now);
        session.sleep = Box::new({
            let slept = Rc::clone(&slept);
            move |duration| slept.set(Some(duration))
        });
        apply_ok(&plan, &session);
        assert_eq!(slept.get(), None);

        write_file(&first, "alpha\n");
        let plan = plan_for(
            &vault,
            vec![(first.as_path(), "alpha-new\n", false)],
            Vec::new(),
            scan.clone(),
        );
        set_mtime(&first, now);
        slept.set(None);
        let mut session = make_session(&temp, scan, "quiet-status");
        session.now_system = Box::new(move || now);
        session.sleep = Box::new({
            let slept = Rc::clone(&slept);
            move |duration| slept.set(Some(duration))
        });
        apply_ok(&plan, &session);
        assert_eq!(slept.get(), None);
    }

    #[test]
    fn quiet_period_waits_once_then_defers_if_changed() {
        let (temp, vault, first, second) = fixture();
        let scan = vec![first.clone(), second.clone()];
        let now = SystemTime::now();
        set_mtime(&first, now - Duration::from_millis(500));
        let plan = plan_for(
            &vault,
            vec![(first.as_path(), "alpha-new\n", true)],
            Vec::new(),
            scan.clone(),
        );
        let slept = Rc::new(Cell::new(None));
        let mut session = make_session(&temp, scan, "quiet-wait");
        session.now_system = Box::new(move || now);
        session.sleep = Box::new({
            let slept = Rc::clone(&slept);
            let first = first.clone();
            move |duration| {
                slept.set(Some(duration));
                fs::write(&first, "typed\n").unwrap();
            }
        });
        let error = apply_plan(&plan, &session).expect_err("defer");
        assert_eq!(slept.get(), Some(Duration::from_millis(1500)));
        assert_eq!(error.reason, ReasonCode::VaultChanged);
        assert_eq!(fs::read_to_string(&first).unwrap(), "typed\n");
    }

    #[test]
    fn future_mtime_defers_without_sleeping() {
        let (temp, vault, first, second) = fixture();
        let scan = vec![first.clone(), second.clone()];
        let now = SystemTime::now();
        set_mtime(&first, now + Duration::from_secs(30));
        let plan = plan_for(
            &vault,
            vec![(first.as_path(), "alpha-new\n", true)],
            Vec::new(),
            scan.clone(),
        );
        let slept = Rc::new(Cell::new(false));
        let mut session = make_session(&temp, scan, "future");
        session.now_system = Box::new(move || now);
        session.sleep = Box::new({
            let slept = Rc::clone(&slept);
            move |_| slept.set(true)
        });
        let error = apply_plan(&plan, &session).expect_err("future");
        assert_eq!(error.reason, ReasonCode::QuietPeriod);
        assert!(!slept.get());
        assert_eq!(fs::read_to_string(&first).unwrap(), "alpha\n");
    }

    #[cfg(unix)]
    #[test]
    fn multiply_linked_output_is_rejected() {
        let (temp, vault, first, second) = fixture();
        let linked = vault.join("link.md");
        fs::hard_link(&first, &linked).unwrap();
        let scan = vec![first.clone(), second.clone(), linked.clone()];
        let original = snapshot(&first, InputKind::Note);
        let error = planned_write(
            first.clone(),
            &original,
            b"alpha-new\n".to_vec(),
            false,
        )
        .expect_err("nlink");
        assert!(matches!(error, CaptureError::Unsupported { .. }));
        let _ = (temp, scan);
    }

    #[test]
    fn live_rescan_sees_new_vault_file() {
        let (temp, vault, first, second) = fixture();
        let scan = list_markdown(&vault).unwrap();
        let plan = plan_for(
            &vault,
            vec![(first.as_path(), "alpha-new\n", false)],
            Vec::new(),
            scan,
        );
        let mut session = live_scan_session(&temp, &vault, "live-scan");
        session.before_preflight = Some(Box::new({
            let extra = vault.join("extra.md");
            move || fs::write(&extra, "new\n").unwrap()
        }));
        let error = apply_plan(&plan, &session).expect_err("rescan");
        assert_eq!(error.reason, ReasonCode::VaultChanged);
        assert_eq!(fs::read_to_string(&first).unwrap(), "alpha\n");
        let _ = second;
    }
}
