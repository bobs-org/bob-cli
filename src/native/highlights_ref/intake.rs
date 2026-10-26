//! Locked, one-reference intake used by `bob ref create` and later
//! reference producers.

use super::intake_status;
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
    Audio {
        source: PathBuf,
        destination: PathBuf,
        pdf: PathBuf,
    },
}

impl IntakeUnit {
    pub(super) fn display_path(&self, config: &Config) -> PathBuf {
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

    pub(super) fn library_pdf(&self) -> &Path {
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

    pub(super) fn label(self) -> &'static str {
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
    // Field order is the release order: scan.lock, then bob_sync.lock.
    _scan_lock: fs::File,
    _maintenance_lock: fs::File,
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
    let producer = request.producer;
    let progress = request.progress;
    let outcome = file_unit(config, unit, request);
    let warning = match &outcome {
        IntakeOutcome::Ingested { .. } => {
            intake_status::record_success(producer, 1)
        }
        IntakeOutcome::Queued { reason, detail, .. } => {
            intake_status::record_failure(producer, reason.as_str(), detail)
        }
        IntakeOutcome::Incomplete { error } => {
            intake_status::record_failure(producer, "incomplete", error)
        }
        IntakeOutcome::Kept { .. }
        | IntakeOutcome::WouldKeep { .. }
        | IntakeOutcome::WouldIngest { .. } => None,
    };
    if let (Some(warning), Some(callback)) = (warning, progress) {
        callback(&warning);
    }
    outcome
}

fn file_unit(
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
            for path in &ingested.ignored_paths {
                progress(&ignored_path_warning(path));
            }
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
            _scan_lock: scan,
            _maintenance_lock: maintenance,
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
        // Locked reread recheck before any move/task/note mutation: when
        // another host filed the whole-bundle name after queue time,
        // refuse filing and preserve the queued source for recovery.
        if !matches!(unit, IntakeUnit::Audio { .. }) {
            let occupants = recheck_reread_occupancy(
                config,
                unit.library_pdf(),
                &reread_own_files(unit),
            )
            .map_err(|error| {
                IngestFailure::Queued(QueueFailure::new(
                    QueueReason::Refused,
                    error.to_string(),
                ))
            })?;
            if !occupants.is_empty() {
                return Err(IngestFailure::Queued(reread_collision(
                    unit.library_pdf(),
                    occupants,
                )));
            }
        }
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
                return rollback_or_queue(
                    &mut moves,
                    QueueFailure::new(
                        queue_reason_for_plan_error(&error),
                        error.to_string(),
                    ),
                );
            }
        };
        if let Err(error) = ensure_safe_to_write(config, iter::once(&plan)) {
            return rollback_or_queue(
                &mut moves,
                QueueFailure::new(QueueReason::Refused, error.to_string()),
            );
        }
        // Capture before `execute_pdf_sync`: routed intents drop out of
        // `scoped_write_path_set` once their destination already contains
        // the candidate identity.
        let mut path_set = scoped_write_path_set(config, &plan).write_paths;
        let mut session = MarkerWriteSession::for_explicit_write()
            .map_err(|error| IngestFailure::Incomplete(error.to_string()))?;
        let write = match execute_pdf_sync(
            config,
            &plan,
            &mut RunWriteState::default(),
            &mut session,
        ) {
            Ok(PdfSyncOutcome::Written(report)) => report,
            Ok(PdfSyncOutcome::Held(hold)) => {
                return rollback_or_queue(
                    &mut moves,
                    QueueFailure::new(QueueReason::Refused, hold.message),
                );
            }
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
        for moved in &moves {
            path_set.insert(moved.to.clone());
        }
        let (paths, ignored) =
            filter_ignored_paths(&config.bob_dir, &self.child_env, path_set);
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
            title: note_title(&plan.pdf, &plan.synced_projection),
            ref_type: plan.stable_metadata.ref_type.clone(),
            source_pdf: plan.stable_metadata.source_pdf.clone(),
            moved: moves
                .iter()
                .map(|moved| (moved.from.clone(), moved.to.clone()))
                .collect(),
            write,
            ignored_paths: ignored,
        })
    }
}

