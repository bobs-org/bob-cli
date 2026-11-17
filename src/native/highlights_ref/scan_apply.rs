//! Bounded mutation window for a prepared full scan.
//!
//! Human and JSON full scans share this protocol: recover (if needed) under
//! both locks then release; run the hook and plan the existing library
//! with neither lock held; return lock-free when there is no intake or
//! actual write; otherwise acquire `bob_sync.lock` then `scan.lock`,
//! validate, intake, and write inside that window. Intake-only and
//! create-time ingest keep their own lock protocol.
use super::*;

/// How the pre-scan hook should print.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum HookMode {
    Human,
    Json,
}

/// Successful full-scan apply: planning plus, in write mode, assembled
/// write outcomes. `acquired_writer_lock` is the receipt-ownership bit.
pub(super) struct AppliedFullScan {
    pub prepared: PreparedFullScan,
    /// `None` on a dry run (callers report from plan outcomes).
    pub write_outcomes: Option<Vec<ScanWriteOutcome>>,
    pub hook: Option<PreScanHook>,
    pub acquired_writer_lock: bool,
    /// Exact JSON envelope line written as the receipt while the writer
    /// lock was held. Presenters must print this line in JSON mode so
    /// stdout and the receipt stay byte-identical.
    pub receipt_line: Option<String>,
}

/// Failed full-scan apply, with enough context to render the existing
/// error envelope and (when the writer lock was held) its receipt.
pub(super) struct ApplyError {
    pub error: CommandError,
    pub intake: Vec<IntakeMove>,
    pub completed_intake: usize,
    pub acquired_writer_lock: bool,
    pub receipt_line: Option<String>,
}

impl ApplyError {
    fn prelock(error: CommandError) -> Self {
        Self {
            error,
            intake: Vec::new(),
            completed_intake: 0,
            acquired_writer_lock: false,
            receipt_line: None,
        }
    }
}

/// Writer-lock pair: `scan.lock` is declared first so it drops before
/// `bob_sync.lock`, reversing the acquire order.
struct MutationLocks {
    _scan: Option<fs::File>,
    _maintenance: Option<fs::File>,
}

impl MutationLocks {
    fn held(scan: Option<fs::File>, maintenance: Option<fs::File>) -> Self {
        scan_test::enter_mutation_window();
        Self {
            _scan: scan,
            _maintenance: maintenance,
        }
    }
}

impl Drop for MutationLocks {
    fn drop(&mut self) {
        scan_test::leave_mutation_window();
    }
}

fn acquire_mutation_locks() -> Result<MutationLocks> {
    scan_test::pause("before_lock");
    let maintenance = acquire_scan_maintenance_lock(false)?;
    let scan = acquire_scan_writer_lock(false)?;
    Ok(MutationLocks::held(scan, maintenance))
}

/// Shared full-scan lifecycle for human and JSON entry points.
pub(super) fn apply_full_scan(
    config: &Config,
    options: SyncOptions,
    jobs: usize,
    no_hooks: bool,
    attended: bool,
    hook_mode: HookMode,
) -> std::result::Result<AppliedFullScan, ApplyError> {
    validate_library_layout(config).map_err(ApplyError::prelock)?;
    refuse_nested_writing_scan(options.dry_run).map_err(ApplyError::prelock)?;

    recover_pending_overwrite(config, options)?;

    let hook =
        configured_pre_scan_hook(no_hooks).map_err(ApplyError::prelock)?;
    run_hook(config, hook.as_ref(), options.dry_run, hook_mode)
        .map_err(ApplyError::prelock)?;

    let prepared = prepare_full_scan_with_retry(config, options, jobs)
        .map_err(ApplyError::prelock)?;
    scan_test::pause("after_prepare");

    if options.dry_run {
        return Ok(AppliedFullScan {
            prepared,
            write_outcomes: None,
            hook,
            acquired_writer_lock: false,
            receipt_line: None,
        });
    }

    if !prepared_scan_needs_mutation(&prepared) {
        return Ok(no_work_applied(prepared, hook));
    }

    lock_validate_intake_and_execute(
        config, options, jobs, no_hooks, attended, hook, prepared,
    )
}

