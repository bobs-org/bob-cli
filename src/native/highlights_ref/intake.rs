//! Locked, one-reference intake used by `bob ref create` and later
//! reference producers.

use super::*;
use crate::native::vault_sync;
use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};

const DEFAULT_MAINTENANCE_WAIT: Duration = Duration::from_secs(60);
const DEFAULT_SCAN_WAIT: Duration = Duration::from_secs(30);
const ENV_INTAKE_LOCK_WAIT_SECONDS: &str = "BOB_REF_INTAKE_LOCK_WAIT_SECONDS";
const ENV_INGEST_ON_CREATE: &str = "BOB_HIGHLIGHTS_INGEST_ON_CREATE";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum IntakeUnit {
    Pdf {
        source: PathBuf,
        destination: PathBuf,
    },
    LibraryPdf {
        pdf: PathBuf,
    },
    #[allow(dead_code)]
    Audio {
        source: PathBuf,
        destination: PathBuf,
        pdf: PathBuf,
    },
}

impl IntakeUnit {
    fn display_path(&self, config: &Config) -> PathBuf {
        let path = match self {
            Self::Pdf { source, .. } | Self::Audio { source, .. } => {
                source.clone()
            }
            Self::LibraryPdf { pdf } => pdf.clone(),
        };
        path.strip_prefix(&config.bob_dir)
            .unwrap_or(&path)
            .to_path_buf()
    }

    fn library_pdf(&self) -> &Path {
        match self {
            Self::Pdf { destination, .. }
            | Self::LibraryPdf { pdf: destination } => destination,
            Self::Audio { pdf, .. } => pdf,
        }
    }

    fn is_library_pdf(&self) -> bool {
        matches!(self, Self::LibraryPdf { .. })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum QueueReason {
    LockBusy,
    ScanBusy,
    OtherVault,
    OutsideVault,
    NotAWorktree,
    PreSyncFailed,
    Collision,
    TooLarge,
    Refused,
    NeedsWritePdfs,
}

impl QueueReason {
    #[allow(dead_code)]
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::LockBusy => "lock_busy",
            Self::ScanBusy => "scan_busy",
            Self::OtherVault => "other_vault",
            Self::OutsideVault => "outside_vault",
            Self::NotAWorktree => "not_a_worktree",
            Self::PreSyncFailed => "pre_sync_failed",
            Self::Collision => "collision",
            Self::TooLarge => "too_large",
            Self::Refused => "refused",
            Self::NeedsWritePdfs => "needs_write_pdfs",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::LockBusy => "lock busy",
            Self::ScanBusy => "scan busy",
            Self::OtherVault => "different vault",
            Self::OutsideVault => "configured path outside vault",
            Self::NotAWorktree => "vault is not a Git worktree",
            Self::PreSyncFailed => "pre-sync failed",
            Self::Collision => "library collision",
            Self::TooLarge => "file too large",
            Self::Refused => "ingest refused",
            Self::NeedsWritePdfs => "requires --write-pdfs",
        }
    }
}

#[derive(Debug)]
struct QueueFailure {
    reason: QueueReason,
    detail: String,
}

impl QueueFailure {
    fn new(reason: QueueReason, detail: impl Into<String>) -> Self {
        Self {
            reason,
            detail: detail.into(),
        }
    }
}

pub(super) struct IntakeRequest<'a> {
    pub(super) producer: &'a str,
    pub(super) keep_reason: Option<&'a str>,
    pub(super) dry_run: bool,
    pub(super) progress: Option<&'a dyn Fn(&str)>,
}

#[derive(Debug)]
pub(super) enum IntakeOutcome {
    Ingested {
        pdf: PathBuf,
        note: PathBuf,
        task_destination: Option<PathBuf>,
        task_block_id: Option<String>,
        adopted: bool,
        commit_sha: Option<String>,
        commit_warning: Option<String>,
        sync_status: String,
    },
    Queued {
        path: PathBuf,
        reason: QueueReason,
        detail: String,
    },
    Kept {
        why: String,
    },
    Incomplete {
        error: String,
    },
    WouldIngest {
        pdf: PathBuf,
        note: PathBuf,
    },
    WouldKeep {
        why: String,
    },
}

