//! Versioned JSON report for `bob ref scan --format json`.
//!
//! Human mode stays byte-identical (see `sync.rs`); this module mirrors
//! its stage order but collects a machine-readable envelope instead of
//! printing display lines. Stdout carries exactly one compact JSON line;
//! hook chatter goes to stderr, and hard failures print a coded error
//! envelope with exit 1 and no `bob ref:` stderr line.
use super::sections::SectionDiagnosticKind;
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
pub(super) struct ScanJsonHook {
    status: &'static str,
    command: Option<String>,
}

/// One PDF intake move: vault-relative `from` and `to`, in move order.
/// Sidecar and audio companion moves are not listed.
#[derive(Debug, Clone, Serialize)]
pub(super) struct ScanJsonIntakeMove {
    pub(super) from: String,
    pub(super) to: String,
}

/// The `summary` object: the same counts the human summary line prints.
/// `held` is always present and comes last.
#[derive(Debug, Clone, Serialize)]
pub(super) struct ScanJsonSummary {
    pub(super) pdfs: usize,
    pub(super) created: usize,
    pub(super) updated: usize,
    pub(super) unchanged: usize,
    pub(super) markers: usize,
    pub(super) tasks: usize,
    pub(super) failures: usize,
    pub(super) held: usize,
}

/// One created or updated reference note, in scan order.
#[derive(Debug, Clone, Serialize)]
pub(super) struct ScanJsonNote {
    action: &'static str,
    path: String,
    title: String,
    ref_type: Option<String>,
    source_pdf: String,
    marker: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    marker_write: Option<ScanJsonMarkerWrite>,
    #[serde(skip_serializing_if = "is_zero_usize")]
    image_notes: usize,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    image_notes_unresolved: Vec<ScanJsonUnresolvedImageNote>,
    #[serde(skip_serializing_if = "is_zero_usize")]
    continuations: usize,
    #[serde(skip_serializing_if = "is_zero_usize")]
    continuations_dangling: usize,
    #[serde(skip_serializing_if = "is_zero_usize")]
    underlines_merged: usize,
    #[serde(skip_serializing_if = "is_zero_usize")]
    underlines_standalone: usize,
    #[serde(skip_serializing_if = "is_zero_usize")]
    sections: usize,
    #[serde(skip_serializing_if = "is_zero_usize")]
    section_tags_unknown: usize,
}

/// One unresolved image note on a scanned reference.
#[derive(Debug, Clone, Serialize)]
pub(super) struct ScanJsonUnresolvedImageNote {
    source: String,
    state: UnresolvedImageNoteState,
}

fn is_zero_usize(value: &usize) -> bool {
    *value == 0
}

fn is_false(value: &bool) -> bool {
    !*value
}

/// One per-PDF failure: the same text the human report prints.
#[derive(Debug, Clone, Serialize)]
pub(super) struct ScanJsonFailure {
    pdf: String,
    stage: &'static str,
    message: String,
}

/// One `--intake-only` item that is still queued in `xlib/` (or would be
/// on a dry run): vault-relative PDF or audio path, a stable
/// [`QueueReason`] slug, and the human detail.
#[derive(Debug, Clone, Serialize)]
pub(super) struct ScanJsonQueued {
    pdf: String,
    reason: &'static str,
    message: String,
}

/// One marker write's machine-readable facts, right after `marker`.
/// Present exactly when a marker was written in write mode; `status` is
/// present only when the marker's `status` line changed.
#[derive(Debug, Clone, Serialize)]
pub(super) struct ScanJsonMarkerWrite {
    mode: &'static str,
    editor_saved_first: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    status: Option<ScanJsonStatusChange>,
    #[serde(skip_serializing_if = "is_false")]
    restored: bool,
}

/// A marker `status` line change (`ready|next|wip|read|abandoned` values).
#[derive(Debug, Clone, Serialize)]
pub(super) struct ScanJsonStatusChange {
    from: String,
    to: String,
}

/// One held PDF: nothing was written for it and the next scan retries.
/// Always the last field of the success envelope, `[]` for dry runs.
#[derive(Debug, Clone, Serialize)]
pub(super) struct ScanJsonHold {
    pdf: String,
    note: String,
    title: String,
    reason: &'static str,
    message: String,
}