fn no_work_applied(
    prepared: PreparedFullScan,
    hook: Option<PreScanHook>,
) -> AppliedFullScan {
    let write_outcomes = unchanged_write_outcomes_for(&prepared.plan_outcomes);
    AppliedFullScan {
        prepared,
        write_outcomes: Some(write_outcomes),
        hook,
        acquired_writer_lock: false,
        receipt_line: None,
    }
}

fn refuse_nested_writing_scan(dry_run: bool) -> Result<()> {
    if dry_run {
        return Ok(());
    }
    if bob_env::var_os(ENV_IN_PRE_SCAN_HOOK).is_some() {
        return Err(CommandError::new(
            "could not acquire the vault maintenance lock: nested writing scan from a pre-scan hook",
        )
        .with_code("lock_busy"));
    }
    Ok(())
}

/// Recover a pending overwrite under both locks, then release before the
/// hook and ordinary preparation. Dry runs only report. A recovery error
/// stops before the hook; writing recovery that mutated settles a receipt
/// while the locks are still held.
fn recover_pending_overwrite(
    config: &Config,
    options: SyncOptions,
) -> std::result::Result<(), ApplyError> {
    match overwrite::recovery::load_intent(config)
        .map_err(ApplyError::prelock)?
    {
        None => Ok(()),
        Some(_) if options.dry_run => {
            overwrite::recovery::recover_with_locks_held(
                config,
                &ob::child_env(),
                true,
            )
            .map(|_| ())
            .map_err(ApplyError::prelock)
        }
        Some(_) => {
            let locks =
                acquire_mutation_locks().map_err(ApplyError::prelock)?;
            match overwrite::recovery::recover_with_locks_held(
                config,
                &ob::child_env(),
                false,
            ) {
                Ok(notice) => {
                    if let Some(notice) = notice {
                        eprintln!("{notice}");
                        emit_recovery_receipt(config, options);
                    }
                    drop(locks);
                    Ok(())
                }
                Err(error) => Err(finish_locked_error(
                    config,
                    options,
                    error,
                    Vec::new(),
                    0,
                    locks,
                )),
            }
        }
    }
}

fn emit_recovery_receipt(config: &Config, options: SyncOptions) {
    let pdfs = collect_pdf_paths(config, false).unwrap_or_default();
    let mut counts = ScanCounts::default();
    counts.unchanged = pdfs.len();
    emit_scan_receipt(&render_scan_report_envelope(
        true,
        "write",
        options.write_pdf,
        scan_hook_for_envelope(true, false, None),
        Vec::new(),
        scan_summary_json(&counts, pdfs.len(), 0),
        Vec::new(),
        Vec::new(),
        Vec::new(),
        false,
        Vec::new(),
        Vec::new(),
    ));
}

fn run_hook(
    config: &Config,
    hook: Option<&PreScanHook>,
    dry_run: bool,
    mode: HookMode,
) -> Result<()> {
    match mode {
        HookMode::Human => run_pre_scan_hook(config, hook, dry_run),
        HookMode::Json => run_pre_scan_hook_json(config, hook, dry_run),
    }
}

