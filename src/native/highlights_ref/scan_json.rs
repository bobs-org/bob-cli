//! Versioned JSON report for `bob ref scan --format json`.
//!
//! Human mode stays byte-identical (see `sync.rs`); this module mirrors
//! its stage order but collects a machine-readable envelope instead of
//! printing display lines. Stdout carries exactly one compact JSON line;
//! hook chatter goes to stderr, and hard failures print a coded error
//! envelope with exit 1 and no `bob ref:` stderr line.
use super::*;

use crate::native::ref_library::output::{generated_at, REF_SCHEMA_VERSION};

/// Envelope `command` value shared by both JSON shapes.
const SCAN_COMMAND: &str = "ref scan";

/// Internal override for the writer-lock wait, in seconds (tests).
const ENV_SCAN_LOCK_WAIT_SECONDS: &str = "BOB_REF_SCAN_LOCK_WAIT_SECONDS";

/// Default bounded wait for the scan writer lock, in seconds.
const SCAN_LOCK_WAIT_SECONDS_DEFAULT: f64 = 120.0;

/// Poll interval while waiting for the scan writer lock.
const SCAN_LOCK_POLL: std::time::Duration =
    std::time::Duration::from_millis(250);

/// The `hook` object inside the success envelope.
#[derive(Debug, Clone, Serialize)]
struct ScanJsonHook {
    status: &'static str,
    command: Option<String>,
}

/// One PDF intake move: vault-relative `from` and `to`, in move order.
/// Sidecar and audio companion moves are not listed.
#[derive(Debug, Clone, Serialize)]
pub(super) struct ScanJsonIntakeMove {
    from: String,
    to: String,
}

/// The `summary` object: the same counts the human summary line prints.
#[derive(Debug, Clone, Serialize)]
struct ScanJsonSummary {
    pdfs: usize,
    created: usize,
    updated: usize,
    unchanged: usize,
    markers: usize,
    tasks: usize,
    failures: usize,
}

/// One created or updated reference note, in scan order.
#[derive(Debug, Clone, Serialize)]
struct ScanJsonNote {
    action: &'static str,
    path: String,
    title: String,
    ref_type: Option<String>,
    source_pdf: String,
    marker: bool,
    #[serde(skip_serializing_if = "is_zero_usize")]
    image_notes: usize,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    image_notes_unresolved: Vec<ScanJsonUnresolvedImageNote>,
    #[serde(skip_serializing_if = "is_zero_usize")]
    continuations: usize,
    #[serde(skip_serializing_if = "is_zero_usize")]
    continuations_dangling: usize,
}

/// One unresolved image note on a scanned reference.
#[derive(Debug, Clone, Serialize)]
struct ScanJsonUnresolvedImageNote {
    source: String,
    state: UnresolvedImageNoteState,
}

fn is_zero_usize(value: &usize) -> bool {
    *value == 0
}

/// One per-PDF failure: the same text the human report prints.
#[derive(Debug, Clone, Serialize)]
struct ScanJsonFailure {
    pdf: String,
    stage: &'static str,
    message: String,
}

/// One `--intake-only` item that is still queued in `xlib/` (or would be
/// on a dry run): vault-relative PDF or audio path, a stable
/// [`QueueReason`] slug, and the human detail.
#[derive(Debug, Clone, Serialize)]
struct ScanJsonQueued {
    pdf: String,
    reason: &'static str,
    message: String,
}

/// Success and partial-failure envelope. Field order is the contract.
#[derive(Debug, Clone, Serialize)]
struct ScanReportEnvelope {
    ok: bool,
    schema_version: u32,
    command: &'static str,
    generated_at: String,
    mode: &'static str,
    write_pdfs: bool,
    hook: ScanJsonHook,
    intake: Vec<ScanJsonIntakeMove>,
    summary: ScanJsonSummary,
    notes: Vec<ScanJsonNote>,
    failures: Vec<ScanJsonFailure>,
    intake_only: bool,
    queued: Vec<ScanJsonQueued>,
}