/// Success and partial-failure envelope. Field order is the contract.
#[derive(Debug, Clone, Serialize)]
pub(super) struct ScanReportEnvelope {
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
    held: Vec<ScanJsonHold>,
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
    attended: bool,
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
            false,
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
                false,
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
                false,
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
                    true,
                );
            }
        }
    };
    let hook = scan_hook_for_envelope(
        no_hooks,
        options.dry_run,
        resolved_hook.as_ref(),
    );

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
            true,
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
                true,
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
                true,
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
                true,
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
            true,
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
            true,
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
            true,
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
                        None,
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
            Vec::new(),
            false,
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
            true,
        );
    }
    let mut session = match MarkerWriteSession::for_scan(attended) {
        Ok(session) => session,
        Err(error) => {
            return print_scan_error_envelope(
                config,
                options,
                mode,
                &intake,
                completed_intake,
                &error,
                true,
            );
        }
    };
    let mut run_state = RunWriteState::default();
    let mut write_outcomes = Vec::new();
    for plan in &plans {
        match execute_pdf_sync(config, plan, &mut run_state, &mut session) {
            Ok(PdfSyncOutcome::Written(report)) => {
                write_outcomes.push(ScanWriteOutcome::Written(report))
            }
            Ok(PdfSyncOutcome::Held(hold)) => {
                write_outcomes.push(ScanWriteOutcome::Held(hold))
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
            ScanWriteOutcome::Held(_) | ScanWriteOutcome::Failed(_) => None,
        })
        .collect::<Vec<_>>();
    let mut counts = ScanCounts::from_reports(&reports);
    counts.held = held_count(&write_outcomes);
    let assembled =
        assemble_full_scan_entries(config, &plan_outcomes, &write_outcomes);
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
        assembled.notes,
        assembled.failures,
        assembled.held,
        true,
    )
}

/// One marker write's JSON facts, or `None` when no marker was written
/// in write mode (dry runs never carry `marker_write`).
pub(super) fn marker_write_json(
    report: &SyncWriteReport,
) -> Option<ScanJsonMarkerWrite> {
    let info = report.marker_write.as_ref()?;
    if report.marker_action == "none" {
        return None;
    }
    Some(ScanJsonMarkerWrite {
        mode: match info.mode {
            MarkerWriteMode::Coordinated => "coordinated",
            MarkerWriteMode::Direct => "direct",
        },
        editor_saved_first: info.editor_saved_first,
        status: info
            .status_change
            .clone()
            .map(|(from, to)| ScanJsonStatusChange { from, to }),
        restored: info.restored,
    })
}

/// One held PDF's JSON row: vault-relative paths plus the stable reason
/// slug and the human message.
pub(super) fn scan_hold_entry(
    config: &Config,
    hold: &ScanHold,
) -> ScanJsonHold {
    ScanJsonHold {
        pdf: scan_vault_relative(config, &hold.pdf),
        note: scan_vault_relative(config, &hold.note_path),
        title: hold.title.clone(),
        reason: hold.reason.as_str(),
        message: hold.message.clone(),
    }
}

/// One note entry: the path `ref list` reports, the title the scan
/// wrote, the plan's stable metadata, and the marker flag.
fn scan_note_entry(
    config: &Config,
    plan: &PdfSyncPlan,
    action: &'static str,
    marker: bool,
    marker_write: Option<ScanJsonMarkerWrite>,
) -> ScanJsonNote {
    let (
        image_notes,
        image_notes_unresolved,
        continuations,
        continuations_dangling,
        underlines_merged,
        underlines_standalone,
        sections,
        section_tags_unknown,
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
                rendered.underlines_merged,
                rendered.standalone_underlines.len(),
                rendered.sections.len(),
                rendered
                    .section_diagnostics
                    .iter()
                    .filter(|diagnostic| {
                        diagnostic.kind == SectionDiagnosticKind::TagUnknown
                    })
                    .count(),
            )
        })
        .unwrap_or((0, Vec::new(), 0, 0, 0, 0, 0, 0));
    ScanJsonNote {
        action,
        path: scan_vault_relative(config, &plan.note_path),
        title: note_title(&plan.pdf, &plan.synced_projection),
        ref_type: plan.stable_metadata.ref_type.clone(),
        source_pdf: plan.stable_metadata.source_pdf.clone(),
        marker,
        marker_write,
        image_notes,
        image_notes_unresolved,
        continuations,
        continuations_dangling,
        underlines_merged,
        underlines_standalone,
        sections,
        section_tags_unknown,
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
        ScanLine::Success { .. } | ScanLine::Held { .. } => {
            failure.error.to_string()
        }
    };
    ScanJsonFailure {
        pdf: scan_vault_relative(config, &failure.pdf),
        stage,
        message,
    }
}