fn lock_validate_intake_and_execute(
    config: &Config,
    options: SyncOptions,
    jobs: usize,
    no_hooks: bool,
    attended: bool,
    hook: Option<PreScanHook>,
    mut prepared: PreparedFullScan,
) -> std::result::Result<AppliedFullScan, ApplyError> {
    let invocation_date = prepared.invocation_date.clone();
    let mut retried = false;
    loop {
        let locks = match acquire_mutation_locks() {
            Ok(locks) => locks,
            Err(error) => return Err(ApplyError::prelock(error)),
        };

        if let Err(error) = validate_library_layout(config) {
            return Err(finish_locked_error(
                config,
                options,
                error,
                Vec::new(),
                0,
                locks,
            ));
        }

        match overwrite::recovery::load_intent(config) {
            Err(error) => {
                return Err(finish_locked_error(
                    config,
                    options,
                    error,
                    prepared.intake.clone(),
                    0,
                    locks,
                ));
            }
            Ok(Some(_)) => {
                match overwrite::recovery::recover_with_locks_held(
                    config,
                    &ob::child_env(),
                    false,
                ) {
                    Ok(notice) => {
                        if let Some(notice) = notice {
                            eprintln!("{notice}");
                            // Settle recovery-only work while serialized:
                            // publish the receipt while both locks are still
                            // held so a no-work/error rebuild cannot leave
                            // real recovery mutations with a stale sentinel.
                            // A later execution overwrites this with the
                            // complete result; an abandoned retry never
                            // publishes on its own.
                            emit_recovery_receipt(config, options);
                        }
                    }
                    Err(error) => {
                        return Err(finish_locked_error(
                            config,
                            options,
                            error,
                            prepared.intake.clone(),
                            0,
                            locks,
                        ));
                    }
                }
                drop(locks);
                if retried {
                    return Err(ApplyError::prelock(
                        scan_inputs_changed_error(
                            "overwrite recovery invalidated the prepared scan",
                        ),
                    ));
                }
                retried = true;
                prepared =
                    rebuild_prepared(config, options, jobs, &invocation_date)
                        .map_err(ApplyError::prelock)?;
                if !prepared_scan_needs_mutation(&prepared) {
                    return Ok(no_work_applied(prepared, hook));
                }
                continue;
            }
            Ok(None) => {}
        }

        match validate_prepared_scan(&prepared) {
            PreparedValidation::Failed(error) => {
                return Err(finish_locked_error(
                    config,
                    options,
                    error,
                    prepared.intake.clone(),
                    0,
                    locks,
                ));
            }
            PreparedValidation::Stale { reason }
                if is_destination_asset_stale(&reason) =>
            {
                // Dest-asset vandalism cannot be restored: overwrite
                // protection refuses Copy onto existing different bytes, so
                // a rebuild would collide at write. Fail closed.
                return Err(finish_locked_error(
                    config,
                    options,
                    scan_inputs_changed_error(&reason),
                    prepared.intake.clone(),
                    0,
                    locks,
                ));
            }
            PreparedValidation::Stale { reason } => {
                drop(locks);
                if retried {
                    return Err(ApplyError::prelock(
                        scan_inputs_changed_error(&reason),
                    ));
                }
                retried = true;
                prepared =
                    rebuild_prepared(config, options, jobs, &invocation_date)
                        .map_err(ApplyError::prelock)?;
                if !prepared_scan_needs_mutation(&prepared) {
                    return Ok(no_work_applied(prepared, hook));
                }
                continue;
            }
            PreparedValidation::Unchanged => {}
        }

        scan_test::pause("before_intake");
        let intake = match plan_xlib_intake(config) {
            Ok(intake) => intake,
            Err(error) => {
                return Err(finish_locked_error(
                    config,
                    options,
                    error,
                    prepared.intake.clone(),
                    0,
                    locks,
                ));
            }
        };
        if intake != prepared.intake {
            drop(locks);
            if retried {
                return Err(ApplyError::prelock(scan_inputs_changed_error(
                    "intake membership changed",
                )));
            }
            retried = true;
            prepared =
                rebuild_prepared(config, options, jobs, &invocation_date)
                    .map_err(ApplyError::prelock)?;
            if !prepared_scan_needs_mutation(&prepared) {
                return Ok(no_work_applied(prepared, hook));
            }
            continue;
        }

        // Validate prospective note outputs for the existing library plus
        // all queued destination PDFs under the mutation locks, before the
        // first move. This restores the former collision guard for
        // intake-only initial libraries and for conflicts between existing
        // and new PDFs. Uses the existing guard helpers so code, paths, and
        // human/JSON behavior stay identical to dry-run.
        {
            let mut combined = prepared.pdfs.clone();
            combined.extend(intake_affected_pdfs(&intake));
            combined.sort();
            combined.dedup();
            if let Err(error) = validate_output_collisions(config, &combined) {
                return Err(finish_locked_error(
                    config,
                    options,
                    error,
                    intake.clone(),
                    0,
                    locks,
                ));
            }
        }

        let (moved, result) = execute_xlib_intake_counting(&intake);
        if let Err(error) = result {
            return Err(finish_locked_error(
                config, options, error, intake, moved, locks,
            ));
        }
        prepared.intake = intake.clone();
        scan_test::pause("after_intake_move");

        if let Err(error) =
            integrate_intake_plans(&mut prepared, &intake, options, jobs)
        {
            return Err(finish_locked_error(
                config,
                options,
                error,
                intake.clone(),
                intake.len(),
                locks,
            ));
        }

        // Recheck the combined final plans for output and asset collisions
        // before any plan executes. Intake planning alone cannot see
        // conflicts across the combined set (e.g., two queued PDFs mapping
        // to one ref note, or a queued/existing note collision). New retry
        // branches are forbidden after any owned move; failures use the
        // existing protected failure/cleanup semantics and never erase
        // intervening data.
        {
            if let Err(error) =
                validate_output_collisions(config, &prepared.pdfs)
            {
                return Err(finish_locked_error(
                    config,
                    options,
                    error,
                    intake.clone(),
                    intake.len(),
                    locks,
                ));
            }
            let combined_plans = prepared
                .plan_outcomes
                .iter()
                .filter_map(|outcome| match outcome {
                    ScanPlanOutcome::Planned(plan) => Some(plan.as_ref()),
                    ScanPlanOutcome::Held(_) | ScanPlanOutcome::Failed(_) => {
                        None
                    }
                })
                .collect::<Vec<_>>();
            if let Err(error) =
                validate_planned_asset_collisions(&combined_plans)
            {
                return Err(finish_locked_error(
                    config,
                    options,
                    error,
                    intake.clone(),
                    intake.len(),
                    locks,
                ));
            }
        }

        let executed = match execute_prepared_plans(
            config, options, attended, no_hooks, hook, prepared,
        ) {
            Ok(applied) => applied,
            Err(error) => {
                return Err(finish_locked_error(
                    config,
                    options,
                    error,
                    intake.clone(),
                    intake.len(),
                    locks,
                ));
            }
        };
        drop(locks);
        return Ok(executed);
    }
}