struct IntakeSession {
    _maintenance_lock: fs::File,
    _scan_lock: fs::File,
    child_env: ob::ChildEnv,
}

pub(super) fn ingest_on_create() -> Result<(bool, &'static str)> {
    let config = bob_config::load_highlights_config(&bob_config::config_path())
        .map_err(super::config_error)?;
    let configured = config.ingest_on_create().unwrap_or(true);
    let raw = match bob_env::var(ENV_INGEST_ON_CREATE) {
        Ok(raw) => raw,
        Err(std::env::VarError::NotPresent) => {
            return Ok((configured, "highlights.ingest_on_create is false"));
        }
        Err(std::env::VarError::NotUnicode(value)) => {
            return Err(CommandError::new(format!(
                "invalid {ENV_INGEST_ON_CREATE} value {value:?}; accepted values are 0, false, no, off, 1, true, yes, on"
            )));
        }
    };
    let value = raw.trim();
    if value.is_empty() {
        return Ok((configured, "highlights.ingest_on_create is false"));
    }
    match value.to_ascii_lowercase().as_str() {
        "0" | "false" | "no" | "off" => {
            Ok((false, "disabled by BOB_HIGHLIGHTS_INGEST_ON_CREATE"))
        }
        "1" | "true" | "yes" | "on" => Ok((
            true,
            "enabled by BOB_HIGHLIGHTS_INGEST_ON_CREATE",
        )),
        _ => Err(CommandError::new(format!(
            "invalid {ENV_INGEST_ON_CREATE} value {raw:?}; accepted values are 0, false, no, off, 1, true, yes, on"
        ))),
    }
}

pub(super) fn finish_intake(
    config: &Config,
    unit: IntakeUnit,
    request: IntakeRequest<'_>,
) -> IntakeOutcome {
    if let Some(why) = request.keep_reason {
        return if request.dry_run {
            IntakeOutcome::WouldKeep {
                why: why.to_string(),
            }
        } else {
            IntakeOutcome::Kept {
                why: why.to_string(),
            }
        };
    }
    if request.dry_run {
        let pdf = unit.library_pdf().to_path_buf();
        let note = match ref_note_path(config, &pdf) {
            Ok(note) => note,
            Err(error) => {
                return IntakeOutcome::Queued {
                    path: unit.display_path(config),
                    reason: QueueReason::Refused,
                    detail: error.to_string(),
                };
            }
        };
        return IntakeOutcome::WouldIngest { pdf, note };
    }

    let path = unit.display_path(config);
    let mut progress = |line: &str| {
        if let Some(callback) = request.progress {
            callback(line);
        }
    };
    let session = match IntakeSession::open(config, &mut progress) {
        Ok(session) => session,
        Err(failure) => {
            return IntakeOutcome::Queued {
                path,
                reason: failure.reason,
                detail: failure.detail,
            };
        }
    };

    if !unit.is_library_pdf() {
        if let Err(failure) = session.pre_sync() {
            return IntakeOutcome::Queued {
                path,
                reason: failure.reason,
                detail: failure.detail,
            };
        }
    }

    match session.ingest(config, &unit, request.producer) {
        Ok(mut ingested) => {
            ingested.sync_status = session.post_sync();
            IntakeOutcome::Ingested {
                pdf: ingested.pdf,
                note: ingested.note,
                task_destination: ingested.task_destination,
                task_block_id: ingested.task_block_id,
                adopted: ingested.adopted,
                commit_sha: ingested.commit_sha,
                commit_warning: ingested.commit_warning,
                sync_status: ingested.sync_status,
            }
        }
        Err(IngestFailure::Queued(failure)) => IntakeOutcome::Queued {
            path,
            reason: failure.reason,
            detail: failure.detail,
        },
        Err(IngestFailure::Incomplete(error)) => {
            IntakeOutcome::Incomplete { error }
        }
    }
}