/// The `error` body inside the hard-failure envelope.
#[derive(Debug, Clone, Serialize)]
struct ScanErrorBody {
    code: String,
    message: String,
    hint: Option<String>,
    paths: Vec<String>,
}

/// Hard-failure envelope (`ok: false`, exit 1). Field order is fixed.
#[derive(Debug, Clone, Serialize)]
struct ScanErrorEnvelope {
    ok: bool,
    schema_version: u32,
    command: &'static str,
    generated_at: String,
    mode: &'static str,
    write_pdfs: bool,
    intake: Vec<ScanJsonIntakeMove>,
    error: ScanErrorBody,
    intake_only: bool,
    queued: Vec<ScanJsonQueued>,
}

/// Vault-relative forward-slash path, the same way `ref_library` strips
/// the vault prefix for `ref list` rows. Never canonicalizes, so the
/// value is byte-identical to the `path` that `bob ref list -f json`
/// reports for the same note.
fn scan_vault_relative(config: &Config, path: &Path) -> String {
    path.strip_prefix(&config.bob_dir)
        .map(|relative| relative.to_string_lossy().replace('\\', "/"))
        .unwrap_or_else(|_| path.to_string_lossy().into_owned())
}

/// Take the shared vault-maintenance lock (`bob_sync.lock`) that writing
/// scans hold before `scan.lock`. Dry runs never lock. On contention the
/// wait line goes to stderr once in both modes, then the lock retries
/// every 250 ms for up to 120 s (`BOB_REF_SCAN_LOCK_WAIT_SECONDS`
/// overrides both this wait and the scan-lock wait) before failing with
/// `lock_busy`. The returned file holds the lock until the scan returns.
pub(super) fn acquire_scan_maintenance_lock(
    dry_run: bool,
) -> Result<Option<fs::File>> {
    if dry_run {
        return Ok(None);
    }
    let timeout = scan_lock_wait_duration();
    ob::acquire_lock_waiting(timeout, || {
        eprintln!("waiting for another vault maintenance run…");
    })
    .map(Some)
    .map_err(|error| {
        CommandError::new(format!(
            "could not acquire the vault maintenance lock: {error}"
        ))
        .with_code("lock_busy")
    })
}

/// Take the exclusive writer lock serializing writing scans
/// (`bob_cli_state_dir()/ref/scan.lock`). Dry runs never lock. On
/// contention the wait line goes to stderr once in both modes, then the
/// lock retries every 250 ms for up to 120 s (`BOB_REF_SCAN_LOCK_WAIT_SECONDS`
/// overrides, for tests) before failing with `scan_busy`. The returned
/// file holds the lock until the scan returns. Callers take
/// [`acquire_scan_maintenance_lock`] first.
pub(super) fn acquire_scan_writer_lock(
    dry_run: bool,
) -> Result<Option<fs::File>> {
    if dry_run {
        return Ok(None);
    }
    let dir = bob_env::bob_cli_state_dir().join("ref");
    let path = dir.join("scan.lock");
    let file = bob_env::create_state_lock_file(&dir, "scan.lock").map_err(
        |error| {
            CommandError::new(format!(
                "open scan lock {}: {error}",
                path.display()
            ))
        },
    )?;
    {
        use fs2::FileExt;
        if file.try_lock_exclusive().is_ok() {
            return Ok(Some(file));
        }
    }
    eprintln!("waiting for another bob ref scan to finish…");
    let wait_secs = scan_lock_wait_seconds();
    let start = std::time::Instant::now();
    loop {
        if start.elapsed().as_secs_f64() >= wait_secs {
            break;
        }
        std::thread::sleep(SCAN_LOCK_POLL);
        {
            use fs2::FileExt;
            if file.try_lock_exclusive().is_ok() {
                return Ok(Some(file));
            }
        }
    }
    Err(CommandError::new("another bob ref scan is still running")
        .with_code("scan_busy"))
}