fn rebuild_prepared(
    config: &Config,
    options: SyncOptions,
    jobs: usize,
    date: &str,
) -> Result<PreparedFullScan> {
    prepare_full_scan_with_retry_date(
        config,
        options,
        jobs,
        Some(date.to_string()),
    )
}

fn execute_prepared_plans(
    config: &Config,
    options: SyncOptions,
    attended: bool,
    no_hooks: bool,
    hook: Option<PreScanHook>,
    prepared: PreparedFullScan,
) -> Result<AppliedFullScan> {
    let write_outcomes = {
        let plans = prepared
            .plan_outcomes
            .iter()
            .filter_map(|outcome| match outcome {
                ScanPlanOutcome::Planned(plan) => Some(plan.as_ref()),
                ScanPlanOutcome::Held(_) | ScanPlanOutcome::Failed(_) => None,
            })
            .collect::<Vec<_>>();
        let dirty_preflight =
            scan_dirty_preflight(config, plans.iter().copied())?;
        let mut session = MarkerWriteSession::for_scan(attended)?;
        let mut run_state = RunWriteState::default();
        let mut write_outcomes = Vec::new();
        for plan in &plans {
            match execute_scan_plan(
                config,
                plan,
                &dirty_preflight,
                &mut run_state,
                &mut session,
            ) {
                Ok(PdfSyncOutcome::Written(report)) => {
                    write_outcomes.push(ScanWriteOutcome::Written(report))
                }
                Ok(PdfSyncOutcome::Held(hold)) => {
                    write_outcomes.push(ScanWriteOutcome::Held(hold))
                }
                Err(error) => {
                    write_outcomes.push(ScanWriteOutcome::Failed(
                        ScanFailure::new(plan.pdf.clone(), error),
                    ));
                }
            }
        }
        write_outcomes
    };
    let receipt_line = write_scan_envelope_line(
        config,
        options,
        no_hooks,
        hook.as_ref(),
        &prepared,
        &write_outcomes,
    );
    emit_scan_receipt(&receipt_line);
    Ok(AppliedFullScan {
        prepared,
        write_outcomes: Some(write_outcomes),
        hook,
        acquired_writer_lock: true,
        receipt_line: Some(receipt_line),
    })
}

fn finish_locked_error(
    config: &Config,
    options: SyncOptions,
    error: CommandError,
    intake: Vec<IntakeMove>,
    completed_intake: usize,
    _locks: MutationLocks,
) -> ApplyError {
    let mut receipt = WriteScanReceipt::new(config, options);
    receipt.set_intake(intake.clone(), completed_intake);
    let line = receipt.write_error(&error);
    ApplyError {
        error,
        intake,
        completed_intake,
        acquired_writer_lock: true,
        receipt_line: Some(line),
    }
}