impl IntakeSession {
    fn open(
        config: &Config,
        progress: &mut dyn FnMut(&str),
    ) -> std::result::Result<Self, QueueFailure> {
        validate_vault_identity(config)?;
        let (maintenance_wait, scan_wait) = intake_lock_waits();
        let maintenance = ob::acquire_lock_waiting(maintenance_wait, || {
            progress("waiting for another vault maintenance run…")
        })
        .map_err(|error| {
            QueueFailure::new(QueueReason::LockBusy, error.to_string())
        })?;
        let child_env = ob::child_env();
        match ob::detect_git_worktree(&config.bob_dir, &child_env) {
            Ok(true) => {}
            Ok(false) | Err(_) => {
                return Err(QueueFailure::new(
                    QueueReason::NotAWorktree,
                    format!(
                        "{} is not a Git worktree",
                        config.bob_dir.display()
                    ),
                ));
            }
        }
        let scan = super::acquire_scan_writer_lock_with_timeout_and_wait(
            scan_wait,
            || progress("waiting for another bob ref scan to finish…"),
        )
        .map_err(|error| {
            QueueFailure::new(QueueReason::ScanBusy, error.to_string())
        })?;
        Ok(Self {
            _maintenance_lock: maintenance,
            _scan_lock: scan,
            child_env,
        })
    }

    fn pre_sync(&self) -> std::result::Result<(), QueueFailure> {
        let report = vault_sync::run_cycle_with_existing_lock_report(
            &self.child_env,
            true,
        );
        if report.ok && report.conflicts.is_empty() {
            return Ok(());
        }
        let detail = if report.conflicts.is_empty() {
            report
                .error
                .unwrap_or_else(|| "vault-sync reported failure".to_string())
        } else {
            format!("vault sync conflict in {}", report.conflicts.join(", "))
        };
        Err(QueueFailure::new(QueueReason::PreSyncFailed, detail))
    }

    fn post_sync(&self) -> String {
        let report = vault_sync::run_cycle_with_existing_lock_report(
            &self.child_env,
            true,
        );
        if !report.conflicts.is_empty() {
            return format!(
                "pushed with conflicts in {} (see _conflicts/sync_conflicts.md)",
                report.conflicts.join(", ")
            );
        }
        if !report.ok {
            return format!(
                "push pending ({}); background vault-sync will publish it",
                report.error.unwrap_or_else(|| "push failed".to_string())
            );
        }
        "pushed".to_string()
    }

    fn ingest(
        &self,
        config: &Config,
        unit: &IntakeUnit,
        producer: &str,
    ) -> std::result::Result<Ingested, IngestFailure> {
        let mut moves =
            unit_moves(config, unit).map_err(IngestFailure::Queued)?;
        validate_unit_collisions(config, unit, &moves)
            .map_err(IngestFailure::Queued)?;
        validate_move_sizes(&moves).map_err(IngestFailure::Queued)?;
        perform_moves(&mut moves).map_err(IngestFailure::Queued)?;

        let pdf = unit.library_pdf().to_path_buf();
        let plan = match plan_single_pdf_sync(
            config,
            &pdf,
            SyncOptions {
                dry_run: false,
                write_pdf: false,
                prefer: None,
            },
        ) {
            Ok(plan) => plan,
            Err(error) => {
                let reason = if error.message.contains("requires --write-pdf") {
                    QueueReason::NeedsWritePdfs
                } else {
                    QueueReason::Refused
                };
                return rollback_or_queue(
                    &mut moves,
                    QueueFailure::new(reason, error.to_string()),
                );
            }
        };
        let write = match write_single_pdf_sync(config, &plan) {
            Ok(report) => report,
            Err(error) => {
                if git_changes_only_moved_files(config, &self.child_env, &moves)
                {
                    return rollback_or_queue(
                        &mut moves,
                        QueueFailure::new(
                            QueueReason::Refused,
                            error.to_string(),
                        ),
                    );
                }
                return Err(IngestFailure::Incomplete(format!(
                    "ingest stopped after moving the PDF into lib/: {error}"
                )));
            }
        };

        let note = plan.note_path.clone();
        let mut path_set = scoped_write_path_set(config, &plan).write_paths;
        for moved in &moves {
            path_set.insert(moved.to.clone());
        }
        let (paths, ignored) =
            filter_ignored_paths(&config.bob_dir, &self.child_env, path_set);
        for path in ignored {
            eprintln!(
                "warning: skipped ignored intake write path {}",
                path.display()
            );
        }
        let subject = intake_commit_subject(producer, unit, config);
        let (commit_sha, commit_warning) = match ob::commit_paths(
            &config.bob_dir,
            &self.child_env,
            &subject,
            &paths,
        ) {
            Ok(Some(sha)) => (Some(sha), None),
            Ok(None) => (None, None),
            Err(error) => (None, Some(error)),
        };
        let task = task_destination_and_id(config, &plan, &write);
        let adopted =
            write.reading_created.is_none() && plan.reading_located.is_some();
        Ok(Ingested {
            pdf,
            note,
            task_destination: task.as_ref().map(|(path, _)| path.clone()),
            task_block_id: task.map(|(_, id)| id),
            adopted,
            commit_sha,
            commit_warning,
            sync_status: String::new(),
        })
    }
}