/// Take the scan writer lock with a caller-specific bound. Reference renames
/// use a shorter budget than scans themselves so the command stays within the
/// Bob Mac Capture request timeout while still serializing with scans.
pub(super) fn acquire_scan_writer_lock_with_timeout(
    timeout: std::time::Duration,
) -> Result<fs::File> {
    acquire_scan_writer_lock_with_timeout_and_wait(timeout, || {
        eprintln!("waiting for another bob ref scan to finish…");
    })
}

pub(super) fn acquire_scan_writer_lock_with_timeout_and_wait(
    timeout: std::time::Duration,
    on_first_wait: impl FnOnce(),
) -> Result<fs::File> {
    let dir = bob_env::bob_cli_state_dir().join("ref");
    let path = dir.join("scan.lock");
    let file = bob_env::create_state_lock_file(&dir, "scan.lock").map_err(
        |error| {
            CommandError::new(format!(
                "open scan lock {}: {error}",
                path.display()
            ))
        },
    )?;
    {
        use fs2::FileExt;
        if file.try_lock_exclusive().is_ok() {
            return Ok(file);
        }
    }
    on_first_wait();
    let started = std::time::Instant::now();
    loop {
        if started.elapsed() >= timeout {
            return Err(CommandError::new(format!(
                "another bob ref scan is still running (waited {} seconds)",
                timeout.as_secs()
            ))
            .with_code("scan_busy"));
        }
        std::thread::sleep(
            SCAN_LOCK_POLL.min(timeout.saturating_sub(started.elapsed())),
        );
        use fs2::FileExt;
        if file.try_lock_exclusive().is_ok() {
            return Ok(file);
        }
    }
}

/// Bounded writer-lock wait, in seconds.
fn scan_lock_wait_seconds() -> f64 {
    bob_env::var(ENV_SCAN_LOCK_WAIT_SECONDS)
        .ok()
        .and_then(|value| value.parse::<f64>().ok())
        .filter(|value| value.is_finite() && *value >= 0.0)
        .unwrap_or(SCAN_LOCK_WAIT_SECONDS_DEFAULT)
}

fn scan_lock_wait_duration() -> std::time::Duration {
    let secs = scan_lock_wait_seconds();
    if secs <= 0.0 {
        std::time::Duration::ZERO
    } else {
        std::time::Duration::from_secs_f64(secs)
    }
}

