use std::{
    io,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use crate::native::env as bob_env;

pub(super) const TOOL: &str = "task-status-hooks";

pub(crate) const QUIET_PERIOD: Duration = Duration::from_secs(2);
pub(crate) const RETENTION: Duration = Duration::from_secs(30 * 24 * 60 * 60);

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

    pub(super) fn matches(&self, other: &Self) -> bool {
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
    /// Vault note membership captured at plan time, compared against a fresh
    /// rescan during preflight.
    ///
    /// A caller whose plan does not depend on note membership may pass an
    /// empty `scan_paths` with a `rescan` closure returning an empty list.
    pub scan_paths: Vec<PathBuf>,
    pub outputs: Vec<PlannedWrite>,
}

pub(crate) struct ApplySession {
    /// Tool name owning this session. It selects the
    /// `$XDG_STATE_HOME/bob-cli/<tool>/…` recovery root, the manifest `tool`
    /// value, the `bob <tool>: warning:` prefixes, and the retention scope:
    /// pruning only touches manifests recorded with the same tool.
    pub tool: &'static str,
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
            tool: TOOL,
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

    pub(super) fn now(&self) -> SystemTime {
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
    pub(super) fn io(path: &Path, error: io::Error) -> Self {
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
    pub(super) fn lock_contention() -> Self {
        Self {
            reason: ReasonCode::LockContention,
            message: "another Bob vault maintenance run is already active; rerun later"
                .to_string(),
            applied_files: Vec::new(),
            deferred_files: Vec::new(),
            recovery_directory: None,
        }
    }

    pub(super) fn lock_io(path: &Path, error: io::Error) -> Self {
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

    pub(super) fn capture(reason: ReasonCode, error: CaptureError) -> Self {
        Self {
            reason,
            message: error.message(),
            applied_files: Vec::new(),
            deferred_files: Vec::new(),
            recovery_directory: None,
        }
    }

    pub(super) fn io(
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

    pub(super) fn vault_changed(
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

    pub(super) fn quiet_period(
        plan: &WritePlan,
        detail: impl Into<String>,
    ) -> Self {
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

pub(super) struct StagedWrite {
    pub(super) dest: PathBuf,
    pub(super) temp: PathBuf,
    pub(super) proposed_bytes: Vec<u8>,
}

pub(super) struct AppliedWrite {
    pub(super) path: PathBuf,
    pub(super) proposed_bytes: Vec<u8>,
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

pub(super) fn remaining_outputs(
    plan: &WritePlan,
    applied: &[PathBuf],
) -> Vec<PathBuf> {
    plan.outputs
        .iter()
        .map(|output| output.path.clone())
        .filter(|path| !applied.iter().any(|applied| applied == path))
        .collect()
}