struct Ingested {
    pdf: PathBuf,
    note: PathBuf,
    task_destination: Option<PathBuf>,
    task_block_id: Option<String>,
    adopted: bool,
    commit_sha: Option<String>,
    commit_warning: Option<String>,
    sync_status: String,
}

enum IngestFailure {
    Queued(QueueFailure),
    Incomplete(String),
}

struct MovedFile {
    from: PathBuf,
    to: PathBuf,
}

fn validate_vault_identity(
    config: &Config,
) -> std::result::Result<(), QueueFailure> {
    let actual = fs::canonicalize(&config.bob_dir).map_err(|error| {
        QueueFailure::new(
            QueueReason::OutsideVault,
            format!("resolve vault {}: {error}", config.bob_dir.display()),
        )
    })?;
    let selected = fs::canonicalize(bob_env::bob_dir()).map_err(|error| {
        QueueFailure::new(
            QueueReason::OtherVault,
            format!(
                "resolve BOB_DIR {}: {error}",
                bob_env::bob_dir().display()
            ),
        )
    })?;
    if actual != selected {
        return Err(QueueFailure::new(
            QueueReason::OtherVault,
            format!(
                "selected vault {} differs from BOB_DIR {}",
                actual.display(),
                selected.display()
            ),
        ));
    }
    for (name, path) in [
        ("lib_dir", &config.lib_dir),
        ("ref_dir", &config.ref_dir),
        ("xlib_dir", &config.xlib_dir),
    ] {
        let resolved = canonicalize_with_missing(path).map_err(|error| {
            QueueFailure::new(
                QueueReason::OutsideVault,
                format!("resolve {name} {}: {error}", path.display()),
            )
        })?;
        if !resolved.starts_with(&actual) {
            return Err(QueueFailure::new(
                QueueReason::OutsideVault,
                format!(
                    "{name} resolves outside {}: {}",
                    actual.display(),
                    resolved.display()
                ),
            ));
        }
    }
    Ok(())
}

fn canonicalize_with_missing(path: &Path) -> std::io::Result<PathBuf> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };
    let mut ancestor = absolute.as_path();
    let mut suffix = Vec::new();
    while !ancestor.exists() {
        let name = ancestor.file_name().ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("no existing ancestor for {}", path.display()),
            )
        })?;
        suffix.push(name.to_os_string());
        ancestor = ancestor.parent().ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("no existing ancestor for {}", path.display()),
            )
        })?;
    }
    let mut canonical = fs::canonicalize(ancestor)?;
    for component in suffix.iter().rev() {
        canonical.push(component);
    }
    Ok(super::stamp::normalize_lexically(&canonical))
}

fn intake_lock_waits() -> (Duration, Duration) {
    match bob_env::var(ENV_INTAKE_LOCK_WAIT_SECONDS)
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .map(Duration::from_secs)
    {
        Some(wait) => (wait, wait),
        None => (DEFAULT_MAINTENANCE_WAIT, DEFAULT_SCAN_WAIT),
    }
}

fn unit_moves(
    config: &Config,
    unit: &IntakeUnit,
) -> std::result::Result<Vec<MovedFile>, QueueFailure> {
    let planned = match unit {
        IntakeUnit::Pdf { source, .. } => {
            let plan = plan_single_xlib_pdf_intake(config, source).map_err(
                |error| {
                    QueueFailure::new(QueueReason::Refused, error.to_string())
                },
            )?;
            std::iter::once((plan.source, plan.destination))
                .chain(plan.companions)
                .collect::<Vec<_>>()
        }
        IntakeUnit::LibraryPdf { .. } => Vec::new(),
        IntakeUnit::Audio {
            source,
            destination,
            ..
        } => {
            vec![(source.clone(), destination.clone())]
        }
    };
    Ok(planned
        .into_iter()
        .map(|(from, to)| MovedFile { from, to })
        .collect())
}