/// Notes, failures, and holds assembled from one full scan, in scan
/// order. Shared by JSON mode (stdout) and human mode (receipt) so both
/// envelopes carry the same entries.
pub(super) struct AssembledScanEntries {
    pub(super) notes: Vec<ScanJsonNote>,
    pub(super) failures: Vec<ScanJsonFailure>,
    pub(super) held: Vec<ScanJsonHold>,
}

/// Assemble one full scan's JSON entries from its plan and write outcomes.
/// Held PDFs are neither notes nor failures.
pub(super) fn assemble_full_scan_entries(
    config: &Config,
    plan_outcomes: &[ScanPlanOutcome],
    write_outcomes: &[ScanWriteOutcome],
) -> AssembledScanEntries {
    let mut notes = Vec::new();
    let mut failures = Vec::new();
    let mut held = Vec::new();
    let mut write_index = 0usize;
    for outcome in plan_outcomes {
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
                            marker_write_json(report),
                        ));
                    }
                    Some(ScanWriteOutcome::Failed(failure)) => {
                        failures
                            .push(scan_failure_entry(config, failure, "write"));
                    }
                    Some(ScanWriteOutcome::Held(hold)) => {
                        held.push(scan_hold_entry(config, hold));
                    }
                    Some(ScanWriteOutcome::Written(_)) | None => {}
                }
            }
            ScanPlanOutcome::Failed(failure) => {
                failures.push(scan_failure_entry(config, failure, "plan"));
            }
        }
    }
    AssembledScanEntries {
        notes,
        failures,
        held,
    }
}

/// The `hook` object for an envelope, from the resolved hook (or a peek at
/// the configured hook when hooks are skipped). Write mode reports `ran`;
/// dry runs report `would_run`.
pub(super) fn scan_hook_for_envelope(
    no_hooks: bool,
    dry_run: bool,
    resolved: Option<&PreScanHook>,
) -> ScanJsonHook {
    if no_hooks {
        let peeked = configured_pre_scan_hook(false)
            .ok()
            .flatten()
            .map(|hook| hook.display());
        return ScanJsonHook {
            status: "skipped",
            command: peeked,
        };
    }
    match resolved {
        Some(hook) => ScanJsonHook {
            status: if dry_run { "would_run" } else { "ran" },
            command: Some(hook.display()),
        },
        None => ScanJsonHook {
            status: if bob_env::var_os(ENV_PRE_SCAN_HOOK)
                .is_some_and(|value| value.to_string_lossy().trim().is_empty())
            {
                "skipped"
            } else {
                "none"
            },
            command: None,
        },
    }
}

/// The `summary` object: the same counts the human summary line prints,
/// with `held` last.
pub(super) fn scan_summary_json(
    counts: &ScanCounts,
    pdf_count: usize,
    failure_count: usize,
) -> ScanJsonSummary {
    ScanJsonSummary {
        pdfs: pdf_count,
        created: counts.creates,
        updated: counts.updates,
        unchanged: counts.unchanged,
        markers: counts.marker_updates,
        tasks: counts.annotation_tasks_created,
        failures: failure_count,
        held: counts.held,
    }
}