/// Run `scan` in JSON mode: print exactly one envelope line and return
/// the exit code (0, 1, or nothing else; usage errors never reach here).
pub(super) fn scan_library_json(
    config: &Config,
    options: SyncOptions,
    jobs: usize,
    no_hooks: bool,
) -> i32 {
    let mode = if options.dry_run { "dry_run" } else { "write" };

    if let Err(error) = validate_library_layout(config) {
        return print_scan_error_envelope(
            config,
            options,
            mode,
            &[],
            0,
            &error,
        );
    }

    let _maintenance_lock = match acquire_scan_maintenance_lock(options.dry_run)
    {
        Ok(lock) => lock,
        Err(error) => {
            return print_scan_error_envelope(
                config,
                options,
                mode,
                &[],
                0,
                &error,
            );
        }
    };
    let _scan_lock = match acquire_scan_writer_lock(options.dry_run) {
        Ok(lock) => lock,
        Err(error) => {
            return print_scan_error_envelope(
                config,
                options,
                mode,
                &[],
                0,
                &error,
            );
        }
    };

    let resolved_hook: Option<PreScanHook> = if no_hooks {
        None
    } else {
        match configured_pre_scan_hook(false) {
            Ok(hook) => hook,
            Err(error) => {
                return print_scan_error_envelope(
                    config,
                    options,
                    mode,
                    &[],
                    0,
                    &error,
                );
            }
        }
    };
    let (hook_status, hook_command) = if no_hooks {
        let peeked = configured_pre_scan_hook(false)
            .ok()
            .flatten()
            .map(|hook| hook.display());
        ("skipped", peeked)
    } else {
        match &resolved_hook {
            Some(hook) => (
                if options.dry_run { "would_run" } else { "ran" },
                Some(hook.display()),
            ),
            None => (
                if bob_env::var_os(ENV_PRE_SCAN_HOOK).is_some_and(|value| {
                    value.to_string_lossy().trim().is_empty()
                }) {
                    "skipped"
                } else {
                    "none"
                },
                None,
            ),
        }
    };
    let hook = ScanJsonHook {
        status: hook_status,
        command: hook_command,
    };

    if let Err(error) =
        run_pre_scan_hook_json(config, resolved_hook.as_ref(), options.dry_run)
    {
        return print_scan_error_envelope(
            config,
            options,
            mode,
            &[],
            0,
            &error,
        );
    }

    let intake = match plan_xlib_intake(config) {
        Ok(intake) => intake,
        Err(error) => {
            return print_scan_error_envelope(
                config,
                options,
                mode,
                &[],
                0,
                &error,
            );
        }
    };
    let mut completed_intake = 0usize;
    if !options.dry_run {
        let (moved, result) = execute_xlib_intake_counting(&intake);
        completed_intake = moved;
        if let Err(error) = result {
            return print_scan_error_envelope(
                config,
                options,
                mode,
                &intake,
                completed_intake,
                &error,
            );
        }
        completed_intake = intake.len();
    }

    let pdfs = match collect_pdf_paths(config, options.dry_run) {
        Ok(pdfs) => pdfs,
        Err(error) => {
            return print_scan_error_envelope(
                config,
                options,
                mode,
                &intake,
                completed_intake,
                &error,
            );
        }
    };
    if let Err(error) = validate_output_collisions(config, &pdfs) {
        return print_scan_error_envelope(
            config,
            options,
            mode,
            &intake,
            completed_intake,
            &error,
        );
    }

    let ctx = ScanContext::build(config);
    let mut plan_outcomes = pdfs
        .iter()
        .zip(plan_pdfs(config, &pdfs, options, jobs, &ctx))
        .map(|(pdf, result)| match result {
            Ok(plan) => ScanPlanOutcome::Planned(Box::new(plan)),
            Err(error) => ScanPlanOutcome::Failed(ScanFailure {
                pdf: pdf.clone(),
                error,
            }),
        })
        .collect::<Vec<_>>();
    let mut plans_to_finalize = plan_outcomes
        .iter_mut()
        .filter_map(|outcome| match outcome {
            ScanPlanOutcome::Planned(plan) => Some(plan.as_mut()),
            ScanPlanOutcome::Failed(_) => None,
        })
        .collect::<Vec<_>>();
    reserve_preview_ids(config, &mut plans_to_finalize, &ctx);
    if let Err(error) =
        finalize_annotation_task_plans(config, &mut plans_to_finalize, &ctx)
    {
        return print_scan_error_envelope(
            config,
            options,
            mode,
            &intake,
            completed_intake,
            &error,
        );
    }
    let plans = plan_outcomes
        .iter()
        .filter_map(|outcome| match outcome {
            ScanPlanOutcome::Planned(plan) => Some(plan.as_ref()),
            ScanPlanOutcome::Failed(_) => None,
        })
        .collect::<Vec<_>>();
    if let Err(error) = validate_planned_asset_collisions(&plans) {
        return print_scan_error_envelope(
            config,
            options,
            mode,
            &intake,
            completed_intake,
            &error,
        );
    }
    let plan_failures = plan_outcomes
        .iter()
        .filter_map(|outcome| match outcome {
            ScanPlanOutcome::Planned(_) => None,
            ScanPlanOutcome::Failed(failure) => Some(failure),
        })
        .collect::<Vec<_>>();

    if options.dry_run {
        let counts = ScanCounts::from_plans(&plans);
        let notes = plan_outcomes
            .iter()
            .filter_map(|outcome| match outcome {
                ScanPlanOutcome::Planned(plan)
                    if plan.stable_note_action != "none" =>
                {
                    Some(scan_note_entry(
                        config,
                        plan,
                        plan.stable_note_action,
                        plan.marker_write_needed,
                    ))
                }
                ScanPlanOutcome::Planned(_) | ScanPlanOutcome::Failed(_) => {
                    None
                }
            })
            .collect::<Vec<_>>();
        let failures = plan_failures
            .iter()
            .map(|failure| scan_failure_entry(config, failure, "plan"))
            .collect::<Vec<_>>();
        return print_scan_report_envelope(
            config,
            options,
            mode,
            hook,
            &intake,
            counts,
            pdfs.len(),
            plan_failures.len(),
            notes,
            failures,
        );
    }

    if let Err(error) = ensure_safe_to_write(config, plans.iter().copied()) {
        return print_scan_error_envelope(
            config,
            options,
            mode,
            &intake,
            completed_intake,
            &error,
        );
    }
    let mut run_state = RunWriteState::default();
    let mut write_outcomes = Vec::new();
    for plan in &plans {
        match execute_pdf_sync(config, plan, &mut run_state) {
            Ok(report) => {
                write_outcomes.push(ScanWriteOutcome::Written(report))
            }
            Err(error) => {
                write_outcomes.push(ScanWriteOutcome::Failed(ScanFailure {
                    pdf: plan.pdf.clone(),
                    error,
                }));
            }
        }
    }
    let reports = write_outcomes
        .iter()
        .filter_map(|outcome| match outcome {
            ScanWriteOutcome::Written(report) => Some(report.clone()),
            ScanWriteOutcome::Failed(_) => None,
        })
        .collect::<Vec<_>>();
    let counts = ScanCounts::from_reports(&reports);
    let mut write_index = 0usize;
    let mut notes = Vec::new();
    let mut failures = Vec::new();
    for outcome in &plan_outcomes {
        match outcome {
            ScanPlanOutcome::Planned(plan) => {
                let write_outcome = write_outcomes.get(write_index);
                write_index += 1;
                match write_outcome {
                    Some(ScanWriteOutcome::Written(report))
                        if report.note_action != "none" =>
                    {
                        notes.push(scan_note_entry(
                            config,
                            plan,
                            report.note_action,
                            report.marker_action != "none",
                        ));
                    }
                    Some(ScanWriteOutcome::Failed(failure)) => {
                        failures
                            .push(scan_failure_entry(config, failure, "write"));
                    }
                    Some(ScanWriteOutcome::Written(_)) | None => {}
                }
            }
            ScanPlanOutcome::Failed(failure) => {
                failures.push(scan_failure_entry(config, failure, "plan"));
            }
        }
    }
    let failure_count = plan_failures.len()
        + write_outcomes
            .iter()
            .filter(|outcome| matches!(outcome, ScanWriteOutcome::Failed(_)))
            .count();
    print_scan_report_envelope(
        config,
        options,
        mode,
        hook,
        &intake,
        counts,
        pdfs.len(),
        failure_count,
        notes,
        failures,
    )
}