fn validate_unit_collisions(
    config: &Config,
    unit: &IntakeUnit,
    moves: &[MovedFile],
) -> std::result::Result<(), QueueFailure> {
    let collision = |path: &Path| {
        QueueFailure::new(
            QueueReason::Collision,
            format!("destination already exists: {}", path.display()),
        )
    };
    for moved in moves {
        if moved.to.exists() {
            return Err(collision(&moved.to));
        }
    }
    let pdf = unit.library_pdf();
    if matches!(unit, IntakeUnit::Pdf { .. }) {
        for sidecar in library_destination_sidecars(pdf) {
            if sidecar.exists() {
                return Err(collision(&sidecar));
            }
        }
        for extension in AUDIO_COMPANION_EXTENSIONS {
            let audio = pdf.with_extension(extension);
            if audio.exists() && !moves.iter().any(|moved| moved.to == audio) {
                return Err(collision(&audio));
            }
        }
    }
    if let IntakeUnit::Audio { .. } = unit {
        if !pdf.with_extension("pdf").is_file() {
            return Err(QueueFailure::new(
                QueueReason::Collision,
                format!(
                    "matching library PDF is missing: {}",
                    pdf.with_extension("pdf").display()
                ),
            ));
        }
    }
    let note = ref_note_path(config, pdf).map_err(|error| {
        QueueFailure::new(QueueReason::Refused, error.to_string())
    })?;
    if note.exists() && matches!(unit, IntakeUnit::Pdf { .. }) {
        return Err(collision(&note));
    }
    Ok(())
}

fn validate_move_sizes(
    moves: &[MovedFile],
) -> std::result::Result<(), QueueFailure> {
    for moved in moves {
        let metadata = fs::metadata(&moved.from).map_err(|error| {
            QueueFailure::new(
                QueueReason::Refused,
                format!("stat {}: {error}", moved.from.display()),
            )
        })?;
        if metadata.len() >= vault_sync::LARGE_FILE_LIMIT_BYTES {
            return Err(QueueFailure::new(
                QueueReason::TooLarge,
                format!(
                    "{} is {} bytes; vault-sync's limit is 95 MiB",
                    moved.from.display(),
                    metadata.len()
                ),
            ));
        }
    }
    Ok(())
}

fn perform_moves(
    moves: &mut Vec<MovedFile>,
) -> std::result::Result<(), QueueFailure> {
    let mut completed = Vec::new();
    for moved in moves.iter() {
        if let Some(parent) = moved.to.parent() {
            if let Err(error) = fs::create_dir_all(parent) {
                let detail = format!("create {}: {error}", parent.display());
                return Err(rollback_moves(&mut completed, detail));
            }
        }
        if let Err(error) = super::io::rename_no_replace(&moved.from, &moved.to)
        {
            let detail = format!(
                "move {} to {}: {error}",
                moved.from.display(),
                moved.to.display()
            );
            return Err(rollback_moves(&mut completed, detail));
        }
        completed.push(MovedFile {
            from: moved.from.clone(),
            to: moved.to.clone(),
        });
    }
    *moves = completed;
    Ok(())
}

fn rollback_moves(moves: &mut Vec<MovedFile>, detail: String) -> QueueFailure {
    let mut rollback_errors = Vec::new();
    for moved in moves.iter().rev() {
        if let Some(parent) = moved.from.parent() {
            let _ = fs::create_dir_all(parent);
        }
        if let Err(error) = super::io::rename_no_replace(&moved.to, &moved.from)
        {
            rollback_errors.push(format!(
                "{} -> {}: {error}",
                moved.to.display(),
                moved.from.display()
            ));
        }
    }
    moves.clear();
    let detail = if rollback_errors.is_empty() {
        detail
    } else {
        format!("{detail}; rollback failed: {}", rollback_errors.join(", "))
    };
    QueueFailure::new(QueueReason::Refused, detail)
}