/// Render the success/partial envelope line shared by JSON stdout and both
/// modes' receipts. Field order is the contract.
#[allow(clippy::too_many_arguments)]
pub(super) fn render_scan_report_envelope(
    ok: bool,
    mode: &'static str,
    write_pdfs: bool,
    hook: ScanJsonHook,
    intake: Vec<ScanJsonIntakeMove>,
    summary: ScanJsonSummary,
    notes: Vec<ScanJsonNote>,
    failures: Vec<ScanJsonFailure>,
    intake_only: bool,
    queued: Vec<ScanJsonQueued>,
    held: Vec<ScanJsonHold>,
) -> String {
    let envelope = ScanReportEnvelope {
        ok,
        schema_version: REF_SCHEMA_VERSION,
        command: SCAN_COMMAND,
        generated_at: generated_at(),
        mode,
        write_pdfs,
        hook,
        intake,
        summary,
        notes,
        failures,
        intake_only,
        queued,
        held,
    };
    serde_json::to_string(&envelope).expect("scan envelope serializes")
}

/// Print the success/partial envelope as one compact JSON line and
/// return its exit code: 0 exactly when `failures` is empty. A writing
/// scan that holds the writer lock also writes that exact line as the
/// scan receipt.
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
    held: Vec<ScanJsonHold>,
    write_receipt: bool,
) -> i32 {
    let ok = failures.is_empty();
    let line = render_scan_report_envelope(
        ok,
        mode,
        options.write_pdf,
        hook,
        intake_moves(config, intake),
        scan_summary_json(&counts, pdf_count, failure_count),
        notes,
        failures,
        false,
        Vec::new(),
        held,
    );
    println!("{line}");
    if write_receipt {
        emit_scan_receipt(&line);
    }
    i32::from(!ok)
}

/// Print the hard-failure envelope as one compact JSON line (exit 1),
/// with no `bob ref:` stderr line. A writing scan that holds the writer
/// lock also writes that exact line as the scan receipt.
fn print_scan_error_envelope(
    config: &Config,
    options: SyncOptions,
    mode: &'static str,
    intake: &[IntakeMove],
    completed_intake: usize,
    error: &CommandError,
    write_receipt: bool,
) -> i32 {
    emit_scan_error_envelope(
        config,
        options,
        mode,
        intake_moves(config, &intake[..completed_intake.min(intake.len())]),
        error,
        false,
        write_receipt,
    )
}

fn emit_scan_error_envelope(
    config: &Config,
    options: SyncOptions,
    mode: &'static str,
    intake: Vec<ScanJsonIntakeMove>,
    error: &CommandError,
    intake_only: bool,
    write_receipt: bool,
) -> i32 {
    let line = render_scan_error_envelope(
        config,
        options,
        mode,
        intake,
        error,
        intake_only,
    );
    println!("{line}");
    if write_receipt {
        emit_scan_receipt(&line);
    }
    1
}

/// Render the hard-failure envelope line shared by JSON stdout and both
/// modes' receipts. The error envelope has no `held` field.
pub(super) fn render_scan_error_envelope(
    config: &Config,
    options: SyncOptions,
    mode: &'static str,
    intake: Vec<ScanJsonIntakeMove>,
    error: &CommandError,
    intake_only: bool,
) -> String {
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
    serde_json::to_string(&envelope).expect("scan error envelope serializes")
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
    let outcome = intake::run_intake_only_tracked(
        config,
        dry_run,
        &mut progress,
        &mut |_| {},
    );
    let run = match outcome.run {
        Ok(run) => run,
        Err(error) => {
            return emit_scan_error_envelope(
                config,
                options,
                mode,
                Vec::new(),
                &error,
                true,
                outcome.lock_held,
            );
        }
    };

    // A writing drain holds the scan writer lock; dry runs, empty queues,
    // and lock-busy scans never do.
    let write_receipt =
        !dry_run && matches!(run, intake::IntakeOnlyRun::Drained(_));
    let (line, code) = render_intake_only_envelope(config, mode, dry_run, &run);
    println!("{line}");
    if write_receipt {
        emit_scan_receipt(&line);
    }
    code
}