/// One note entry: the path `ref list` reports, the title the scan
/// wrote, the plan's stable metadata, and the marker flag.
fn scan_note_entry(
    config: &Config,
    plan: &PdfSyncPlan,
    action: &'static str,
    marker: bool,
) -> ScanJsonNote {
    let (
        image_notes,
        image_notes_unresolved,
        continuations,
        continuations_dangling,
    ) = plan
        .rendered_highlights
        .as_ref()
        .map(|rendered| {
            (
                rendered.image_note_count,
                rendered
                    .image_notes_unresolved
                    .iter()
                    .map(|note| ScanJsonUnresolvedImageNote {
                        source: note.source_text.clone(),
                        state: note.state,
                    })
                    .collect(),
                rendered.continuation_count,
                rendered.dangling_continuations.len(),
            )
        })
        .unwrap_or((0, Vec::new(), 0, 0));
    ScanJsonNote {
        action,
        path: scan_vault_relative(config, &plan.note_path),
        title: note_title(&plan.pdf, &plan.synced_projection),
        ref_type: plan.stable_metadata.ref_type.clone(),
        source_pdf: plan.stable_metadata.source_pdf.clone(),
        marker,
        image_notes,
        image_notes_unresolved,
        continuations,
        continuations_dangling,
    }
}