fn rollback_or_queue<T>(
    moves: &mut Vec<MovedFile>,
    failure: QueueFailure,
) -> std::result::Result<T, IngestFailure> {
    let reason = failure.reason;
    let detail = failure.detail;
    let rollback = rollback_moves(moves, detail);
    Err(IngestFailure::Queued(QueueFailure::new(
        reason,
        rollback.detail,
    )))
}

fn git_changes_only_moved_files(
    config: &Config,
    child_env: &ob::ChildEnv,
    moves: &[MovedFile],
) -> bool {
    let Ok(output) = ob::git_command(&config.bob_dir, child_env)
        .args(["status", "--porcelain=v1", "--untracked-files=all", "-z"])
        .output()
    else {
        return false;
    };
    if !output.status.success() {
        return false;
    }
    let allowed = moves
        .iter()
        .filter_map(|moved| moved.to.strip_prefix(&config.bob_dir).ok())
        .map(Path::to_path_buf)
        .collect::<BTreeSet<_>>();
    output
        .stdout
        .split(|byte| *byte == 0)
        .filter(|record| !record.is_empty())
        .all(|record| {
            record
                .get(3..)
                .and_then(|path| std::str::from_utf8(path).ok())
                .map(Path::new)
                .is_some_and(|path| allowed.contains(path))
        })
}

fn filter_ignored_paths(
    vault: &Path,
    child_env: &ob::ChildEnv,
    paths: BTreeSet<PathBuf>,
) -> (Vec<PathBuf>, Vec<PathBuf>) {
    let mut included = Vec::new();
    let mut ignored = Vec::new();
    for path in paths {
        let relative = path.strip_prefix(vault).unwrap_or(&path);
        let result = ob::git_command(vault, child_env)
            .arg("check-ignore")
            .arg("-q")
            .arg("--")
            .arg(relative)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        if result.is_ok_and(|status| status.success()) {
            ignored.push(path);
        } else {
            included.push(relative.to_path_buf());
        }
    }
    (included, ignored)
}

fn task_destination_and_id(
    config: &Config,
    plan: &PdfSyncPlan,
    write: &SyncWriteReport,
) -> Option<(PathBuf, String)> {
    let outcome = write
        .reading_created
        .as_ref()
        .or(write.reading_updated.as_ref());
    if let Some(outcome) = outcome {
        return Some((config.bob_dir.join(&outcome.dest), outcome.id.clone()));
    }
    plan.reading_located.as_ref().and_then(|located| {
        located
            .block_id
            .clone()
            .map(|id| (config.bob_dir.join(&located.path), id))
    })
}

fn intake_commit_subject(
    producer: &str,
    unit: &IntakeUnit,
    config: &Config,
) -> String {
    let pdf = unit.library_pdf();
    let relative = pdf.strip_prefix(&config.lib_dir).unwrap_or(pdf);
    let reference = relative
        .components()
        .find_map(|component| match component {
            std::path::Component::Normal(name) => name.to_str(),
            _ => None,
        })
        .unwrap_or("reference");
    let stem = pdf
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("reference");
    match unit {
        IntakeUnit::Audio { .. } => {
            format!("{producer}: attach audio {reference}/{stem}")
        }
        _ => format!("{producer}: ingest {reference}/{stem}"),
    }
}

pub(super) fn print_preview(
    config: &Config,
    unit: &IntakeUnit,
    keep_reason: Option<&str>,
) {
    if let Some(why) = keep_reason {
        println!("intake: keep ({why})");
        return;
    }
    let pdf = unit.library_pdf();
    match ref_note_path(config, pdf) {
        Ok(note) => println!(
            "intake: would ingest into {} → {} (pull, file note and reading task, commit, push)",
            report_path(config, pdf),
            report_path(config, &note)
        ),
        Err(error) => println!("intake: cannot plan ({error})"),
    }
}