/// Render the `--intake-only` envelope line shared by JSON stdout and both
/// modes' receipts: filed notes in `notes`, completed PDF moves in
/// `intake`, and anything still waiting in `queued`. `held` is always `[]`.
pub(super) fn render_intake_only_envelope(
    config: &Config,
    mode: &'static str,
    dry_run: bool,
    run: &intake::IntakeOnlyRun,
) -> (String, i32) {
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
                        if let Some(entry) = pdf_move_entry(config, unit) {
                            intake.push(entry);
                        }
                    }
                    intake::QueuePreview::WouldQueue {
                        unit,
                        reason,
                        detail,
                    } => queued.push(queued_entry(
                        config,
                        unit,
                        *reason,
                        detail.clone(),
                    )),
                }
            }
        }
        intake::IntakeOnlyRun::Drained(report) => {
            pdfs = report.items.len();
            for item in &report.items {
                match item {
                    intake::DrainItem::Filed { unit: _, ingested } => {
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
                                marker_write: marker_write_json(
                                    &ingested.write,
                                ),
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
                                underlines_merged: ingested
                                    .write
                                    .underlines_merged,
                                underlines_standalone: ingested
                                    .write
                                    .standalone_underlines
                                    .len(),
                                sections: ingested.write.section_count,
                                section_tags_unknown: ingested
                                    .write
                                    .section_diagnostics
                                    .iter()
                                    .filter(|diagnostic| {
                                        diagnostic.kind
                                            == SectionDiagnosticKind::TagUnknown
                                    })
                                    .count(),
                            });
                        }
                        reports.push(ingested.write.clone());
                    }
                    intake::DrainItem::Queued {
                        unit,
                        reason,
                        detail,
                    } => queued.push(queued_entry(
                        config,
                        unit,
                        *reason,
                        detail.clone(),
                    )),
                    intake::DrainItem::Incomplete { unit, error } => {
                        // The PDF already moved into lib/ before the stop.
                        if let Some(entry) = pdf_move_entry(config, unit) {
                            intake.push(entry);
                        }
                        failures.push(ScanJsonFailure {
                            pdf: scan_vault_relative(
                                config,
                                unit.library_pdf(),
                            ),
                            stage: "write",
                            message: error.clone(),
                        });
                    }
                }
            }
        }
    }

    let counts = ScanCounts::from_reports(&reports);
    let ok = failures.is_empty() && (dry_run || queued.is_empty());
    let line = render_scan_report_envelope(
        ok,
        mode,
        false,
        ScanJsonHook {
            status: "skipped",
            command: None,
        },
        intake,
        scan_summary_json(&counts, pdfs, failures.len()),
        notes,
        failures,
        true,
        queued,
        Vec::new(),
    );
    (line, i32::from(!ok))
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

#[cfg(test)]
mod tests {
    use super::*;

    fn note(sections: usize, section_tags_unknown: usize) -> ScanJsonNote {
        ScanJsonNote {
            action: "update",
            path: "ref/paper.md".to_string(),
            title: "Paper".to_string(),
            ref_type: None,
            source_pdf: "paper.pdf".to_string(),
            marker: false,
            marker_write: None,
            image_notes: 0,
            image_notes_unresolved: Vec::new(),
            continuations: 0,
            continuations_dangling: 0,
            underlines_merged: 0,
            underlines_standalone: 0,
            sections,
            section_tags_unknown,
        }
    }

    #[test]
    fn section_fields_are_omitted_when_zero() {
        let value = serde_json::to_value(note(0, 0)).expect("note serializes");
        assert!(!value
            .as_object()
            .expect("note is object")
            .contains_key("sections"));
        assert!(!value
            .as_object()
            .expect("note is object")
            .contains_key("section_tags_unknown"));
    }

    #[test]
    fn section_fields_are_present_when_nonzero() {
        let value = serde_json::to_value(note(2, 1)).expect("note serializes");
        let object = value.as_object().expect("note is object");
        assert_eq!(
            object.get("sections").and_then(|value| value.as_u64()),
            Some(2)
        );
        assert_eq!(
            object
                .get("section_tags_unknown")
                .and_then(|value| value.as_u64()),
            Some(1)
        );
    }
}