/// One failure entry, with the same text the human report prints.
fn scan_failure_entry(
    config: &Config,
    failure: &ScanFailure,
    stage: &'static str,
) -> ScanJsonFailure {
    let message = match ScanLine::from_failure(failure, stage == "write") {
        ScanLine::Failed { message, .. } => message,
        ScanLine::Success { .. } => failure.error.to_string(),
    };
    ScanJsonFailure {
        pdf: scan_vault_relative(config, &failure.pdf),
        stage,
        message,
    }
}

/// Print the success/partial envelope as one compact JSON line and
/// return its exit code: 0 exactly when `failures` is empty.
#[allow(clippy::too_many_arguments)]
fn print_scan_report_envelope(
    config: &Config,
    options: SyncOptions,
    mode: &'static str,
    hook: ScanJsonHook,
    intake: &[IntakeMove],
    counts: ScanCounts,
    pdf_count: usize,
    failure_count: usize,
    notes: Vec<ScanJsonNote>,
    failures: Vec<ScanJsonFailure>,
) -> i32 {
    let ok = failures.is_empty();
    let envelope = ScanReportEnvelope {
        ok,
        schema_version: REF_SCHEMA_VERSION,
        command: SCAN_COMMAND,
        generated_at: generated_at(),
        mode,
        write_pdfs: options.write_pdf,
        hook,
        intake: intake_moves(config, intake),
        summary: ScanJsonSummary {
            pdfs: pdf_count,
            created: counts.creates,
            updated: counts.updates,
            unchanged: counts.unchanged,
            markers: counts.marker_updates,
            tasks: counts.annotation_tasks_created,
            failures: failure_count,
        },
        notes,
        failures,
        intake_only: false,
        queued: Vec::new(),
    };
    emit_scan_report_envelope(&envelope)
}

/// Print one report envelope line; the exit code is 0 exactly when `ok`.
fn emit_scan_report_envelope(envelope: &ScanReportEnvelope) -> i32 {
    println!(
        "{}",
        serde_json::to_string(envelope).expect("scan envelope serializes")
    );
    i32::from(!envelope.ok)
}

/// Print the hard-failure envelope as one compact JSON line (exit 1),
/// with no `bob ref:` stderr line.
fn print_scan_error_envelope(
    config: &Config,
    options: SyncOptions,
    mode: &'static str,
    intake: &[IntakeMove],
    completed_intake: usize,
    error: &CommandError,
) -> i32 {
    emit_scan_error_envelope(
        config,
        options,
        mode,
        intake_moves(config, &intake[..completed_intake.min(intake.len())]),
        error,
        false,
    )
}

fn emit_scan_error_envelope(
    config: &Config,
    options: SyncOptions,
    mode: &'static str,
    intake: Vec<ScanJsonIntakeMove>,
    error: &CommandError,
    intake_only: bool,
) -> i32 {
    let (code, message, hint, paths) = scan_error_parts(config, error);
    let envelope = ScanErrorEnvelope {
        ok: false,
        schema_version: REF_SCHEMA_VERSION,
        command: SCAN_COMMAND,
        generated_at: generated_at(),
        mode,
        write_pdfs: options.write_pdf,
        intake,
        error: ScanErrorBody {
            code,
            message,
            hint,
            paths,
        },
        intake_only,
        queued: Vec::new(),
    };
    println!(
        "{}",
        serde_json::to_string(&envelope)
            .expect("scan error envelope serializes")
    );
    1
}

