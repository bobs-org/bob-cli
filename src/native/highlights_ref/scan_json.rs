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
}

/// One per-PDF failure: the same text the human report prints.
#[derive(Debug, Clone, Serialize)]
struct ScanJsonFailure {
    pdf: String,
    stage: &'static str,
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

/// Take the exclusive writer lock serializing writing scans
/// (`bob_cli_state_dir()/ref/scan.lock`). Dry runs never lock. On
/// contention the wait line goes to stderr once in both modes, then the
/// lock retries every 250 ms for up to 120 s (`BOB_REF_SCAN_LOCK_WAIT_SECONDS`
/// overrides, for tests) before failing with `scan_busy`. The returned
/// file holds the lock until the scan returns.
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

/// Bounded writer-lock wait, in seconds.
fn scan_lock_wait_seconds() -> f64 {
    bob_env::var(ENV_SCAN_LOCK_WAIT_SECONDS)
        .ok()
        .and_then(|value| value.parse::<f64>().ok())
        .filter(|value| value.is_finite() && *value >= 0.0)
        .unwrap_or(SCAN_LOCK_WAIT_SECONDS_DEFAULT)
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
            ScanWriteOutcome::Written(report) => Some(*report),
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
    ScanJsonNote {
        action,
        path: scan_vault_relative(config, &plan.note_path),
        title: note_title(&plan.pdf, &plan.synced_projection),
        ref_type: plan.stable_metadata.ref_type.clone(),
        source_pdf: plan.stable_metadata.source_pdf.clone(),
        marker,
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
    };
    println!(
        "{}",
        serde_json::to_string(&envelope).expect("scan envelope serializes")
    );
    if ok {
        0
    } else {
        1
    }
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
    let (code, message, hint, paths) = scan_error_parts(config, error);
    let envelope = ScanErrorEnvelope {
        ok: false,
        schema_version: REF_SCHEMA_VERSION,
        command: SCAN_COMMAND,
        generated_at: generated_at(),
        mode,
        write_pdfs: options.write_pdf,
        intake: intake_moves(
            config,
            &intake[..completed_intake.min(intake.len())],
        ),
        error: ScanErrorBody {
            code,
            message,
            hint,
            paths,
        },
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
        "scan_busy" => (
            "another bob ref scan is still running".to_string(),
            Some("wait for it to finish, then scan again".to_string()),
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