pub(super) struct Ingested {
    pdf: PathBuf,
    pub(super) note: PathBuf,
    task_destination: Option<PathBuf>,
    task_block_id: Option<String>,
    adopted: bool,
    commit_sha: Option<String>,
    commit_warning: Option<String>,
    sync_status: String,
    pub(super) write: SyncWriteReport,
    pub(super) title: String,
    pub(super) ref_type: Option<String>,
    pub(super) source_pdf: String,
    pub(super) moved: Vec<(PathBuf, PathBuf)>,
    ignored_paths: Vec<PathBuf>,
}

enum IngestFailure {
    Queued(QueueFailure),
    Incomplete(String),
}

fn queue_reason_for_plan_error(error: &CommandError) -> QueueReason {
    if error.code == Some("needs_write_pdfs") {
        QueueReason::NeedsWritePdfs
    } else {
        QueueReason::Refused
    }
}

fn ignored_path_warning(path: &Path) -> String {
    format!(
        "warning: skipped ignored intake write path {}",
        path.display()
    )
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

/// Recheck complete reread occupancy under the intake locks, after
/// lock-taking and pre-sync and before any move/task/note mutation. Only
/// `<base>__vN` stems probe; ordinary births keep the existing collision
/// checks. The whole-bundle set (notes, PDFs, sidecars, `.textbundle`,
/// companion audio, `.assets/`, and case-insensitive reference ids across
/// every type under `ref/`, `lib/`, and `xlib/`) is rechecked minus the
/// queued unit's own files. A taken name refuses filing with a
/// collision/nothing-filed report and preserves the queued source for
/// recovery; it never re-uses the occupant or leaves a marker id
/// different from its stem.
pub(super) fn recheck_reread_occupancy(
    config: &Config,
    pdf: &Path,
    own: &[PathBuf],
) -> Result<Vec<PathBuf>> {
    let Some(stem) = pdf.file_stem().and_then(|stem| stem.to_str()) else {
        return Ok(Vec::new());
    };
    if split_reread_stem(stem).is_none() {
        return Ok(Vec::new());
    }
    let ref_type = pdf_path_metadata(config, pdf)?.ref_type;
    let Some(ref_type) = ref_type else {
        return Ok(Vec::new());
    };
    let occupancy = DefaultTargetOccupancy::build(config, &ref_type)?;
    let mut occupants = occupancy.occupants_except(stem, own);
    occupants.sort();
    Ok(occupants)
}

fn reread_collision(pdf: &Path, mut occupants: Vec<PathBuf>) -> QueueFailure {
    let stem = pdf
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("<unknown>");
    occupants.sort();
    QueueFailure::new(
        QueueReason::Collision,
        format!(
            "reread {stem} is already filed ({}); nothing filed, queued source preserved",
            occupants
                .iter()
                .map(|path| path.display().to_string())
                .collect::<Vec<_>>()
                .join(", "),
        ),
    )
}

/// The queued unit's own files, ignored by the locked reread recheck:
/// queued sources plus their same-stem companions, and already-placed
/// library destinations (which do not exist yet for queued PDFs, but are
/// harmless to list).
pub(super) fn reread_own_files(unit: &IntakeUnit) -> Vec<PathBuf> {
    if let IntakeUnit::Audio {
        source,
        destination,
        ..
    } = unit
    {
        return vec![source.clone(), destination.clone()];
    }
    let (primary, secondary) = match unit {
        IntakeUnit::Pdf {
            source,
            destination,
        } => (source, Some(destination)),
        IntakeUnit::LibraryPdf { pdf } => (pdf, None),
        IntakeUnit::Audio { .. } => unreachable!("handled above"),
    };
    let mut own = vec![primary.clone()];
    let mut bases = vec![primary.clone()];
    if let Some(other) = secondary {
        own.push(other.clone());
        bases.push(other.clone());
    }
    for base in &bases {
        for extension in ["md", "textbundle"] {
            own.push(base.with_extension(extension));
        }
        for extension in AUDIO_COMPANION_EXTENSIONS {
            own.push(base.with_extension(extension));
        }
    }
    own
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

/// Producer name in the commit subject of every queue drain.
const DRAIN_PRODUCER: &str = "bob ref scan";

/// One queued intake item and what the drain did with it.
pub(super) enum DrainItem {
    Filed {
        unit: IntakeUnit,
        ingested: Box<Ingested>,
    },
    Queued {
        unit: IntakeUnit,
        reason: QueueReason,
        detail: String,
    },
    Incomplete {
        unit: IntakeUnit,
        error: String,
    },
}

/// Everything one drain did, in queue order.
pub(super) struct DrainReport {
    pub(super) items: Vec<DrainItem>,
    /// The single closing sync: `pushed`, `push pending (…)`, or conflicts.
    /// `None` when nothing was filed.
    pub(super) sync_status: Option<String>,
}

impl DrainReport {
    pub(super) fn filed(
        &self,
    ) -> impl Iterator<Item = (&IntakeUnit, &Ingested)> {
        self.items.iter().filter_map(|item| match item {
            DrainItem::Filed { unit, ingested } => {
                Some((unit, ingested.as_ref()))
            }
            _ => None,
        })
    }

    pub(super) fn filed_count(&self) -> usize {
        self.filed().count()
    }

    /// Items still waiting in `xlib/`: queued by a reason, or stopped
    /// mid-ingest.
    pub(super) fn stuck_count(&self) -> usize {
        self.items.len() - self.filed_count()
    }
}

/// What a dry run reports for one queued item.
pub(super) enum QueuePreview {
    Would {
        unit: IntakeUnit,
        note: PathBuf,
    },
    WouldQueue {
        unit: IntakeUnit,
        reason: QueueReason,
        detail: String,
    },
}

pub(super) enum IntakeOnlyRun {
    /// `xlib/` holds nothing to file; no lock was taken and nothing synced.
    Empty,
    Preview(Vec<QueuePreview>),
    Drained(DrainReport),
}

fn xlib_relative(config: &Config, path: &Path) -> Result<PathBuf> {
    path.strip_prefix(&config.xlib_dir)
        .map(Path::to_path_buf)
        .map_err(|error| {
            CommandError::new(format!(
                "derive xlib-relative path for {}: {error}",
                path.display()
            ))
        })
}

/// Every queued item, sorted: each xlib PDF as one unit (its companions and
/// same-stem audio ride along), then each xlib audio file whose library PDF
/// already exists. Audio without any PDF is not drainable and is skipped.
/// Takes no locks.
pub(super) fn enumerate_queue(config: &Config) -> Result<Vec<IntakeUnit>> {
    if !config.xlib_dir.is_dir() {
        return Ok(Vec::new());
    }
    let mut pdfs = Vec::new();
    collect_pdf_paths_from_dir(&config.xlib_dir, &mut pdfs)?;
    pdfs.sort();
    let mut queued_stems = BTreeSet::new();
    let mut units = Vec::new();
    for source in pdfs {
        let relative = xlib_relative(config, &source)?;
        queued_stems.insert(relative.with_extension(""));
        units.push(IntakeUnit::Pdf {
            destination: config.lib_dir.join(&relative),
            source,
        });
    }
    let mut audio_files = Vec::new();
    collect_audio_paths_from_dir(&config.xlib_dir, &mut audio_files)?;
    audio_files.sort();
    for source in audio_files {
        let relative = xlib_relative(config, &source)?;
        if queued_stems.contains(&relative.with_extension("")) {
            continue;
        }
        let Some(pdf) = library_pdf_for_xlib_audio(config, &source) else {
            continue;
        };
        units.push(IntakeUnit::Audio {
            destination: config.lib_dir.join(&relative),
            source,
            pdf,
        });
    }
    Ok(units)
}

/// Local, lock-free checks a drain would also make: collisions, sizes, and
/// the note path. The pulled tree can still add a collision later.
fn preflight_unit(
    config: &Config,
    unit: &IntakeUnit,
) -> std::result::Result<PathBuf, QueueFailure> {
    if !matches!(unit, IntakeUnit::Audio { .. }) {
        let occupants = recheck_reread_occupancy(
            config,
            unit.library_pdf(),
            &reread_own_files(unit),
        )
        .map_err(|error| {
            QueueFailure::new(QueueReason::Refused, error.to_string())
        })?;
        if !occupants.is_empty() {
            return Err(reread_collision(unit.library_pdf(), occupants));
        }
    }
    let moves = unit_moves(config, unit)?;
    validate_unit_collisions(config, unit, &moves)?;
    validate_move_sizes(&moves)?;
    ref_note_path(config, unit.library_pdf()).map_err(|error| {
        QueueFailure::new(QueueReason::Refused, error.to_string())
    })
}

pub(super) fn preview_queue(
    config: &Config,
    units: Vec<IntakeUnit>,
) -> Vec<QueuePreview> {
    units
        .into_iter()
        .map(|unit| match preflight_unit(config, &unit) {
            Ok(note) => QueuePreview::Would { unit, note },
            Err(failure) => QueuePreview::WouldQueue {
                unit,
                reason: failure.reason,
                detail: failure.detail,
            },
        })
        .collect()
}

fn session_error(failure: QueueFailure) -> CommandError {
    CommandError::new(format!("{}: {}", failure.reason.label(), failure.detail))
        .with_code(failure.reason.as_str())
}

/// Drain the local `xlib/` queue for `bob ref scan --intake-only`.
///
/// Validates the layout and looks for work without locks; an empty queue
/// costs nothing. Otherwise it opens one session (`bob_sync.lock`, then
/// `scan.lock`), pulls once, files every unit with its own commit, and
/// pushes once. A unit that cannot be filed stays queued and the drain
/// moves on; a unit that stops mid-ingest ends the drain. `on_item` sees
/// each result as it happens.
/// Update the per-host intake status record for one real drain. An empty
/// queue leaves the record alone.
fn record_drain_status(run: &Result<IntakeOnlyRun>) {
    const COMMAND: &str = "bob ref scan -i";
    match run {
        Err(error) => {
            if let Some(warning) = intake_status::record_failure(
                COMMAND,
                error.code.unwrap_or("scan_failed"),
                &error.message,
            ) {
                eprintln!("{warning}");
            }
        }
        Ok(IntakeOnlyRun::Drained(report)) => {
            if report.filed_count() > 0 {
                if let Some(warning) =
                    intake_status::record_success(COMMAND, report.filed_count())
                {
                    eprintln!("{warning}");
                }
            }
            let stuck = report.items.iter().find_map(|item| match item {
                DrainItem::Filed { .. } => None,
                DrainItem::Queued { reason, detail, .. } => {
                    Some((reason.as_str(), detail.as_str()))
                }
                DrainItem::Incomplete { error, .. } => {
                    Some(("incomplete", error.as_str()))
                }
            });
            if let Some((reason, detail)) = stuck {
                if let Some(warning) =
                    intake_status::record_failure(COMMAND, reason, detail)
                {
                    eprintln!("{warning}");
                }
            }
        }
        Ok(IntakeOnlyRun::Empty | IntakeOnlyRun::Preview(_)) => {}
    }
}

/// Whether an intake-only drain acquired the scan writer lock. A drain
/// writes the scan receipt exactly when the lock was acquired: writing
/// drains and hard failures after the lock. Dry runs, empty queues, and
/// scans that never got the lock write none.
pub(super) struct IntakeOnlyOutcome {
    pub(super) run: Result<IntakeOnlyRun>,
    pub(super) lock_held: bool,
}

/// Drain with lock tracking for the scan receipt. See [`drain_queue`]: the
/// flag is true exactly when the scan writer lock was acquired.
pub(super) fn run_intake_only_tracked(
    config: &Config,
    dry_run: bool,
    progress: &mut dyn FnMut(&str),
    on_item: &mut dyn FnMut(&DrainItem),
) -> IntakeOnlyOutcome {
    let (run, lock_held) = drain_queue(config, dry_run, progress, on_item);
    if !dry_run {
        record_drain_status(&run);
    }
    IntakeOnlyOutcome { run, lock_held }
}

fn drain_queue(
    config: &Config,
    dry_run: bool,
    progress: &mut dyn FnMut(&str),
    on_item: &mut dyn FnMut(&DrainItem),
) -> (Result<IntakeOnlyRun>, bool) {
    // Recover an interrupted overwrite before draining: a pending intent
    // blocks even dry runs (which stay read-only) until a writing command
    // repairs the vault. This hook owns its own locks; the session below
    // locks again sequentially. Recovery notices use the stderr progress
    // reporter so JSON stdout stays parseable.
    match super::overwrite::recovery::recover_before_command(config, dry_run) {
        Ok(notice) => {
            if let Some(notice) = notice {
                progress(&notice);
            }
        }
        Err(error) => return (Err(error), false),
    }
    let layout = (|| -> Result<Vec<IntakeUnit>> {
        validate_library_layout(config)?;
        enumerate_queue(config)
    })();
    let units = match layout {
        Ok(units) => units,
        Err(error) => return (Err(error), false),
    };
    if units.is_empty() {
        return (Ok(IntakeOnlyRun::Empty), false);
    }
    if dry_run {
        return (
            Ok(IntakeOnlyRun::Preview(preview_queue(config, units))),
            false,
        );
    }

    let session = match IntakeSession::open(config, progress) {
        Ok(session) => session,
        Err(failure) => return (Err(session_error(failure)), false),
    };
    if let Err(error) = session.pre_sync().map_err(session_error) {
        return (Err(error), true);
    }
    // The pull cannot add to the gitignored queue, but another drain may have
    // emptied it while this one waited for the locks.
    let units = match enumerate_queue(config) {
        Ok(units) => units,
        Err(error) => return (Err(error), true),
    };
    let mut items = Vec::new();
    let mut stopped = false;
    for unit in units {
        let item = if stopped {
            DrainItem::Queued {
                unit,
                reason: QueueReason::Refused,
                detail: "not attempted: an earlier item stopped mid-ingest"
                    .to_string(),
            }
        } else {
            match session.ingest(config, &unit, DRAIN_PRODUCER) {
                Ok(ingested) => {
                    for path in &ingested.ignored_paths {
                        progress(&ignored_path_warning(path));
                    }
                    DrainItem::Filed {
                        unit,
                        ingested: Box::new(ingested),
                    }
                }
                Err(IngestFailure::Queued(failure)) => DrainItem::Queued {
                    unit,
                    reason: failure.reason,
                    detail: failure.detail,
                },
                Err(IngestFailure::Incomplete(error)) => {
                    stopped = true;
                    DrainItem::Incomplete { unit, error }
                }
            }
        };
        on_item(&item);
        items.push(item);
    }
    let filed = items
        .iter()
        .any(|item| matches!(item, DrainItem::Filed { .. }));
    // An incomplete ingest leaves uncommitted writes behind; syncing now
    // would commit them. Background vault-sync publishes the filed commits.
    let sync_status = match (filed, stopped) {
        (false, _) => None,
        (true, false) => Some(session.post_sync()),
        (true, true) => Some(
            "push pending (an item stopped mid-ingest); background vault-sync will publish it"
                .to_string(),
        ),
    };
    (
        Ok(IntakeOnlyRun::Drained(DrainReport { items, sync_status })),
        true,
    )
}

/// Human `bob ref scan --intake-only`. Returns the process exit code: 0 when
/// the queue is empty or fully filed (or on `--dry-run`), 1 when an item is
/// stuck or the drain could not start. A writing drain holds the scan
/// writer lock, so it writes the same envelope line JSON mode would print
/// as the scan receipt; dry runs, empty queues, and lock-busy scans write
/// none.
pub(super) fn scan_intake_only(config: &Config, dry_run: bool) -> i32 {
    let mode = if dry_run { "dry_run" } else { "write" };
    let styler = Styler::detect();
    let mut progress = |line: &str| eprintln!("{line}");
    let mut on_item =
        |item: &DrainItem| print_drain_item(config, &styler, item);
    let outcome =
        run_intake_only_tracked(config, dry_run, &mut progress, &mut on_item);
    match outcome.run {
        Err(error) => {
            if outcome.lock_held {
                emit_scan_receipt(&render_scan_error_envelope(
                    config,
                    SyncOptions {
                        dry_run,
                        write_pdf: false,
                        prefer: None,
                    },
                    mode,
                    Vec::new(),
                    &error,
                    true,
                ));
            }
            eprintln!("{COMMAND_NAME}: {error}");
            1
        }
        Ok(IntakeOnlyRun::Empty) => {
            println!("intake: nothing queued");
            0
        }
        Ok(IntakeOnlyRun::Preview(previews)) => {
            print_queue_preview(config, &styler, &previews);
            0
        }
        Ok(IntakeOnlyRun::Drained(report)) => {
            if let Some(status) = &report.sync_status {
                println!("sync: {status}");
            }
            println!(
                "intake: filed {} · queued {}",
                report.filed_count(),
                report.stuck_count()
            );
            let run = IntakeOnlyRun::Drained(report);
            let (line, code) =
                render_intake_only_envelope(config, mode, dry_run, &run);
            emit_scan_receipt(&line);
            code
        }
    }
}

fn print_drain_item(config: &Config, styler: &Styler, item: &DrainItem) {
    match item {
        DrainItem::Filed { unit, ingested } => {
            println!("{}", filed_line(config, styler, unit, ingested));
            print_commit_line(ingested);
        }
        DrainItem::Queued {
            unit,
            reason,
            detail,
        } => println!(
            "{} {} ({}: {})",
            styler.yellow("queued:"),
            unit.display_path(config).display(),
            reason.label(),
            detail
        ),
        DrainItem::Incomplete { unit, error } => println!(
            "{} {} ({})",
            styler.red("incomplete:"),
            unit.display_path(config).display(),
            error
        ),
    }
}

fn filed_line(
    config: &Config,
    styler: &Styler,
    unit: &IntakeUnit,
    ingested: &Ingested,
) -> String {
    if let IntakeUnit::Audio { destination, .. } = unit {
        return format!(
            "{} {} → {}",
            styler.green("attached audio:"),
            report_path(config, destination),
            report_path(config, &ingested.note)
        );
    }
    ingested_line_with_label(config, &styler.green("ingested:"), ingested)
}

fn ingested_line_with_label(
    config: &Config,
    label: &str,
    ingested: &Ingested,
) -> String {
    let mut line = format!(
        "{label} {} → {}",
        report_path(config, &ingested.pdf),
        report_path(config, &ingested.note)
    );
    if let (Some(destination), Some(block_id)) =
        (&ingested.task_destination, &ingested.task_block_id)
    {
        line.push_str(&format!(
            " · task → {} ^{block_id}",
            report_path(config, destination)
        ));
    }
    if ingested.adopted {
        line.push_str(" (adopted)");
    }
    line
}

fn print_commit_line(ingested: &Ingested) {
    if let Some(warning) = &ingested.commit_warning {
        println!(
            "sync: commit failed ({warning}); background vault-sync will commit it"
        );
    } else if let Some(sha) = &ingested.commit_sha {
        println!("sync: committed {}", &sha[..sha.len().min(7)]);
    } else {
        println!("sync: already committed");
    }
}

fn print_queue_preview(
    config: &Config,
    styler: &Styler,
    previews: &[QueuePreview],
) {
    let mut would = 0usize;
    let mut queued = 0usize;
    for preview in previews {
        match preview {
            QueuePreview::Would { unit, note } => {
                would += 1;
                match unit {
                    IntakeUnit::Audio { destination, .. } => println!(
                        "{} {} → {}",
                        styler.green("would attach audio:"),
                        unit.display_path(config).display(),
                        report_path(config, destination)
                    ),
                    _ => println!(
                        "{} {} → {} → {}",
                        styler.green("would ingest:"),
                        unit.display_path(config).display(),
                        report_path(config, unit.library_pdf()),
                        report_path(config, note)
                    ),
                }
            }
            QueuePreview::WouldQueue {
                unit,
                reason,
                detail,
            } => {
                queued += 1;
                println!(
                    "{} {} ({}: {})",
                    styler.yellow("would queue:"),
                    unit.display_path(config).display(),
                    reason.label(),
                    detail
                );
            }
        }
    }
    println!("intake: would file {would} · would queue {queued}");
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
        IntakeOutcome::Kept { why } | IntakeOutcome::WouldKeep { why } => {
            let _ = why;
            print_next_step(plan);
            Ok(())
        }
        IntakeOutcome::Incomplete { error } => {
            print_next_step(plan);
            Err(CommandError::new(error))
        }
        other => print_intake_result(config, other),
    }
}

/// Print ingested, queued, or dry-run intake without a create target plan.
pub(super) fn print_intake_result(
    config: &Config,
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
                    "sync: commit failed ({warning}); background vault-sync will commit it · {sync_status}"
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
        IntakeOutcome::WouldIngest { pdf, note } => {
            println!(
                "intake: would ingest into {} → {} (pull, file note and reading task, commit, push)",
                report_path(config, &pdf),
                report_path(config, &note)
            );
            Ok(())
        }
        IntakeOutcome::Kept { .. }
        | IntakeOutcome::WouldKeep { .. }
        | IntakeOutcome::Incomplete { .. } => Err(CommandError::new(
            "internal: kept or incomplete intake needs the create next-step printer",
        )),
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
    fn queue_enumeration_lists_pdfs_then_late_audio_and_skips_the_rest() {
        let temp = tempfile::tempdir().expect("create temp vault");
        let config = config(temp.path());
        for relative in [
            "xlib/chat/b.pdf",
            "xlib/chat/b.mp3",
            "xlib/chat/a.pdf",
            "xlib/paper/late.mp3",
            "xlib/paper/orphan.mp3",
            "lib/paper/late.pdf",
        ] {
            let path = temp.path().join(relative);
            fs::create_dir_all(path.parent().expect("parent"))
                .expect("create parent");
            fs::write(path, b"x").expect("write fixture");
        }

        let units = enumerate_queue(&config).expect("enumerate queue");
        let shapes = units
            .iter()
            .map(|unit| match unit {
                IntakeUnit::Pdf { source, .. } => format!(
                    "pdf {}",
                    source.strip_prefix(temp.path()).unwrap().display()
                ),
                IntakeUnit::Audio { source, .. } => format!(
                    "audio {}",
                    source.strip_prefix(temp.path()).unwrap().display()
                ),
                IntakeUnit::LibraryPdf { .. } => unreachable!(),
            })
            .collect::<Vec<_>>();
        assert_eq!(
            shapes,
            [
                "pdf xlib/chat/a.pdf",
                "pdf xlib/chat/b.pdf",
                "audio xlib/paper/late.mp3",
            ],
            "audio beside a queued PDF rides with it; audio with no PDF is skipped"
        );
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

    #[test]
    fn plan_error_with_needs_write_pdfs_code_queues_that_reason() {
        let coded = CommandError::new(
            "reference note changed but --write-pdf was not supplied; refusing to update the PDF marker",
        )
        .with_code("needs_write_pdfs");
        assert_eq!(
            queue_reason_for_plan_error(&coded),
            QueueReason::NeedsWritePdfs
        );
        let other = CommandError::new("planning refused");
        assert_eq!(queue_reason_for_plan_error(&other), QueueReason::Refused);
        let mismatched_text = CommandError::new("requires --write-pdf");
        assert_eq!(
            queue_reason_for_plan_error(&mismatched_text),
            QueueReason::Refused
        );
    }
}