/// Vault-relative PDF intake moves, in move order. Standalone audio moves
/// still execute on disk but are excluded from the JSON contract.
pub(super) fn intake_moves(
    config: &Config,
    intake: &[IntakeMove],
) -> Vec<ScanJsonIntakeMove> {
    intake
        .iter()
        .filter(|intake_move| is_pdf_path(&intake_move.destination))
        .map(|intake_move| ScanJsonIntakeMove {
            from: scan_vault_relative(config, &intake_move.source),
            to: scan_vault_relative(config, &intake_move.destination),
        })
        .collect()
}

/// Split a hard failure into its coded envelope parts.
fn scan_error_parts(
    config: &Config,
    error: &CommandError,
) -> (String, String, Option<String>, Vec<String>) {
    let code = error.code.unwrap_or("scan_failed").to_string();
    let paths = error
        .paths
        .iter()
        .map(|path| scan_vault_relative(config, path))
        .collect::<Vec<_>>();
    let (message, hint) = match error.code.unwrap_or("scan_failed") {
        "lock_busy" => (
            error.message.clone(),
            Some(
                "vault-sync, nightly, or rename held the maintenance lock; wait for it to finish, then scan again"
                    .to_string(),
            ),
        ),
        "scan_busy" => (
            "another bob ref scan is still running".to_string(),
            Some("wait for it to finish, then scan again".to_string()),
        ),
        "pre_sync_failed" => (
            error.message.clone(),
            Some(
                "the vault could not pull from or push to its remote; fix the sync problem (see bob vault-sync), then rerun"
                    .to_string(),
            ),
        ),
        "dirty_targets" => (
            "refusing to modify dirty vault files".to_string(),
            Some(
                "commit, stash, or clean those paths, then scan again"
                    .to_string(),
            ),
        ),
        "intake_collision" => (
            error.message.clone(),
            Some(
                "remove or rename the existing library destination(s) before rerunning scan"
                    .to_string(),
            ),
        ),
        _ => (error.message.clone(), None),
    };
    (code, message, hint, paths)
}