pub(super) fn print_outcome(
    config: &Config,
    plan: &TargetPlan,
    outcome: IntakeOutcome,
) -> Result<()> {
    match outcome {
        IntakeOutcome::Ingested {
            pdf,
            note,
            task_destination,
            task_block_id,
            adopted,
            commit_sha,
            commit_warning,
            sync_status,
        } => {
            let mut line = format!(
                "ingested: {} → {}",
                report_path(config, &pdf),
                report_path(config, &note)
            );
            if let (Some(destination), Some(block_id)) =
                (task_destination, task_block_id)
            {
                line.push_str(&format!(
                    " · task → {} ^{block_id}",
                    report_path(config, &destination)
                ));
            }
            if adopted {
                line.push_str(" (adopted)");
            }
            println!("{line}");
            if let Some(warning) = commit_warning {
                println!(
                    "sync: committed with warning ({warning}); background vault-sync will commit it"
                );
            } else if let Some(sha) = commit_sha {
                println!(
                    "sync: committed {} · {sync_status}",
                    &sha[..sha.len().min(7)]
                );
            } else {
                println!("sync: already committed · {sync_status}");
            }
            Ok(())
        }
        IntakeOutcome::Queued {
            path,
            reason,
            detail,
        } => {
            println!(
                "queued: {} ({}: {}) · next: bob ref scan",
                report_path(config, &path),
                reason.label(),
                detail
            );
            Ok(())
        }
        IntakeOutcome::Kept { why } | IntakeOutcome::WouldKeep { why } => {
            let _ = why;
            print_next_step(plan);
            Ok(())
        }
        IntakeOutcome::WouldIngest { pdf, note } => {
            println!(
                "intake: would ingest into {} → {} (pull, file note and reading task, commit, push)",
                report_path(config, &pdf),
                report_path(config, &note)
            );
            Ok(())
        }
        IntakeOutcome::Incomplete { error } => {
            print_next_step(plan);
            Err(CommandError::new(error))
        }
    }
}

fn report_path(config: &Config, path: &Path) -> String {
    path.strip_prefix(&config.bob_dir)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(root: &Path) -> Config {
        Config {
            bob_dir: root.to_path_buf(),
            lib_dir: root.join("lib"),
            ref_dir: root.join("ref"),
            xlib_dir: root.join("xlib"),
        }
    }

    fn pdf_case() -> (tempfile::TempDir, Config, IntakeUnit, Vec<MovedFile>) {
        let temp = tempfile::tempdir().expect("create temp vault");
        let config = config(temp.path());
        let source = config.xlib_dir.join("chat/queued.pdf");
        fs::create_dir_all(source.parent().expect("source parent"))
            .expect("create xlib type");
        fs::write(&source, b"%PDF-1.4\n").expect("write xlib PDF");
        let unit = IntakeUnit::Pdf {
            source: source.clone(),
            destination: config.lib_dir.join("chat/queued.pdf"),
        };
        let moves = unit_moves(&config, &unit).expect("plan source unit");
        (temp, config, unit, moves)
    }

    #[test]
    fn single_unit_recheck_detects_pdf_sidecar_audio_and_note_collisions() {
        for (relative, expected) in [
            ("lib/chat/queued.pdf", "collision"),
            ("lib/chat/queued.md", "collision"),
            ("lib/chat/queued.opus", "collision"),
            ("ref/chat/queued.md", "collision"),
        ] {
            let (_temp, config, unit, moves) = pdf_case();
            let conflict = config.bob_dir.join(relative);
            fs::create_dir_all(conflict.parent().expect("collision parent"))
                .expect("create collision parent");
            fs::write(&conflict, b"occupied").expect("write conflict");
            let failure = validate_unit_collisions(&config, &unit, &moves)
                .expect_err("occupied destination must queue");
            assert_eq!(failure.reason.as_str(), expected, "{relative}");
        }
    }

    #[test]
    fn single_unit_size_preflight_uses_vault_sync_limit() {
        let temp = tempfile::tempdir().expect("create temp directory");
        let large = temp.path().join("large.pdf");
        fs::File::create(&large)
            .and_then(|file| file.set_len(vault_sync::LARGE_FILE_LIMIT_BYTES))
            .expect("create sparse file");
        let failure = validate_move_sizes(&[MovedFile {
            from: large,
            to: temp.path().join("lib/large.pdf"),
        }])
        .expect_err("file at the vault-sync limit must queue");
        assert_eq!(failure.reason, QueueReason::TooLarge);
    }
}