/// Run `scan --intake-only --format json`: print exactly one envelope line
/// and return the exit code. The envelope is the normal scan envelope with
/// `hook.status: "skipped"`, `intake` listing completed PDF moves, `notes`
/// listing filed notes, and `queued` listing every item still waiting in
/// `xlib/` (or, on a dry run, every item that would stay queued). On a
/// writing drain, `ok` is false (exit 1) while anything stays queued or
/// stopped mid-ingest. A dry run ignores `queued` for `ok`, matching
/// human mode.
pub(super) fn scan_intake_only_json(config: &Config, dry_run: bool) -> i32 {
    let mode = if dry_run { "dry_run" } else { "write" };
    let options = SyncOptions {
        dry_run,
        write_pdf: false,
        prefer: None,
    };
    let mut progress = |line: &str| eprintln!("{line}");
    let run =
        intake::run_intake_only(config, dry_run, &mut progress, &mut |_| {});
    let run = match run {
        Ok(run) => run,
        Err(error) => {
            return emit_scan_error_envelope(
                config,
                options,
                mode,
                Vec::new(),
                &error,
                true,
            );
        }
    };

    let mut intake = Vec::new();
    let mut notes = Vec::new();
    let mut failures = Vec::new();
    let mut queued = Vec::new();
    let mut reports = Vec::new();
    let mut pdfs = 0usize;
    match run {
        intake::IntakeOnlyRun::Empty => {}
        intake::IntakeOnlyRun::Preview(previews) => {
            pdfs = previews.len();
            for preview in previews {
                match preview {
                    intake::QueuePreview::Would { unit, .. } => {
                        if let Some(entry) = pdf_move_entry(config, &unit) {
                            intake.push(entry);
                        }
                    }
                    intake::QueuePreview::WouldQueue {
                        unit,
                        reason,
                        detail,
                    } => {
                        queued.push(queued_entry(config, &unit, reason, detail))
                    }
                }
            }
        }
        intake::IntakeOnlyRun::Drained(report) => {
            pdfs = report.items.len();
            for item in report.items {
                match item {
                    intake::DrainItem::Filed { ingested, .. } => {
                        intake.extend(
                            ingested
                                .moved
                                .iter()
                                .filter(|(_, to)| is_pdf_path(to))
                                .map(|(from, to)| ScanJsonIntakeMove {
                                    from: scan_vault_relative(config, from),
                                    to: scan_vault_relative(config, to),
                                }),
                        );
                        if ingested.write.note_action != "none" {
                            notes.push(ScanJsonNote {
                                action: ingested.write.note_action,
                                path: scan_vault_relative(
                                    config,
                                    &ingested.note,
                                ),
                                title: ingested.title.clone(),
                                ref_type: ingested.ref_type.clone(),
                                source_pdf: ingested.source_pdf.clone(),
                                marker: ingested.write.marker_action != "none",
                                image_notes: ingested.write.image_note_count,
                                image_notes_unresolved: ingested
                                    .write
                                    .image_notes_unresolved
                                    .iter()
                                    .map(|note| ScanJsonUnresolvedImageNote {
                                        source: note.source_text.clone(),
                                        state: note.state,
                                    })
                                    .collect(),
                                continuations: ingested
                                    .write
                                    .continuation_count,
                                continuations_dangling: ingested
                                    .write
                                    .dangling_continuations
                                    .len(),
                            });
                        }
                        reports.push(ingested.write.clone());
                    }
                    intake::DrainItem::Queued {
                        unit,
                        reason,
                        detail,
                    } => {
                        queued.push(queued_entry(config, &unit, reason, detail))
                    }
                    intake::DrainItem::Incomplete { unit, error } => {
                        // The PDF already moved into lib/ before the stop.
                        if let Some(entry) = pdf_move_entry(config, &unit) {
                            intake.push(entry);
                        }
                        failures.push(ScanJsonFailure {
                            pdf: scan_vault_relative(
                                config,
                                unit.library_pdf(),
                            ),
                            stage: "write",
                            message: error,
                        });
                    }
                }
            }
        }
    }

    let counts = ScanCounts::from_reports(&reports);
    let ok = failures.is_empty() && (dry_run || queued.is_empty());
    emit_scan_report_envelope(&ScanReportEnvelope {
        ok,
        schema_version: REF_SCHEMA_VERSION,
        command: SCAN_COMMAND,
        generated_at: generated_at(),
        mode,
        write_pdfs: false,
        hook: ScanJsonHook {
            status: "skipped",
            command: None,
        },
        intake,
        summary: ScanJsonSummary {
            pdfs,
            created: counts.creates,
            updated: counts.updates,
            unchanged: counts.unchanged,
            markers: counts.marker_updates,
            tasks: counts.annotation_tasks_created,
            failures: failures.len(),
        },
        notes,
        failures,
        intake_only: true,
        queued,
    })
}

/// The xlib→lib move of a PDF unit; audio units move no PDF.
fn pdf_move_entry(
    config: &Config,
    unit: &intake::IntakeUnit,
) -> Option<ScanJsonIntakeMove> {
    match unit {
        intake::IntakeUnit::Pdf {
            source,
            destination,
        } => Some(ScanJsonIntakeMove {
            from: scan_vault_relative(config, source),
            to: scan_vault_relative(config, destination),
        }),
        _ => None,
    }
}

fn queued_entry(
    config: &Config,
    unit: &intake::IntakeUnit,
    reason: intake::QueueReason,
    detail: String,
) -> ScanJsonQueued {
    ScanJsonQueued {
        pdf: unit
            .display_path(config)
            .to_string_lossy()
            .replace('\\', "/"),
        reason: reason.as_str(),
        message: detail,
    }
}
