//! PDF sync planning and execution.
use super::*;
use crate::native::parent_notes as parent_notes_mod;
use crate::native::ref_tasks as ref_tasks_mod;

pub(super) fn sync_pdf(
    config: &Config,
    pdf: &Path,
    options: SyncOptions,
) -> Result<()> {
    if let Some(hold) = replaced_capture_hold(config, pdf)? {
        print_sync_hold_report(&hold);
        return Ok(());
    }
    let plan = plan_single_pdf_sync(config, pdf, options)?;
    print_pdf_sync_report("sync", config, &plan, options);

    if options.dry_run {
        println!("note_action: {}", plan.stable_note_action);
        println!(
            "pdf_marker_action: {}",
            if plan.marker_write_needed {
                "would-update"
            } else {
                "none"
            }
        );
        println!("writes: none");
        return Ok(());
    }

    match write_single_pdf_sync(config, &plan)? {
        PdfSyncOutcome::Written(report) => print_sync_write_report(&report),
        PdfSyncOutcome::Held(hold) => print_sync_hold_report(&hold),
    }
    Ok(())
}

/// Build the finalized plan for one PDF without printing. Create-time
/// ingestion and the `sync` command share the same planning sequence.
pub(super) fn plan_single_pdf_sync(
    config: &Config,
    pdf: &Path,
    options: SyncOptions,
) -> Result<PdfSyncPlan> {
    let ctx = ScanContext::build(config);
    let mut plan = plan_pdf_sync(config, pdf, options, &ctx)?;
    let mut plans_to_finalize = [&mut plan];
    finalize_annotation_task_plans(config, &mut plans_to_finalize, &ctx)?;
    reserve_preview_ids(config, &mut plans_to_finalize, &ctx);
    Ok(plan)
}

/// Apply one finalized PDF plan with the same write guard as scan/sync.
pub(super) fn write_single_pdf_sync(
    config: &Config,
    plan: &PdfSyncPlan,
) -> Result<PdfSyncOutcome> {
    if let Some(hold) = replaced_capture_hold(config, &plan.pdf)? {
        return Ok(PdfSyncOutcome::Held(hold));
    }
    ensure_safe_to_write(config, iter::once(plan))?;
    let mut session = MarkerWriteSession::for_explicit_write()?;
    execute_pdf_sync(config, plan, &mut RunWriteState::default(), &mut session)
}

/// Apply one finalized PDF plan when the caller already owns the complete
/// per-PDF file-coordination scope.
///
/// Overwrite publishes the replacement PDF/note (and optional audio) under
/// its own preimage and dirty-path checks, then asks single-PDF sync to
/// fill pipeline fields. Those published files are already dirty versus
/// HEAD, so the ordinary scan dirty-target veto would refuse the rest of
/// the same transaction. Scan and `sync` still go through
/// [`write_single_pdf_sync`].
pub(super) fn write_single_pdf_sync_with_external_coordination(
    config: &Config,
    plan: &PdfSyncPlan,
) -> Result<PdfSyncOutcome> {
    let mut session = MarkerWriteSession::for_external_coordination();
    execute_pdf_sync(config, plan, &mut RunWriteState::default(), &mut session)
}

pub(super) fn scan_library(
    config: &Config,
    options: SyncOptions,
    jobs: usize,
    verbose: bool,
    no_hooks: bool,
    attended: bool,
) -> Result<()> {
    validate_library_layout(config)?;
    let _maintenance_lock = acquire_scan_maintenance_lock(options.dry_run)?;
    let _scan_lock = acquire_scan_writer_lock(options.dry_run)?;
    // Recover an interrupted overwrite under the held locks before any
    // pre-sync can commit half-applied files. Dry runs only report.
    if let Some(notice) = overwrite::recovery::recover_with_locks_held(
        config,
        &ob::child_env(),
        options.dry_run,
    )? {
        eprintln!("{notice}");
    }
    if options.dry_run {
        return scan_library_dry_run(
            config, options, jobs, verbose, no_hooks, attended,
        );
    }
    scan_library_write(config, options, jobs, verbose, no_hooks, attended)
}

/// Dry-run full scan: planning and human reporting only. Dry runs never
/// take the writer lock and never write the scan receipt.
fn scan_library_dry_run(
    config: &Config,
    options: SyncOptions,
    jobs: usize,
    verbose: bool,
    no_hooks: bool,
    attended: bool,
) -> Result<()> {
    let pre_scan_hook = configured_pre_scan_hook(no_hooks)?;
    run_pre_scan_hook(config, pre_scan_hook.as_ref(), options.dry_run)?;
    let intake = plan_xlib_intake(config)?;
    let pdfs = collect_pdf_paths(config, options.dry_run)?;
    validate_output_collisions(config, &pdfs)?;

    // One locator index for the whole scan, shared across scoped planning
    // threads (plus the invocation date). Planning never mutates it.
    let ctx = ScanContext::build(config);
    // `plan_pdf_sync` is a pure, read-only computation over an independent
    // `&Config`, one PDF path, and the shared context, so planning is
    // embarrassingly parallel. We collect into a position-keyed vector and
    // reassemble in `pdfs` order so reporting output stays deterministic
    // regardless of completion order.
    let mut plan_outcomes = plan_pdfs(config, &pdfs, options, jobs, &ctx);
    let mut plans_to_finalize = plan_outcomes
        .iter_mut()
        .filter_map(|outcome| match outcome {
            ScanPlanOutcome::Planned(plan) => Some(plan.as_mut()),
            ScanPlanOutcome::Held(_) | ScanPlanOutcome::Failed(_) => None,
        })
        .collect::<Vec<_>>();
    finalize_annotation_task_plans(config, &mut plans_to_finalize, &ctx)?;
    reserve_preview_ids(config, &mut plans_to_finalize, &ctx);
    let plans = plan_outcomes
        .iter()
        .filter_map(|outcome| match outcome {
            ScanPlanOutcome::Planned(plan) => Some(plan.as_ref()),
            ScanPlanOutcome::Held(_) | ScanPlanOutcome::Failed(_) => None,
        })
        .collect::<Vec<_>>();
    validate_planned_asset_collisions(&plans)?;
    let plan_failures = plan_outcomes
        .iter()
        .filter_map(|outcome| match outcome {
            ScanPlanOutcome::Planned(_) | ScanPlanOutcome::Held(_) => None,
            ScanPlanOutcome::Failed(failure) => Some(failure),
        })
        .collect::<Vec<_>>();

    let styler = Styler::detect();
    let session = MarkerWriteSession::for_scan(attended)?;
    if verbose {
        print_verbose_scan_plan_report(
            config,
            options,
            pdfs.len(),
            &intake,
            &plan_outcomes,
            &session,
            &plans,
        );
    } else {
        print_concise_intake_report(config, &intake, options.dry_run, &styler);
        print_scan_header(config, pdfs.len(), options.dry_run, &styler);
    }

    if verbose {
        print_scan_plan_summary(
            &plans,
            plan_failures.len(),
            plan_hold_count(&plan_outcomes),
        );
        println!("writes: none");
    } else {
        print_concise_scan_plan_report(
            &plan_outcomes,
            &plans,
            pdfs.len(),
            plan_failures.len(),
            &styler,
        );
    }
    if plan_failures.is_empty() {
        Ok(())
    } else {
        Err(scan_partial_failure_error(&plan_failures, &[]))
    }
}

/// Writing full scan: the writer lock is held, so every outcome — success,
/// partial failure, and hard failure after the lock — writes the same
/// envelope line JSON mode would print as the scan receipt.
fn scan_library_write(
    config: &Config,
    options: SyncOptions,
    jobs: usize,
    verbose: bool,
    no_hooks: bool,
    attended: bool,
) -> Result<()> {
    let mut receipt = WriteScanReceipt::new(config, options);
    let pre_scan_hook = match configured_pre_scan_hook(no_hooks) {
        Ok(hook) => hook,
        Err(error) => {
            receipt.write_error(&error);
            return Err(error);
        }
    };
    if let Err(error) =
        run_pre_scan_hook(config, pre_scan_hook.as_ref(), options.dry_run)
    {
        receipt.write_error(&error);
        return Err(error);
    }
    let intake = match plan_xlib_intake(config) {
        Ok(intake) => intake,
        Err(error) => {
            receipt.write_error(&error);
            return Err(error);
        }
    };
    let (moved, result) = execute_xlib_intake_counting(&intake);
    receipt.set_intake(intake.clone(), moved);
    if let Err(error) = result {
        receipt.write_error(&error);
        return Err(error);
    }
    receipt.set_intake(intake.clone(), intake.len());
    let pdfs = match collect_pdf_paths(config, options.dry_run) {
        Ok(pdfs) => pdfs,
        Err(error) => {
            receipt.write_error(&error);
            return Err(error);
        }
    };
    if let Err(error) = validate_output_collisions(config, &pdfs) {
        receipt.write_error(&error);
        return Err(error);
    }

    // One locator index for the whole scan, shared across scoped planning
    // threads (plus the invocation date). Planning never mutates it.
    let ctx = ScanContext::build(config);
    // `plan_pdf_sync` is a pure, read-only computation over an independent
    // `&Config`, one PDF path, and the shared context, so planning is
    // embarrassingly parallel. We collect into a position-keyed vector and
    // reassemble in `pdfs` order so reporting output stays deterministic
    // regardless of completion order.
    let mut plan_outcomes = plan_pdfs(config, &pdfs, options, jobs, &ctx);
    let mut plans_to_finalize = plan_outcomes
        .iter_mut()
        .filter_map(|outcome| match outcome {
            ScanPlanOutcome::Planned(plan) => Some(plan.as_mut()),
            ScanPlanOutcome::Held(_) | ScanPlanOutcome::Failed(_) => None,
        })
        .collect::<Vec<_>>();
    if let Err(error) =
        finalize_annotation_task_plans(config, &mut plans_to_finalize, &ctx)
    {
        receipt.write_error(&error);
        return Err(error);
    }
    reserve_preview_ids(config, &mut plans_to_finalize, &ctx);
    let plans = plan_outcomes
        .iter()
        .filter_map(|outcome| match outcome {
            ScanPlanOutcome::Planned(plan) => Some(plan.as_ref()),
            ScanPlanOutcome::Held(_) | ScanPlanOutcome::Failed(_) => None,
        })
        .collect::<Vec<_>>();
    if let Err(error) = validate_planned_asset_collisions(&plans) {
        receipt.write_error(&error);
        return Err(error);
    }
    let plan_failures = plan_outcomes
        .iter()
        .filter_map(|outcome| match outcome {
            ScanPlanOutcome::Planned(_) | ScanPlanOutcome::Held(_) => None,
            ScanPlanOutcome::Failed(failure) => Some(failure),
        })
        .collect::<Vec<_>>();

    let styler = Styler::detect();
    let mut session = match MarkerWriteSession::for_scan(attended) {
        Ok(session) => session,
        Err(error) => {
            receipt.write_error(&error);
            return Err(error);
        }
    };
    if verbose {
        print_verbose_scan_plan_report(
            config,
            options,
            pdfs.len(),
            &intake,
            &plan_outcomes,
            &session,
            &plans,
        );
    } else {
        print_concise_intake_report(config, &intake, options.dry_run, &styler);
        print_scan_header(config, pdfs.len(), options.dry_run, &styler);
    }

    let dirty_preflight =
        match scan_dirty_preflight(config, plans.iter().copied()) {
            Ok(preflight) => preflight,
            Err(error) => {
                receipt.write_error(&error);
                return Err(error);
            }
        };
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
    let reports = write_outcomes
        .iter()
        .filter_map(|outcome| match outcome {
            ScanWriteOutcome::Written(report) => Some(report.clone()),
            ScanWriteOutcome::Held(_) | ScanWriteOutcome::Failed(_) => None,
        })
        .collect::<Vec<_>>();
    let write_failures = write_outcomes
        .iter()
        .filter_map(|outcome| match outcome {
            ScanWriteOutcome::Failed(failure) => Some(failure),
            ScanWriteOutcome::Written(_) | ScanWriteOutcome::Held(_) => None,
        })
        .collect::<Vec<_>>();
    if verbose {
        print_verbose_scan_write_marker_lines(
            &plan_outcomes,
            &write_outcomes,
            &session,
        );
        for failure in &write_failures {
            print_scan_write_failure_entry(failure);
        }
        print_scan_write_summary(
            &reports,
            &plans,
            plan_failures.len(),
            write_failures.len(),
            held_count(&write_outcomes) + plan_hold_count(&plan_outcomes),
        );
    } else {
        print_concise_scan_write_report(
            &plan_outcomes,
            &write_outcomes,
            &reports,
            pdfs.len(),
            plan_failures.len(),
            write_failures.len(),
            &styler,
        );
    }
    let mut counts = ScanCounts::from_reports(&reports);
    counts.held = held_count(&write_outcomes) + plan_hold_count(&plan_outcomes);
    let assembled =
        assemble_full_scan_entries(config, &plan_outcomes, &write_outcomes);
    emit_scan_receipt(&render_scan_report_envelope(
        plan_failures.is_empty() && write_failures.is_empty(),
        "write",
        options.write_pdf,
        scan_hook_for_envelope(
            no_hooks,
            options.dry_run,
            pre_scan_hook.as_ref(),
        ),
        intake_moves(config, &intake),
        scan_summary_json(
            &counts,
            pdfs.len(),
            plan_failures.len() + write_failures.len(),
        ),
        assembled.notes,
        assembled.failures,
        Vec::new(),
        false,
        Vec::new(),
        assembled.held,
    ));
    if plan_failures.is_empty() && write_failures.is_empty() {
        Ok(())
    } else {
        Err(scan_partial_failure_error(&plan_failures, &write_failures))
    }
}

/// Plan every PDF, returning results in the same order as `pdfs`.
///
/// With `jobs <= 1` (or a trivial workload) this is the original sequential
/// loop. Otherwise a small fixed pool of scoped threads pulls PDFs off a shared
/// counter; each worker keeps its results paired with their original index so
/// we can restore `pdfs` order before returning.
pub(super) fn plan_pdfs(
    config: &Config,
    pdfs: &[PathBuf],
    options: SyncOptions,
    jobs: usize,
    ctx: &ScanContext,
) -> Vec<ScanPlanOutcome> {
    if jobs <= 1 || pdfs.len() <= 1 {
        return pdfs
            .iter()
            .map(|pdf| plan_pdf_sync_for_scan(config, pdf, options, ctx))
            .collect();
    }

    let next = AtomicUsize::new(0);
    let worker_count = jobs.min(pdfs.len());
    let mut indexed: Vec<(usize, ScanPlanOutcome)> = thread::scope(|scope| {
        let handles: Vec<_> = (0..worker_count)
            .map(|_| {
                let next = &next;
                scope.spawn(move || {
                    let mut local = Vec::new();
                    loop {
                        let index = next.fetch_add(1, Ordering::Relaxed);
                        let Some(pdf) = pdfs.get(index) else {
                            break;
                        };
                        local.push((
                            index,
                            plan_pdf_sync_for_scan(config, pdf, options, ctx),
                        ));
                    }
                    local
                })
            })
            .collect();
        handles
            .into_iter()
            .flat_map(|handle| {
                handle.join().expect("planning worker thread panicked")
            })
            .collect()
    });

    indexed.sort_by_key(|(index, _)| *index);
    indexed.into_iter().map(|(_, result)| result).collect()
}

/// Plan one scan target, recognizing an archived capture before marker
/// parsing or any annotation/task planning.
fn plan_pdf_sync_for_scan(
    config: &Config,
    pdf: &Path,
    options: SyncOptions,
    ctx: &ScanContext,
) -> ScanPlanOutcome {
    match replaced_capture_hold(config, pdf) {
        Ok(Some(hold)) => ScanPlanOutcome::Held(hold),
        Ok(None) => match plan_pdf_sync(config, pdf, options, ctx) {
            Ok(plan) => ScanPlanOutcome::Planned(Box::new(plan)),
            Err(error) => ScanPlanOutcome::Failed(ScanFailure::new(
                pdf.to_path_buf(),
                error,
            )),
        },
        Err(error) => {
            ScanPlanOutcome::Failed(ScanFailure::new(pdf.to_path_buf(), error))
        }
    }
}

/// Hold a PDF whose exact bytes match one of this reference's archived
/// annotated captures. The archive reader validates lineage and paths; no
/// filename or cross-reference hash guesses are used.
pub(super) fn replaced_capture_hold(
    config: &Config,
    pdf: &Path,
) -> Result<Option<ScanHold>> {
    if !pdf.is_file() {
        return Ok(None);
    }
    let note_path = ref_note_path(config, pdf)?;
    if !note_path.is_file() {
        return Ok(None);
    }
    let Ok(note_relative) = note_path.strip_prefix(&config.bob_dir) else {
        return Ok(None);
    };
    let note_relative = note_relative.to_string_lossy().replace('\\', "/");
    let records = overwrite::archive::records_for_live_note(
        &config.bob_dir,
        &note_relative,
    )
    .map_err(|error| {
        CommandError::new(format!(
            "read archived capture metadata for {note_relative}: {}",
            error.message
        ))
    })?;
    let archived_hashes = records
        .iter()
        .filter_map(|record| record.annotated_pdf.as_ref())
        .map(|archive| archive.sha256.as_str())
        .collect::<Vec<_>>();
    if archived_hashes.is_empty() {
        return Ok(None);
    }
    let live_hash = sha256_file(pdf)?;
    if !archived_hashes.contains(&live_hash.as_str()) {
        return Ok(None);
    }
    Ok(Some(ScanHold {
        pdf: pdf.to_path_buf(),
        note_path: note_path.clone(),
        title: note_title(pdf, &Projection::new()),
        reason: HoldReason::ReplacedCapture,
        message: "Highlights wrote the replaced capture back".to_string(),
    }))
}

/// Vault-relative forward-slash path for locator-index lookups.
fn vault_rel_forward(config: &Config, path: &Path) -> String {
    path.strip_prefix(&config.bob_dir)
        .map(|rel| rel.to_string_lossy().replace('\\', "/"))
        .unwrap_or_else(|_| path.to_string_lossy().replace('\\', "/"))
}

/// Derive the birth-only `source_markdown` pointer for a new note:
/// the single store under `.bob/ref-sources/<ref_type>/<stem>--*/`
/// whose recorded PDF sha matches the incoming PDF. `None` when the note
/// already exists, the type or stem is unknown, or no single store matches.
fn derive_birth_source_markdown(
    config: &Config,
    pdf: &Path,
    note_path: &Path,
    pdf_sha256: &str,
) -> Option<String> {
    use super::io::pdf_path_metadata;
    let meta = pdf_path_metadata(config, pdf).ok()?;
    let ref_type = meta.ref_type?;
    let stem = note_path.file_stem()?.to_str()?.to_string();
    let pointer = super::source_store::find_birth_store(
        &config.bob_dir,
        &ref_type,
        &stem,
        pdf_sha256,
    )?;
    // The catalog identifies the birth PDF, while read_store verifies the
    // content-addressed source bytes and their recorded digest. A pointer
    // is only safe to attach when both checks succeed.
    super::source_store::read_store(&config.bob_dir, &pointer)
        .bytes
        .as_ref()?;
    Some(pointer)
}

/// Carry a plan's birth-only state into execution metadata for a new
/// note: the reread `earlier:` lineage and the saved-source
/// `source_markdown` pointer. Both derive once at planning time; the
/// executor's fresh `pipeline_metadata(…)` starts with neither, so a
/// newborn re-render must copy them or the filed note loses them. For
/// existing notes the plan holds no birth state (`None`), and the
/// authored lines survive through frontmatter preservation.
fn carry_birth_only_metadata(
    metadata: &mut PipelineMetadata,
    stable: &PipelineMetadata,
) {
    metadata.birth_lineage = stable.birth_lineage.clone();
    metadata.source_markdown = stable.source_markdown.clone();
}

pub(super) fn plan_pdf_sync(
    config: &Config,
    pdf: &Path,
    options: SyncOptions,
    ctx: &ScanContext,
) -> Result<PdfSyncPlan> {
    let marker = read_pdf_marker(pdf)?;
    let marker_input = parse_marker_with_normalization(&marker.contents)?;
    let marker_projection = marker_input.projection.clone();
    let note_path = ref_note_path(config, pdf)?;
    validate_note_target(&note_path)?;
    let note = read_note(&note_path)?;
    let ref_note_rel = vault_rel_forward(config, &note_path);
    let candidates: Vec<ref_tasks_mod::LocatedRefTask> =
        ctx.index.candidates(&ref_note_rel).to_vec();
    let branch = classify_note_branch(
        note.exists(),
        &ref_note_rel,
        &note.body,
        &candidates,
    );
    if branch == NoteBranch::V2 {
        return plan_pdf_sync_v2(
            config,
            pdf,
            options,
            ctx,
            marker,
            marker_input,
            marker_projection,
            note_path,
            note,
            ref_note_rel,
            candidates,
        );
    }
    let pdf_task_line = parse_pdf_task_line(&note.body)?;
    let frontmatter_input = note.synced_projection_with_normalization()?;
    let frontmatter_projection = frontmatter_input.projection;
    let marker_hash = projection_hash(&marker_projection)?;
    let frontmatter_hash = projection_hash(&frontmatter_projection)?;
    let last_hash = note.marker_hash();
    let (base_projection, base_status_normalized) =
        note.marker_base_projection_with_normalization()?;
    let status_normalization = StatusNormalization {
        marker: marker_input.status_normalized,
        frontmatter: frontmatter_input.status_normalized,
        base: base_status_normalized,
    };
    let stale_restore = is_stale_editor_save(
        note.marker_replaced_hash().as_deref(),
        last_hash.as_deref(),
        base_projection.is_some(),
        &marker_hash,
    );
    let resolve_marker = if stale_restore {
        base_projection
            .as_ref()
            .expect("stale save requires a stored base")
    } else {
        &marker_projection
    };
    let resolve_hash = if stale_restore {
        last_hash
            .as_deref()
            .expect("stale save requires a stored hash")
    } else {
        marker_hash.as_str()
    };
    let mut resolution = resolve_sync_projection(SyncInputs {
        last_hash: last_hash.as_deref(),
        base_projection: base_projection.as_ref(),
        marker_projection: resolve_marker,
        marker_hash: resolve_hash,
        frontmatter_projection: &frontmatter_projection,
        frontmatter_hash: &frontmatter_hash,
        note_exists: note.exists(),
        prefer: options.prefer,
    })?;
    // Capture intake eligibility before the `^ref` task signal so the final
    // closing run still imports pending annotation tasks from any open
    // status (`ready`, `next`, or `wip`), then re-check after the signal so
    // a reopen to any open status also intakes in the same run instead of
    // waiting for the next pass.
    let intake_allowed_before_signal =
        projection_status_is_open(&resolution.projection);
    let pdf_task_signal = apply_pdf_task_status_signal(
        &mut resolution,
        &pdf_task_line,
        base_projection.as_ref(),
        resolve_marker,
        &frontmatter_projection,
    )?;
    if stale_restore {
        append_stale_save_reason(&mut resolution.decision.reason);
    }
    let annotation_task_intake_allowed = intake_allowed_before_signal
        || projection_status_is_open(&resolution.projection);
    let decision = resolution.decision;
    let synced_projection = resolution.projection;
    validate_required_marker_keys(
        &synced_projection,
        decision.source.as_str(),
    )?;
    let synced_hash = projection_hash(&synced_projection)?;
    let rendered_marker = render_marker(&synced_projection)?;
    let mut marker_write_needed = (decision.frontmatter_contributed
        || status_normalization.marker.is_some()
        || stale_restore)
        && normalize_line_endings(&rendered_marker)
            != normalize_line_endings(&marker.contents);

    if marker_write_needed && !options.write_pdf && !options.dry_run {
        if skip_stale_restore_without_write_opt_in(
            stale_restore,
            decision.frontmatter_contributed,
            status_normalization.marker.is_some(),
            options,
        ) {
            marker_write_needed = false;
        } else {
            return Err(CommandError::new(
                "reference note changed but --write-pdf was not supplied; refusing to update the PDF marker",
            )
            .with_code("needs_write_pdfs"));
        }
    }

    let sidecar = read_sidecar_for_sync(pdf, options)?;
    let strip_return_links =
        super::return_links::strip_enabled(&synced_projection);
    let rendered_highlights = sidecar
        .as_ref()
        .map(|sidecar| {
            render_sidecar_highlights(
                config,
                pdf,
                &note_path,
                &note,
                sidecar,
                strip_return_links,
            )
        })
        .transpose()?;
    let sidecar_path = sidecar.as_ref().map(|sidecar| sidecar.path.clone());
    let rendered_highlights_count =
        rendered_highlights.as_ref().map(|rendered| rendered.count);
    let image_assets = rendered_highlights
        .as_ref()
        .map(|rendered| rendered.image_assets.clone())
        .unwrap_or_default();
    let mut stable_metadata = pipeline_metadata(
        config,
        pdf,
        &marker.source_pdf_sha256,
        &note,
        sidecar.as_ref(),
        rendered_highlights.as_ref(),
        false,
    )?;
    // Birth-only `source_markdown` pointer: new notes look up the
    // write-once store by (ref_type, stem, PDF sha); existing notes keep
    // their authored line verbatim.
    if !note.exists()
        && let Some(pointer) = derive_birth_source_markdown(
            config,
            pdf,
            &note_path,
            &marker.source_pdf_sha256,
        )
    {
        stable_metadata.source_markdown = Some(pointer);
    }
    let rendered_body = note.render_body(
        pdf,
        &synced_projection,
        &stable_metadata.source_pdf,
        rendered_highlights.as_ref(),
        &stable_metadata,
    )?;
    let annotation_task_candidates = if annotation_task_intake_allowed {
        annotation_task_candidates(
            config,
            &note_path,
            pdf,
            sidecar.as_ref(),
            rendered_highlights.as_ref(),
            strip_return_links,
        )?
    } else {
        Vec::new()
    };
    let existing_replaced = note.marker_replaced_hash();
    let dropping_replaced = is_dropping_replaced_hash(
        last_hash.as_deref(),
        &marker_hash,
        marker_write_needed,
        existing_replaced.as_deref(),
    );
    let replaced_hash = replaced_hash_to_store(
        existing_replaced.as_deref(),
        &marker_hash,
        last_hash.as_deref(),
        marker_write_needed,
    );
    let rendered_with_hash = note.render_with_projection(
        &synced_projection,
        &synced_hash,
        replaced_hash.as_deref(),
        &stable_metadata,
        &rendered_body,
    );
    let (stable_rendered_note, replaced_hash) = finalize_replaced_hash_render(
        &note,
        rendered_with_hash,
        replaced_hash,
        dropping_replaced,
    );
    let stable_note_action = change_action(
        note.exists(),
        note.contents().as_deref(),
        &stable_rendered_note,
    );
    // The legacy v1 path keeps today's planning and execution exactly, but
    // it still carries a hands-off reading plan so human reports can count
    // the remaining open in-note trackers. Bytes and writes are unchanged.
    let v1_reading_plan = ReadingTaskPlan {
        branch: NoteBranch::V1,
        kind: ReadingTaskKind::V1,
        action: ReadingTaskAction::NoWrite,
        residence: None,
        embed: None,
        status_target: None,
        task_changed: false,
        refuse_status_parent_writes: false,
        diagnostics: open_v1_tracker_diagnostic(&ref_tasks_mod::find_trackers(
            &note.body,
        ))
        .into_iter()
        .collect(),
    };

    Ok(PdfSyncPlan {
        pdf: pdf.to_path_buf(),
        note_path,
        sidecar_path,
        marker,
        decision,
        rendered_highlights_count,
        synced_projection,
        synced_hash,
        rendered_marker,
        marker_write_needed,
        stale_restore,
        replaced_hash,
        note,
        sidecar,
        rendered_highlights,
        stable_metadata,
        rendered_body,
        stable_rendered_note,
        stable_note_action,
        image_assets,
        annotation_task_candidates,
        annotation_tasks_created: 0,
        annotation_tasks_skipped: 0,
        annotation_task_relation: None,
        routed_task_note_writes: Vec::new(),
        routed_intents: Vec::new(),
        reading_task_plan: Some(v1_reading_plan),
        reading_located: None,
        reading_diagnostics: Vec::new(),
        preview_block_id: None,
        v2_residence: None,
        pdf_task_signal,
        status_normalization,
    })
}

/// Plan one PDF on the v2 branch: parent-free sync snapshots, located-task
/// status via the reading-task plan, residence-rendered parents, and managed
/// embeds. V1 behavior is untouched (the caller only routes V2 here).
#[allow(clippy::too_many_arguments)]
fn plan_pdf_sync_v2(
    config: &Config,
    pdf: &Path,
    options: SyncOptions,
    ctx: &ScanContext,
    marker: PdfMarker,
    marker_input: NormalizedProjection,
    marker_projection: Projection,
    note_path: PathBuf,
    note: ParsedNote,
    ref_note_rel: String,
    candidates: Vec<ref_tasks_mod::LocatedRefTask>,
) -> Result<PdfSyncPlan> {
    let v1_hits = ref_tasks_mod::find_trackers(&note.body);
    let frontmatter_input = note.synced_projection_with_normalization()?;
    let parent_projection = if note.exists() {
        &frontmatter_input.projection
    } else {
        &marker_projection
    };
    let parent_hint = parent_projection
        .get(FIELD_PARENT)
        .and_then(MarkerValue::as_string)
        .unwrap_or("")
        .to_string();
    let marker_pf = without_parent(&marker_projection);
    let frontmatter_pf = without_parent(&frontmatter_input.projection);
    let marker_hash = projection_hash(&marker_pf)?;
    let frontmatter_hash = projection_hash(&frontmatter_pf)?;
    let last_hash = note.marker_hash();
    let (base_projection, base_status_normalized) =
        note.marker_base_projection_with_normalization()?;
    let base_pf = base_projection.as_ref().map(normalize_v2_base);
    let status_normalization = StatusNormalization {
        marker: marker_input.status_normalized,
        frontmatter: frontmatter_input.status_normalized,
        base: base_status_normalized,
    };
    let stale_restore = is_stale_editor_save(
        note.marker_replaced_hash().as_deref(),
        last_hash.as_deref(),
        base_pf.is_some(),
        &marker_hash,
    );
    let resolve_marker = if stale_restore {
        base_pf.as_ref().expect("stale save requires a stored base")
    } else {
        &marker_pf
    };
    let resolve_hash = if stale_restore {
        last_hash
            .as_deref()
            .expect("stale save requires a stored hash")
    } else {
        marker_hash.as_str()
    };
    let mut resolution = resolve_sync_projection(SyncInputs {
        last_hash: last_hash.as_deref(),
        base_projection: base_pf.as_ref(),
        marker_projection: resolve_marker,
        marker_hash: resolve_hash,
        frontmatter_projection: &frontmatter_pf,
        frontmatter_hash: &frontmatter_hash,
        note_exists: note.exists(),
        prefer: options.prefer,
    })?;
    let status_of = |projection: &Projection| {
        projection
            .get(FIELD_STATUS)
            .and_then(MarkerValue::as_string)
            .map(str::to_string)
    };
    let resolved_status = status_of(&resolution.projection);
    let base_status = base_pf
        .as_ref()
        .and_then(|base| {
            base.get(FIELD_STATUS).and_then(MarkerValue::as_string)
        })
        .map(str::to_string);
    let marker_status = status_of(resolve_marker);
    let frontmatter_status = status_of(&frontmatter_pf);
    // Preliminary selection to size the reopen fallback: when the selected
    // task is archived-terminal, its source residence decides the reopen
    // parent. The planner re-selects internally; this only feeds
    // `archive_source_open`.
    let preliminary =
        ref_tasks_mod::select_for_ref(&candidates, &v1_hits, None);
    let archive_source_open = match &preliminary.task {
        Some(ref_tasks_mod::Selected::V2(located)) if located.archived => {
            located.residence.as_deref().is_some_and(|route| {
                parent_notes_mod::resolve_parent(&config.bob_dir, route).is_ok()
            })
        }
        _ => false,
    };
    // Reread birth lineage derives once, when the note is new, from the
    // PDF marker identity: a `<base>__vN` stem probes for a live
    // predecessor sharing identity, and the confirmed reading flows into
    // task creation and previews. Reopens confirm from the existing
    // note's own `earlier:` list instead. Ordinary births carry none.
    let birth_lineage = if note.exists() {
        None
    } else {
        derive_birth_lineage(config, &note_path, &marker_projection)
    };
    let note_raws: Vec<String> = note
        .frontmatter
        .iter()
        .map(|entry| entry.raw.clone())
        .collect();
    let confirmed_reading = birth_lineage
        .as_ref()
        .map(|lineage| ref_tasks_mod::ReadingIdentity {
            root_stem: lineage.root_stem.clone(),
            number: lineage.reading,
        })
        .or_else(|| {
            note.exists()
                .then(|| reading_from_note_frontmatter(&note_raws))
                .flatten()
        });
    let reading_plan = plan_reading_task(ReadingTaskPlanInputs {
        note_exists: note.exists(),
        ref_note_rel: &ref_note_rel,
        note_body: &note.body,
        candidates: &candidates,
        v1_hits: &v1_hits,
        parent_hint: &parent_hint,
        resolved_status: resolved_status.as_deref(),
        base_status: base_status.as_deref(),
        marker_status: marker_status.as_deref(),
        frontmatter_status: frontmatter_status.as_deref(),
        archive_source_open,
        confirmed_reading,
        resolver: &|hint| {
            parent_notes_mod::resolve_parent(&config.bob_dir, hint)
        },
    })?;
    // Eligibility from the merged projection before the reading-task signal,
    // mirroring the v1 path: an open merged status before the signal keeps
    // the closing run's intake, and an open status after the signal keeps a
    // reopen intake in the same run.
    let pre_signal_open = projection_status_is_open(&resolution.projection);
    // Task-gesture status drives the synced projection (like v1's PDF-task
    // signal, but from the located task via the stored base). Marker-only or
    // frontmatter-only changes leave the merged projection alone; the
    // executor flips the task checkbox instead.
    if let Some(target) = reading_plan.status_target
        && reading_plan.task_changed
        && Some(target)
            != resolution
                .projection
                .get(FIELD_STATUS)
                .and_then(MarkerValue::as_string)
    {
        resolution.projection.insert(
            FIELD_STATUS.to_string(),
            MarkerValue::String(target.to_string()),
        );
        resolution.decision.frontmatter_contributed = true;
        if !resolution.decision.reason.is_empty() {
            resolution.decision.reason.push_str("; ");
        }
        resolution
            .decision
            .reason
            .push_str("reading task set status");
    }
    if stale_restore {
        append_stale_save_reason(&mut resolution.decision.reason);
    }
    // Preserve annotation eligibility before and after the status signal so
    // closing and reopening runs import pending work from any open status.
    let annotation_task_intake_allowed =
        pre_signal_open || projection_status_is_open(&resolution.projection);
    // Ambiguity or unselectable state refuses status/parent/task writes
    // rather than guessing: no task, marker, or frontmatter write and no
    // invented birth embed. A per-PDF planning failure keeps partial-scan
    // continuation (the scan collects this Err alongside other PDFs).
    if reading_plan.refuse_status_parent_writes {
        let mut detail = reading_plan
            .diagnostics
            .iter()
            .map(|diagnostic| {
                format!("{}: {}", diagnostic.code, diagnostic.detail)
            })
            .collect::<Vec<_>>()
            .join("; ");
        if detail.is_empty() {
            detail = "ambiguous reading tasks; refusing status/parent writes"
                .to_string();
        }
        let mut error = CommandError::new(format!(
            "refusing to sync {}: {detail}",
            pdf.display()
        ));
        if reading_plan
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "multiple_open_ref_tasks")
        {
            error.ref_task_claimants = candidates
                .iter()
                .filter(|task| {
                    !task.archived && ref_tasks_mod::is_open_mark(task.mark)
                })
                .map(|task| task.path.clone())
                .collect();
        }
        return Err(error);
    }
    let decision = resolution.decision;
    let synced_pf = resolution.projection;
    // Parent for the rendered note: a reopen/birth insert uses its new
    // destination route (never the archived task's old residence); ordinary
    // existing refs use the live located residence, and missing tasks keep
    // no residence and never invent one.
    let birth_route: Option<String> = match &reading_plan.action {
        ReadingTaskAction::Insert {
            destination_route, ..
        } => Some(destination_route.clone()),
        _ => None,
    };
    let residence: Option<String> = match &reading_plan.action {
        ReadingTaskAction::Insert { .. } => birth_route.clone(),
        _ => reading_plan.residence.clone().or(birth_route.clone()),
    };
    // Never silently drop `parent` from validation: require the marker hint
    // or a plan residence.
    let full_for_validation = {
        let mut full = synced_pf.clone();
        if let Some(route) = residence.as_deref() {
            full.insert(
                FIELD_PARENT.to_string(),
                residence_parent_value(route),
            );
        } else if let Some(parent) = marker_projection.get(FIELD_PARENT) {
            full.insert(FIELD_PARENT.to_string(), parent.clone());
        }
        full
    };
    validate_required_marker_keys(
        &full_for_validation,
        decision.source.as_str(),
    )?;
    let synced_hash = projection_hash(&synced_pf)?;
    // Semantic parent-free update versus the optional residence-hint
    // refresh: only non-parent synced-field contributions (including
    // lifecycle status and normalization) need the PDF opt-in. A
    // parent-only move never triggers the refusal, and normal scans
    // preserve the marker birth hint; only an explicit PDF write refreshes
    // a stale hint. Writing runs refuse before any write when required
    // marker changes lack opt-in; dry runs preview them.
    let semantic_rendered = render_marker(&synced_pf)?;
    let semantic_current = render_marker(&marker_pf)?;
    let semantic_needed = (decision.frontmatter_contributed
        || status_normalization.marker.is_some()
        || stale_restore)
        && normalize_line_endings(&semantic_rendered)
            != normalize_line_endings(&semantic_current);
    let skip_restore = skip_stale_restore_without_write_opt_in(
        stale_restore,
        decision.frontmatter_contributed,
        status_normalization.marker.is_some(),
        options,
    );
    if semantic_needed
        && !options.write_pdf
        && !options.dry_run
        && !skip_restore
    {
        return Err(CommandError::new(
            "reference note changed but --write-pdf was not supplied; refusing to update the PDF marker",
        )
        .with_code("needs_write_pdfs"));
    }
    let (rendered_marker, marker_write_needed) = match residence.as_deref() {
        Some(route) => {
            let with_hint =
                marker_projection_with_parent_hint(&synced_pf, route);
            let rendered = render_marker(&with_hint)?;
            let hint_needed = normalize_line_endings(&rendered)
                != normalize_line_endings(&marker.contents);
            // Dry runs preview the required semantic change even without
            // opt-in; normal writes also refresh a stale hint under opt-in.
            let needed = semantic_needed || (options.write_pdf && hint_needed);
            // Without opt-in (and outside dry-run preview) preserve the
            // marker bytes; the semantic refusal above already fired when
            // needed, so here `needed` is only true under opt-in/dry-run.
            if skip_restore {
                (marker.contents.clone(), false)
            } else if options.write_pdf || options.dry_run {
                (rendered, needed)
            } else {
                (marker.contents.clone(), false)
            }
        }
        None => (marker.contents.clone(), false),
    };
    let sidecar = read_sidecar_for_sync(pdf, options)?;
    let strip_return_links = super::return_links::strip_enabled(&synced_pf);
    let rendered_highlights = sidecar
        .as_ref()
        .map(|sidecar| {
            render_sidecar_highlights(
                config,
                pdf,
                &note_path,
                &note,
                sidecar,
                strip_return_links,
            )
        })
        .transpose()?;
    let sidecar_path = sidecar.as_ref().map(|sidecar| sidecar.path.clone());
    let rendered_highlights_count =
        rendered_highlights.as_ref().map(|rendered| rendered.count);
    let image_assets = rendered_highlights
        .as_ref()
        .map(|rendered| rendered.image_assets.clone())
        .unwrap_or_default();
    let mut stable_metadata = pipeline_metadata(
        config,
        pdf,
        &marker.source_pdf_sha256,
        &note,
        sidecar.as_ref(),
        rendered_highlights.as_ref(),
        false,
    )?;
    if let Some(lineage) = birth_lineage {
        stable_metadata.birth_lineage = Some(lineage);
    }
    // Birth-only `source_markdown` pointer alongside birth lineage: new
    // notes look up the write-once store by (ref_type, stem, PDF sha);
    // existing notes keep their authored line verbatim.
    if !note.exists()
        && stable_metadata.source_markdown.is_none()
        && let Some(pointer) = derive_birth_source_markdown(
            config,
            pdf,
            &note_path,
            &marker.source_pdf_sha256,
        )
    {
        stable_metadata.source_markdown = Some(pointer);
    }
    let highlights_content = rendered_highlights
        .as_ref()
        .map(|rendered| rendered.content.as_str())
        .unwrap_or("");
    // The confirmed reading for a birth insert: the preview and execution
    // IDs agree through one base, so the placeholder patch names the same
    // ID the executor allocates.
    let insert_reading: Option<ref_tasks_mod::ReadingIdentity> =
        match &reading_plan.action {
            ReadingTaskAction::Insert { reading, .. } => reading.clone(),
            _ => None,
        };
    let pdf_stem = pdf
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("ref");
    // Render the body: births use the managed-embed anatomy (no in-note
    // tracker); existing notes heal the embed slot and keep highlights
    // rendering; missing/refused notes never invent a target.
    let rendered_body = if !note.exists() {
        match &reading_plan.action {
            ReadingTaskAction::Insert { .. } => {
                let route = residence.as_deref().unwrap_or("mac_inbox");
                v2_birth_body(
                    &note_title(pdf, &synced_pf),
                    route,
                    &ref_tasks_mod::birth_preview_id(
                        pdf_stem,
                        insert_reading.as_ref(),
                    ),
                    highlights_content,
                    stable_metadata.audio.as_deref(),
                )
            }
            _ => {
                if let Some(embed) = reading_plan.embed.as_ref() {
                    v2_birth_body(
                        &note_title(pdf, &synced_pf),
                        &embed.target,
                        &embed.block_id,
                        highlights_content,
                        stable_metadata.audio.as_deref(),
                    )
                } else {
                    v2_birth_body(
                        &note_title(pdf, &synced_pf),
                        birth_route.as_deref().unwrap_or("mac_inbox"),
                        &format!(
                            "ref-{}",
                            ref_tasks_mod::slug_ref_stem(
                                pdf.file_stem()
                                    .and_then(|stem| stem.to_str())
                                    .unwrap_or("ref"),
                            )
                        ),
                        highlights_content,
                        stable_metadata.audio.as_deref(),
                    )
                }
            }
        }
    } else if let Some(embed) = reading_plan.embed.as_ref() {
        let healed =
            heal_managed_embed(&note.body, &embed.target, &embed.block_id);
        // Preserve managed-region validation: a missing/broken Highlights
        // region on the healed path fails with the established
        // missing/broken-region error rather than silently syncing the
        // healed embed alone.
        let body_with_highlights = if rendered_highlights.is_some() {
            let replacement = highlights_content;
            replace_managed_region(&healed, replacement)?
        } else {
            healed
        };
        maybe_insert_audio_embed_fallback(
            &note,
            &stable_metadata,
            &body_with_highlights,
        )
    } else {
        note.render_body(
            pdf,
            &synced_pf,
            &stable_metadata.source_pdf,
            rendered_highlights.as_ref(),
            &stable_metadata,
        )?
    };
    let annotation_task_candidates = if annotation_task_intake_allowed {
        annotation_task_candidates(
            config,
            &note_path,
            pdf,
            sidecar.as_ref(),
            rendered_highlights.as_ref(),
            strip_return_links,
        )?
    } else {
        Vec::new()
    };
    // Render frontmatter with the residence parent, but store the
    // parent-free base: hashes and bases never include `parent`.
    let mut render_projection = synced_pf.clone();
    if let Some(route) = residence.as_deref() {
        render_projection
            .insert(FIELD_PARENT.to_string(), residence_parent_value(route));
    } else if let Some(parent) = frontmatter_input.projection.get(FIELD_PARENT)
    {
        render_projection.insert(FIELD_PARENT.to_string(), parent.clone());
    }
    let existing_replaced = note.marker_replaced_hash();
    let dropping_replaced = is_dropping_replaced_hash(
        last_hash.as_deref(),
        &marker_hash,
        marker_write_needed,
        existing_replaced.as_deref(),
    );
    let replaced_hash = replaced_hash_to_store(
        existing_replaced.as_deref(),
        &marker_hash,
        last_hash.as_deref(),
        marker_write_needed,
    );
    let mut stable_rendered_note = note.render_with_projection(
        &render_projection,
        &synced_hash,
        replaced_hash.as_deref(),
        &stable_metadata,
        &rendered_body,
    );
    stable_rendered_note =
        restamp_parent_free_base(&stable_rendered_note, &synced_pf);
    let (stable_rendered_note, replaced_hash) = finalize_replaced_hash_render(
        &note,
        stable_rendered_note,
        replaced_hash,
        dropping_replaced,
    );
    let stable_note_action = change_action(
        note.exists(),
        note.contents().as_deref(),
        &stable_rendered_note,
    );
    // A no-op PDF-task signal for v1-shaped reports: v2 status flows
    // through the reading plan instead.
    let pdf_task_signal = PdfTaskStatusSignal {
        status: PdfTaskStatus::Missing,
        status_contributed: None,
    };
    let diagnostics = reading_plan.diagnostics.clone();
    let reading_located =
        select_v2_located_task(note.exists(), &candidates, &v1_hits);
    let _ = ctx;
    Ok(PdfSyncPlan {
        pdf: pdf.to_path_buf(),
        note_path,
        sidecar_path,
        marker,
        decision,
        rendered_highlights_count,
        synced_projection: synced_pf,
        synced_hash,
        rendered_marker,
        marker_write_needed,
        stale_restore,
        replaced_hash,
        note,
        sidecar,
        rendered_highlights,
        stable_metadata,
        rendered_body,
        stable_rendered_note,
        stable_note_action,
        image_assets,
        annotation_task_candidates,
        annotation_tasks_created: 0,
        annotation_tasks_skipped: 0,
        annotation_task_relation: None,
        routed_task_note_writes: Vec::new(),
        routed_intents: Vec::new(),
        reading_task_plan: Some(reading_plan),
        reading_located,
        reading_diagnostics: diagnostics,
        preview_block_id: None,
        v2_residence: residence,
        pdf_task_signal,
        status_normalization,
    })
}

/// Mirror the planner's selection to retain the located task for
/// execution-time revalidation: births adopt the uniquely located orphan,
/// existing notes keep the selected v2 task. Returns `None` for fresh
/// inserts, refusals, missing tasks, and v1 notes.
fn select_v2_located_task(
    note_exists: bool,
    candidates: &[ref_tasks_mod::LocatedRefTask],
    v1_hits: &[ref_tasks_mod::TrackerHit],
) -> Option<ref_tasks_mod::LocatedRefTask> {
    if !note_exists {
        let (task, _) = select_birth_task(candidates, None);
        return match task {
            BirthTask::Adopt(located) => Some(located),
            BirthTask::Insert | BirthTask::Refused => None,
        };
    }
    match ref_tasks_mod::select_for_ref(candidates, v1_hits, None).task {
        Some(ref_tasks_mod::Selected::V2(located)) => Some(located),
        _ => None,
    }
}

/// Rewrite the stored `highlights_marker_base` line to the parent-free
/// snapshot so a residence move never reads as a two-sided edit. The hash
/// line already carries the parent-free hash from planning.
fn restamp_parent_free_base(
    rendered: &str,
    parent_free: &Projection,
) -> String {
    let replacement = format!(
        "{FIELD_MARKER_BASE}: {}",
        MarkerValue::String(projection_snapshot_json(parent_free))
            .as_frontmatter_value()
    );
    let trailing = rendered.ends_with('\n');
    let mut out = Vec::new();
    for line in rendered.lines() {
        if line.starts_with(&format!("{FIELD_MARKER_BASE}:")) {
            out.push(replacement.clone());
        } else {
            out.push(line.to_string());
        }
    }
    let mut rendered = out.join("\n");
    if trailing {
        rendered.push('\n');
    }
    rendered
}

/// Late companion audio for existing-v2 healed bodies: anchor directly
/// after the managed reading-task embed so a late companion appears there
/// even when authored text lies before `## Highlights`, exactly once,
/// preserving that text. V1 placement via `render_body` is untouched.
fn maybe_insert_audio_embed_fallback(
    note: &ParsedNote,
    metadata: &PipelineMetadata,
    body: &str,
) -> String {
    let Some(audio) = metadata.audio.as_deref() else {
        return body.to_string();
    };
    if note_has_audio_field(note) {
        return body.to_string();
    }
    maybe_insert_audio_embed_after_managed(body, audio)
}

/// Reserve deterministic preview IDs in PDF order.
///
/// After parallel planning, walk plans in scan order and reserve a preview
/// block ID for each planned birth/reopen insert against the destination and
/// its archive plus earlier previews. Previews may name IDs for human
/// display; execution always reallocates against current disk bytes, and
/// writes/embeds report the actual result when a race causes a difference.
pub(super) fn reserve_preview_ids(
    config: &Config,
    plans: &mut [&mut PdfSyncPlan],
    ctx: &ScanContext,
) {
    let _ = ctx;
    let mut taken: BTreeMap<PathBuf, BTreeSet<String>> = BTreeMap::new();
    for plan in plans.iter_mut() {
        let (destination_route, prefer_block_id, reading) = match plan
            .reading_task_plan
            .as_ref()
            .map(|reading_plan| &reading_plan.action)
        {
            Some(ReadingTaskAction::Insert {
                destination_route,
                prefer_block_id,
                reading,
                ..
            }) => (
                destination_route.clone(),
                prefer_block_id.clone(),
                reading.clone(),
            ),
            _ => continue,
        };
        let destination =
            config.bob_dir.join(format!("{destination_route}.md"));
        let taken_set = taken.entry(destination.clone()).or_insert_with(|| {
            preview_taken_for_destination(&config.bob_dir, &destination)
        });
        let is_taken = |id: &str| taken_set.contains(id);
        let stem = plan
            .pdf
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or("ref");
        let preview = match prefer_block_id.as_deref() {
            Some(preferred) if !preferred.is_empty() => {
                ref_tasks_mod::allocate_unique_block_id(preferred, &is_taken)
            }
            _ => ref_tasks_mod::allocate_birth_block_id(
                stem,
                reading.as_ref(),
                &is_taken,
            ),
        };
        taken_set.insert(preview.clone());
        // Patch the planning placeholder (fresh `ref-<slug>` or
        // `ref-<root-slug>-vN`, or the reopen old ID) to the reserved
        // preview so human display names the ID. Execution reallocates
        // against fresh disk and reports the actual.
        let placeholder = match prefer_block_id.as_deref() {
            Some(preferred) if !preferred.is_empty() => preferred.to_string(),
            _ => ref_tasks_mod::birth_preview_id(stem, reading.as_ref()),
        };
        if preview != placeholder {
            let needle = format!("#^{placeholder}");
            let replacement = format!("#^{preview}");
            plan.rendered_body =
                plan.rendered_body.replace(&needle, &replacement);
            plan.stable_rendered_note =
                plan.stable_rendered_note.replace(&needle, &replacement);
            plan.stable_note_action = change_action(
                plan.note.exists(),
                plan.note.contents().as_deref(),
                &plan.stable_rendered_note,
            );
        }
        plan.preview_block_id = Some(preview);
    }
}

/// Block IDs already taken for preview allocation: the destination's live
/// IDs plus its archive's, mirroring the executor's collision rules.
fn preview_taken_for_destination(
    bob_dir: &Path,
    destination: &Path,
) -> BTreeSet<String> {
    let mut taken = BTreeSet::new();
    let dest_contents = fs::read_to_string(destination).unwrap_or_default();
    taken.extend(crate::native::collect_done::block_ids_in_markdown(
        &dest_contents,
    ));
    if let Some(archive_rel) =
        preview_archive_rel_for(bob_dir, destination, &dest_contents)
    {
        let archive_contents =
            fs::read_to_string(bob_dir.join(&archive_rel)).unwrap_or_default();
        taken.extend(crate::native::collect_done::block_ids_in_markdown(
            &archive_contents,
        ));
    }
    taken
}

/// Archive relative path for preview seeding: the note's own `done_tasks`
/// link when present, else the vault-root default `done/<stem>_done.md`.
fn preview_archive_rel_for(
    bob_dir: &Path,
    destination: &Path,
    dest_contents: &str,
) -> Option<String> {
    for line in dest_contents.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("done_tasks:")
            && let Some(start) = trimmed.find("[[")
            && let Some(end) = trimmed[start..].find("]]")
        {
            let target = &trimmed[start + 2..start + end];
            let mut rel = target.trim().to_string();
            if !rel.ends_with(".md") {
                rel.push_str(".md");
            }
            return Some(rel);
        }
    }
    let relative = destination.strip_prefix(bob_dir).unwrap_or(destination);
    if relative.components().count() == 1
        && let Some(stem) = destination.file_stem().and_then(|s| s.to_str())
    {
        return Some(format!("done/{stem}_done.md"));
    }
    None
}

pub(super) fn finalize_annotation_task_plans(
    config: &Config,
    plans: &mut [&mut PdfSyncPlan],
    ctx: &ScanContext,
) -> Result<()> {
    // Building the processed-task index walks the entire vault. Skip it
    // entirely when no plan carries annotation-task candidates: with nothing to
    // accept/reject there are no intents to form and no reference-note
    // bodies to mutate, so the index would never be consulted. This keeps the
    // common `sync`/`scan` path (terminal (`read`, `abandoned`, `legacy`) PDFs,
    // or open PDFs with no `#task` bullets) free of the vault-wide scan.
    if plans
        .iter()
        .all(|plan| plan.annotation_task_candidates.is_empty())
    {
        return Ok(());
    }

    let config_path = super::bob_config::config_path();
    let highlights = super::bob_config::load_highlights_config(&config_path)
        .map_err(super::hooks::config_error)?;
    let raw_relation =
        highlights.annotation_task_relation().map(str::to_string);
    let raw_ref = raw_relation.as_deref();
    finalize_annotation_task_plans_with_label(config, plans, ctx, raw_ref)
}

pub(super) fn normalize_annotation_task_relation(
    relation_raw: Option<&str>,
) -> Result<Option<String>> {
    let Some(raw) = relation_raw else {
        return Ok(None);
    };
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    match crate::native::task_relations::label::normalize_relation_label(
        trimmed,
    ) {
        Ok(label) => Ok(Some(label)),
        Err(error) => Err(CommandError::new(format!(
            "invalid highlights.annotation_task_relation in {}: {}; fix or remove it",
            super::bob_config::config_path().display(),
            error.as_str()
        ))),
    }
}

pub(super) fn finalize_annotation_task_plans_with_label(
    config: &Config,
    plans: &mut [&mut PdfSyncPlan],
    ctx: &ScanContext,
    relation_raw: Option<&str>,
) -> Result<()> {
    if plans
        .iter()
        .all(|plan| plan.annotation_task_candidates.is_empty())
    {
        return Ok(());
    }
    let normalized_relation = normalize_annotation_task_relation(relation_raw)?;

    let disk_index = processed_task_index(config)?;
    let created_date = ctx.invocation_date.clone();

    for plan in plans.iter_mut() {
        let plan = &mut **plan;
        plan.annotation_tasks_created = 0;
        plan.annotation_tasks_skipped = 0;
        plan.annotation_task_relation = None;
        plan.routed_task_note_writes.clear();
        plan.routed_intents.clear();

        // Per-plan acceptance against the pristine disk index: planning
        // reservations stay deterministic without one PDF's plan consuming
        // another's follow-up work. Execution dedups against actually
        // successful writes, so a failed PDF never starves a later one.
        let mut processed = disk_index.clone();
        let is_v2 = plan
            .reading_task_plan
            .as_ref()
            .is_some_and(|reading| reading.branch == NoteBranch::V2);
        let residence = plan.v2_residence.clone();
        // v1 local LINKS need the in-note `^ref` tracker. Check once
        // before mutating the body; a malformed duplicate still fails.
        let v1_tracker_present = if is_v2 {
            false
        } else {
            matches!(
                parse_pdf_task_line(&plan.rendered_body)?,
                PdfTaskLineState::Present(_)
            )
        };

        let mut reference_task_lines = Vec::new();
        // Per-destination intents preserve each PDF's ownership: later PDFs
        // are never attached to the first destination group's owner.
        let mut intents: BTreeMap<PathBuf, RoutedInsertionIntent> =
            BTreeMap::new();
        for candidate in std::mem::take(&mut plan.annotation_task_candidates) {
            let (candidate, is_v2_default) = if is_v2 {
                retarget_v2_follow_up(config, residence.as_deref(), candidate)
            } else {
                (candidate, false)
            };
            if !processed.accept(&candidate) {
                plan.annotation_tasks_skipped += 1;
                continue;
            }

            let task_line =
                render_annotation_task_line(config, &candidate, &created_date);
            plan.annotation_tasks_created += 1;
            match &candidate.target {
                AnnotationTaskTarget::ReferenceNote => {
                    reference_task_lines.push(task_line);
                    if let Some(label) = normalized_relation.as_deref()
                        && v1_tracker_present
                    {
                        let link = "[[#^ref]]".to_string();
                        reference_task_lines.push(
                            crate::native::task_relations::format::format_links_line(
                                "\t",
                                &[(link, Some(label.to_string()))],
                            ),
                        );
                    }
                }
                AnnotationTaskTarget::RoutedNote(path) => {
                    let relation = normalized_relation.clone();
                    let intent =
                        intents.entry(path.clone()).or_insert_with(|| {
                            RoutedInsertionIntent {
                                path: path.clone(),
                                lines: Vec::new(),
                                candidates: Vec::new(),
                                owner: plan.pdf.clone(),
                                is_v2_default,
                                annotation_task_relation: relation.clone(),
                            }
                        });
                    // A destination shared by explicit and default routes
                    // keeps the legacy append behavior unless every line is
                    // a v2 default.
                    intent.is_v2_default &= is_v2_default;
                    if intent.annotation_task_relation.is_none() {
                        intent.annotation_task_relation =
                            normalized_relation.clone();
                    }
                    intent.lines.push(task_line);
                    intent.candidates.push(candidate);
                }
            }
        }

        if plan.annotation_tasks_created > 0 {
            plan.annotation_task_relation = normalized_relation.clone();
        }
        if !reference_task_lines.is_empty() {
            plan.rendered_body =
                insert_annotation_task_lines_into_tasks_section(
                    &plan.rendered_body,
                    &reference_task_lines,
                )?;
            refresh_stable_rendered_note(plan);
        }
        plan.routed_intents = intents.into_values().collect();
    }

    Ok(())
}

/// Shared full-scan branch: a dirty plan becomes a whole-PDF hold without
/// entering the executor or reserving any run-state dedupe entries.
pub(super) fn execute_scan_plan(
    config: &Config,
    plan: &PdfSyncPlan,
    dirty_preflight: &ScanDirtyPreflight,
    run_state: &mut RunWriteState,
    session: &mut MarkerWriteSession,
) -> Result<PdfSyncOutcome> {
    if let Some(targets) = dirty_preflight.held_targets_by_pdf.get(&plan.pdf) {
        return Ok(PdfSyncOutcome::Held(dirty_targets_hold(
            config, plan, targets,
        )));
    }
    execute_pdf_sync(config, plan, run_state, session)
}

/// Retarget a v2 unqualified follow-up (local `ReferenceNote`) to the live
/// reading task's residence, or to `mac_inbox` when there is none — never an
/// archive. Explicit `@name` routes keep their behavior. Returns the
/// (possibly retargeted) candidate and whether it takes the v2 default
/// insertion path.
fn retarget_v2_follow_up(
    config: &Config,
    residence: Option<&str>,
    mut candidate: AnnotationTaskCandidate,
) -> (AnnotationTaskCandidate, bool) {
    if !matches!(candidate.target, AnnotationTaskTarget::ReferenceNote) {
        return (candidate, false);
    }
    let route = match residence {
        Some(route) if !route.is_empty() && !route.starts_with("done/") => {
            route.to_string()
        }
        _ => FALLBACK_PARENT_ROUTE.to_string(),
    };
    candidate.target = AnnotationTaskTarget::RoutedNote(
        config.bob_dir.join(format!("{route}.md")),
    );
    (candidate, true)
}

impl ProcessedTaskIndex {
    pub(super) fn accept(
        &mut self,
        candidate: &AnnotationTaskCandidate,
    ) -> bool {
        let legacy_source_task_anchor =
            annotation_task_legacy_source_task_block_id(candidate);
        if self
            .legacy_source_task_anchors
            .contains(&legacy_source_task_anchor)
            || self.processed_ids.contains(&candidate.processed_id)
            || self.legacy_identities.contains(&candidate.identity)
        {
            return false;
        }

        self.legacy_source_task_anchors
            .insert(legacy_source_task_anchor);
        self.processed_ids.insert(candidate.processed_id.clone());
        self.legacy_identities.insert(candidate.identity.clone());
        true
    }
}

pub(super) fn refresh_stable_rendered_note(plan: &mut PdfSyncPlan) {
    plan.stable_rendered_note = plan.note.render_with_projection(
        &plan.synced_projection,
        &plan.synced_hash,
        plan.replaced_hash.as_deref(),
        &plan.stable_metadata,
        &plan.rendered_body,
    );
    plan.stable_note_action = change_action(
        plan.note.exists(),
        plan.note.contents().as_deref(),
        &plan.stable_rendered_note,
    );
}

pub(super) fn execute_pdf_sync_inner(
    config: &Config,
    plan: &PdfSyncPlan,
    run_state: &mut RunWriteState,
    marker_prep: Option<&ResolvedMarkerWrite>,
) -> Result<SyncWriteReport> {
    let is_v2 = plan
        .reading_task_plan
        .as_ref()
        .is_some_and(|reading| reading.branch == NoteBranch::V2);
    if is_v2 {
        execute_pdf_sync_v2(config, plan, run_state, marker_prep)
    } else {
        execute_pdf_sync_v1(config, plan, run_state, marker_prep)
    }
}

fn execute_pdf_sync_v1(
    config: &Config,
    plan: &PdfSyncPlan,
    run_state: &mut RunWriteState,
    marker_prep: Option<&ResolvedMarkerWrite>,
) -> Result<SyncWriteReport> {
    if note_write_planned(plan) {
        ensure_note_unchanged_for_write(plan)?;
    }
    for write in &plan.routed_task_note_writes {
        if write.action != "none" {
            ensure_routed_note_unchanged_for_write(write)?;
        }
    }
    if plan.marker_write_needed {
        write_planned_pdf_marker(plan, marker_prep)?;
    }

    let mut image_assets_written = 0usize;
    let mut image_assets_skipped = 0usize;
    for write in &plan.image_assets {
        if execute_image_asset_write(write)? {
            image_assets_written += 1;
        } else {
            image_assets_skipped += 1;
        }
    }

    let refresh_synced_at =
        plan.stable_note_action != "none" && plan.rendered_highlights.is_some();
    let refresh_metadata = plan.marker_write_needed || refresh_synced_at;
    // New notes always rerender here with a fresh creation timestamp taken
    // immediately before the atomic write, so the persisted `created` value
    // reflects the actual writing invocation (including sidecar-free and
    // marker-write-free creations) rather than an earlier planning preview.
    let is_new_note = !plan.note.exists();
    let rendered_note = if refresh_metadata || is_new_note {
        // `write_pdf_marker` rewrites the PDF in place, so the planning-time
        // hash is stale; rehash the file to record the post-write digest.
        // When no marker write happened the PDF is untouched, so the hash the
        // planner already computed from the same bytes is reused for free.
        let source_pdf_sha256 = if plan.marker_write_needed {
            sha256_file(&plan.pdf)?
        } else {
            plan.marker.source_pdf_sha256.clone()
        };
        let mut metadata = pipeline_metadata(
            config,
            &plan.pdf,
            &source_pdf_sha256,
            &plan.note,
            plan.sidecar.as_ref(),
            plan.rendered_highlights.as_ref(),
            refresh_synced_at,
        )?;
        if is_new_note {
            metadata.created = Some(new_note_created_timestamp());
        }
        carry_birth_only_metadata(&mut metadata, &plan.stable_metadata);
        plan.note.render_with_projection(
            &plan.synced_projection,
            &plan.synced_hash,
            plan.replaced_hash.as_deref(),
            &metadata,
            &plan.rendered_body,
        )
    } else {
        plan.stable_rendered_note.clone()
    };
    let note_action = change_action(
        plan.note.exists(),
        plan.note.contents().as_deref(),
        &rendered_note,
    );
    if plan.note.contents().as_deref() != Some(rendered_note.as_str()) {
        ensure_note_unchanged_for_write(plan)?;
        atomic_write(&plan.note_path, &rendered_note)?;
    }
    let mut routed_note_actions = 0usize;
    for write in &plan.routed_task_note_writes {
        if write.action == "none" {
            continue;
        }
        ensure_routed_note_unchanged_for_write(write)?;
        atomic_write(&write.path, &write.rendered_contents)?;
        routed_note_actions += 1;
    }
    // Insertion intentions rebase against fresh bytes at execution so mixed
    // v1/v2 actions sharing a destination never overwrite one another.
    // v1 links the fixed `^ref` tracker when present; otherwise tasks are
    // written exactly as today with no LINKS line.
    let v1_ref_task = if plan.annotation_task_relation.is_some() {
        match parse_pdf_task_line(&plan.rendered_body) {
            Ok(PdfTaskLineState::Present(_)) => {
                Some((plan.note_path.clone(), "ref".to_string()))
            }
            _ => None,
        }
    } else {
        None
    };
    let intent_outcome =
        execute_routed_intents(config, plan, run_state, v1_ref_task)?;
    routed_note_actions += intent_outcome.note_actions;

    Ok(SyncWriteReport {
        note_action,
        marker_action: if plan.marker_write_needed {
            "updated"
        } else {
            "none"
        },
        image_count: plan
            .rendered_highlights
            .as_ref()
            .map(|rendered| rendered.image_count)
            .unwrap_or(0),
        image_note_count: plan
            .rendered_highlights
            .as_ref()
            .map(|rendered| rendered.image_note_count)
            .unwrap_or(0),
        image_notes_unresolved: plan
            .rendered_highlights
            .as_ref()
            .map(|rendered| rendered.image_notes_unresolved.clone())
            .unwrap_or_default(),
        continuation_count: plan
            .rendered_highlights
            .as_ref()
            .map(|rendered| rendered.continuation_count)
            .unwrap_or(0),
        dangling_continuations: plan
            .rendered_highlights
            .as_ref()
            .map(|rendered| rendered.dangling_continuations.clone())
            .unwrap_or_default(),
        underlines_merged: plan
            .rendered_highlights
            .as_ref()
            .map(|rendered| rendered.underlines_merged)
            .unwrap_or(0),
        standalone_underlines: plan
            .rendered_highlights
            .as_ref()
            .map(|rendered| rendered.standalone_underlines.clone())
            .unwrap_or_default(),
        section_diagnostics: plan
            .rendered_highlights
            .as_ref()
            .map(|rendered| rendered.section_diagnostics.clone())
            .unwrap_or_default(),
        section_count: plan
            .rendered_highlights
            .as_ref()
            .map(|rendered| rendered.sections.len())
            .unwrap_or(0),
        image_assets_written,
        image_assets_skipped,
        routed_note_actions,
        annotation_tasks_created: plan
            .annotation_tasks_created
            .saturating_sub(intent_outcome.deduped),
        annotation_tasks_skipped: plan
            .annotation_tasks_skipped
            .saturating_add(intent_outcome.deduped),
        // The v1 path never touches reading tasks; v2 outcomes live in
        // `execute_pdf_sync_v2`.
        reading_created: None,
        reading_updated: None,
        marker_write: None,
    })
}

/// Outcome of executing one plan's routed insertion intentions.
struct IntentOutcome {
    note_actions: usize,
    /// Planned lines skipped at execution (already present or just written).
    deduped: usize,
}

/// Lazily built vault note index for cross-note relation links, cached
/// on the run state and built at most once per run, only when a
/// cross-note link is actually rendered.
pub(super) fn routed_note_index<'a>(
    config: &Config,
    run_state: &'a mut RunWriteState,
) -> &'a crate::native::vault_links::NoteIndex {
    if run_state.note_index.is_none() {
        run_state.note_index = Some(build_vault_note_index(&config.bob_dir));
    }
    run_state
        .note_index
        .as_ref()
        .expect("note index just built")
}

pub(super) fn build_vault_note_index(
    bob_dir: &Path,
) -> crate::native::vault_links::NoteIndex {
    let mut relative = Vec::new();
    collect_markdown_relative(bob_dir, bob_dir, &mut relative);
    crate::native::vault_links::NoteIndex::from_paths(relative)
}

pub(super) fn collect_markdown_relative(
    root: &Path,
    directory: &Path,
    out: &mut Vec<PathBuf>,
) {
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    let mut entries =
        entries.filter_map(|entry| entry.ok()).collect::<Vec<_>>();
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let path = entry.path();
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_dir() {
            if entry
                .file_name()
                .to_str()
                .is_some_and(|name| name.starts_with('.'))
            {
                continue;
            }
            collect_markdown_relative(root, &path, out);
        } else if file_type.is_file()
            && path
                .extension()
                .and_then(|ext| ext.to_str())
                .is_some_and(|ext| ext.eq_ignore_ascii_case("md"))
            && let Ok(rel) = path.strip_prefix(root)
        {
            out.push(rel.to_path_buf());
        }
    }
}

/// Shortest link form for an annotation-task relation target, following
/// §5: `[[#^id]]` in the same note, otherwise `canonical_link` (basename
/// when unique, else full vault-relative path; archives keep `done/…`).
pub(super) fn annotation_relation_link(
    config: &Config,
    dest_abs: &Path,
    target_abs: &Path,
    block_id: &str,
    run_state: &mut RunWriteState,
) -> String {
    let dest_rel = dest_abs
        .strip_prefix(&config.bob_dir)
        .map(Path::to_path_buf)
        .unwrap_or_else(|_| dest_abs.to_path_buf());
    let target_rel = target_abs
        .strip_prefix(&config.bob_dir)
        .map(Path::to_path_buf)
        .unwrap_or_else(|_| target_abs.to_path_buf());
    if dest_rel == target_rel {
        return format!("[[#^{block_id}]]");
    }
    let index = routed_note_index(config, run_state);
    crate::native::task_dependencies::format::canonical_link(
        &target_rel,
        block_id,
        &dest_rel,
        index,
    )
}

/// Execute routed insertion intentions by rereading each destination and
/// rebasing through capture's preimage-checked writer: v2 defaults use
/// capture's Tasks-section insertion (creating a missing inbox), explicit
/// routes keep the legacy append behavior. Lines already present on disk or
/// written earlier in this run are skipped and counted, never duplicated.
/// Dedup keys commit only after their destination write succeeds (or fresh
/// disk evidence proves the task already exists), so a failed destination
/// never consumes a later PDF's otherwise-identical follow-up while
/// successful writes stay deduped even if a later ref-note write fails.
/// Bounded retries cover only true preimage mismatches, rebuilding against
/// fresh bytes; arbitrary IO errors propagate without retry.
///
/// `ref_task` is the resolved reading-task address the new LINKS lines
/// point at: v1 `(ref note path, "ref")` when the tracker is present, v2
/// `reading.embed` mapped back to an absolute note path. `None` writes
/// tasks exactly as today, with no LINKS line.
fn execute_routed_intents(
    config: &Config,
    plan: &PdfSyncPlan,
    run_state: &mut RunWriteState,
    ref_task: Option<(PathBuf, String)>,
) -> Result<IntentOutcome> {
    let mut outcome = IntentOutcome {
        note_actions: 0,
        deduped: 0,
    };
    for intent in &plan.routed_intents {
        let label = intent
            .annotation_task_relation
            .as_deref()
            .or(plan.annotation_task_relation.as_deref());
        // The link is identical for every task in one destination, so
        // resolve it once per attempt (after dedup proves at least one
        // task still needs it). Cross-note targets build the vault index
        // lazily, only when actually needed.
        let mut attempts = 0usize;
        loop {
            attempts += 1;
            let current = match fs::read_to_string(&intent.path) {
                Ok(contents) => Some(contents),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    if intent.is_v2_default {
                        None
                    } else {
                        return Err(CommandError::new(format!(
                            "read routed task note {}: {error}",
                            intent.path.display()
                        )));
                    }
                }
                Err(error) => {
                    return Err(CommandError::new(format!(
                        "read routed task note {}: {error}",
                        intent.path.display()
                    )));
                }
            };
            // Blocks preserve each task's LINKS child: v2 defaults join
            // the block with `\n` for `insert_task_line` (which converts
            // to CRLF), explicit routes flatten into `append_task_lines`
            // (which must never see an embedded `\n`).
            let mut blocks: Vec<Vec<String>> = Vec::new();
            let mut missing_keys = Vec::new();
            // Cache the resolved link text for this destination attempt.
            let mut cached_link: Option<Option<String>> = None;
            for (line, candidate) in
                intent.lines.iter().zip(intent.candidates.iter())
            {
                let anchor =
                    annotation_task_legacy_source_task_block_id(candidate);
                if run_state.processed_ids.contains(&candidate.processed_id)
                    || run_state.legacy_identities.contains(&candidate.identity)
                    || run_state.source_anchors.contains(&anchor)
                {
                    outcome.deduped += 1;
                    continue;
                }
                if let Some(contents) = current.as_deref()
                    && (contents.contains(candidate.processed_id.as_str())
                        || contents.contains(candidate.identity.as_str())
                        || contents.contains(anchor.as_str()))
                {
                    outcome.deduped += 1;
                    continue;
                }
                let mut block = vec![line.clone()];
                if let (Some(want), Some((target_abs, block_id))) =
                    (label, ref_task.as_ref())
                {
                    let link = match cached_link.clone() {
                        Some(cached) => cached,
                        None => {
                            let rendered = Some(annotation_relation_link(
                                config,
                                &intent.path,
                                target_abs,
                                block_id,
                                run_state,
                            ));
                            cached_link = Some(rendered.clone());
                            rendered
                        }
                    };
                    if let Some(link) = link {
                        block.push(
                            crate::native::task_relations::format::format_links_line(
                                "\t",
                                &[(link, Some(want.to_string()))],
                            ),
                        );
                    }
                }
                blocks.push(block);
                missing_keys.push((
                    candidate.processed_id.clone(),
                    candidate.identity.clone(),
                    anchor,
                ));
            }
            if blocks.is_empty() {
                break;
            }
            let existed = current.is_some();
            let base = current.as_deref().unwrap_or("");
            let rendered = if intent.is_v2_default {
                let joined_blocks = blocks
                    .iter()
                    .map(|block| block.join("\n"))
                    .collect::<Vec<_>>();
                match current.as_deref() {
                    Some(contents) => {
                        let mut updated = contents.to_string();
                        for block in &joined_blocks {
                            let (next, _) =
                                crate::native::capture::insert_task_line(
                                    &updated, block,
                                );
                            updated = next;
                        }
                        updated
                    }
                    None => {
                        let mut rendered = joined_blocks.join("\n");
                        rendered.push('\n');
                        rendered
                    }
                }
            } else {
                let flat = blocks.into_iter().flatten().collect::<Vec<_>>();
                append_task_lines(base, &flat)
            };
            // A concurrent write that already contains our lines is a no-op.
            if current.as_deref() == Some(rendered.as_str()) {
                break;
            }
            let staged = crate::native::capture::StagedTextFile {
                target: intent.path.clone(),
                target_existed: existed,
                original_target: current.clone().unwrap_or_default(),
                updated_target: rendered,
            };
            match crate::native::capture::write_staged_files(
                std::slice::from_ref(&staged),
            ) {
                Ok(()) => {
                    for (processed_id, identity, anchor) in missing_keys {
                        run_state.processed_ids.insert(processed_id);
                        run_state.legacy_identities.insert(identity);
                        run_state.source_anchors.insert(anchor);
                    }
                    outcome.note_actions += 1;
                    break;
                }
                Err(error) => {
                    let msg = error.message.clone();
                    if is_routed_preimage_mismatch(&msg) && attempts < 3 {
                        continue;
                    }
                    return Err(CommandError::new(msg));
                }
            }
        }
    }
    Ok(outcome)
}

/// Read-only asset preconditions before any v2 writes: an existing image
/// destination with different bytes refuses here rather than after the
/// reading task or routed follow-ups have already written.
fn validate_v2_asset_preconditions(plan: &PdfSyncPlan) -> Result<()> {
    for write in &plan.image_assets {
        if let Ok(bytes) = fs::read(&write.dest_path) {
            let dest_sha256 = hex::encode(Sha256::digest(bytes));
            if dest_sha256 != write.source_sha256 {
                return Err(CommandError::new(format!(
                    "image asset destination exists with different bytes: {}",
                    write.dest_path.display()
                )));
            }
        }
    }
    Ok(())
}

/// True for a genuine staged-writer preimage race; all other IO errors
/// propagate without retry.
fn is_routed_preimage_mismatch(msg: &str) -> bool {
    msg.contains("changed on disk after planning")
        || msg.contains("created on disk after planning")
        || msg.contains("deleted on disk after planning")
}

/// Per-PDF v2 execution: destination reading-task and routed insertion
/// actions first, then the authorized marker write, then the ref note with
/// the actual final ID and refreshed metadata. A changed reading line fails
/// before any of this PDF's writes begin; later marker/ref failures rerun
/// into adoption without duplication.
fn execute_pdf_sync_v2(
    config: &Config,
    plan: &PdfSyncPlan,
    run_state: &mut RunWriteState,
    marker_prep: Option<&ResolvedMarkerWrite>,
) -> Result<SyncWriteReport> {
    // Non-mutating pre-validation before any writes: the selected reading
    // line, the required ref-note preimage, the authorized PDF preimage,
    // and asset preconditions. A changed/deleted/ambiguous selected line
    // fails here with the reread error before any destination, marker, or
    // ref write begins. Immediate revalidation still runs at each write
    // boundary below.
    if let Some(located) = plan.reading_located.as_ref() {
        revalidate_located_task(&config.bob_dir, located)?;
    }
    if note_write_planned(plan) {
        ensure_note_unchanged_for_write(plan)?;
    }
    if plan.marker_write_needed && marker_prep.is_none() {
        ensure_pdf_unchanged_for_write(plan)?;
    }
    validate_v2_asset_preconditions(plan)?;
    // The reading-task action validates (and writes) first: a moved or
    // altered task fails here with the reading-task-changed error before the
    // marker, assets, routed follow-ups, or ref note are touched.
    let reading = execute_v2_reading_action(config, plan)?;
    if note_write_planned(plan) {
        ensure_note_unchanged_for_write(plan)?;
    }
    // v2 links the reading task's final address after the reading-task
    // action (a new birth may land past the planning preview id; an
    // archived task keeps its `done/…` path). No embed means no ref task:
    // write the task exactly as today, with no LINKS line. Never emit a
    // link to an id that will not be on disk after the write.
    let v2_ref_task = reading.embed.clone().map(|(target, block_id)| {
        (config.bob_dir.join(format!("{target}.md")), block_id)
    });
    let intent_outcome =
        execute_routed_intents(config, plan, run_state, v2_ref_task)?;
    if plan.marker_write_needed {
        write_planned_pdf_marker(plan, marker_prep)?;
    }

    let mut image_assets_written = 0usize;
    let mut image_assets_skipped = 0usize;
    for write in &plan.image_assets {
        if execute_image_asset_write(write)? {
            image_assets_written += 1;
        } else {
            image_assets_skipped += 1;
        }
    }

    let refresh_synced_at =
        plan.stable_note_action != "none" && plan.rendered_highlights.is_some();
    let refresh_metadata = plan.marker_write_needed || refresh_synced_at;
    let is_new_note = !plan.note.exists();
    // Re-render the body with the actual execution result when allocation
    // or revalidation moved it past the planning preview.
    let execution_body = v2_execution_body(plan, &reading);
    let rendered_note = if refresh_metadata
        || is_new_note
        || execution_body.is_some()
    {
        let source_pdf_sha256 = if plan.marker_write_needed {
            sha256_file(&plan.pdf)?
        } else {
            plan.marker.source_pdf_sha256.clone()
        };
        let mut metadata = pipeline_metadata(
            config,
            &plan.pdf,
            &source_pdf_sha256,
            &plan.note,
            plan.sidecar.as_ref(),
            plan.rendered_highlights.as_ref(),
            refresh_synced_at,
        )?;
        if is_new_note {
            metadata.created = Some(new_note_created_timestamp());
        }
        let mut render_projection = plan.synced_projection.clone();
        if let Some(route) = reading
            .residence
            .as_deref()
            .or(plan.v2_residence.as_deref())
        {
            render_projection.insert(
                FIELD_PARENT.to_string(),
                residence_parent_value(route),
            );
        } else if let Some(parent) = plan.note.frontmatter_value(FIELD_PARENT) {
            render_projection.insert(FIELD_PARENT.to_string(), parent);
        }
        // Newborn reread lineage and the saved-source pointer are
        // birth-only plan state: the execution re-render must carry the
        // same `earlier:` and `source_markdown:` the dry-run preview
        // showed, so repeat syncs preserve them byte-for-byte.
        carry_birth_only_metadata(&mut metadata, &plan.stable_metadata);
        let body = execution_body.as_deref().unwrap_or(&plan.rendered_body);
        let rendered = plan.note.render_with_projection(
            &render_projection,
            &plan.synced_hash,
            plan.replaced_hash.as_deref(),
            &metadata,
            body,
        );
        restamp_parent_free_base(&rendered, &plan.synced_projection)
    } else {
        plan.stable_rendered_note.clone()
    };
    let note_action = change_action(
        plan.note.exists(),
        plan.note.contents().as_deref(),
        &rendered_note,
    );
    if plan.note.contents().as_deref() != Some(rendered_note.as_str()) {
        ensure_note_unchanged_for_write(plan)?;
        atomic_write(&plan.note_path, &rendered_note)?;
    }

    // Actual successful reading-task actions only: an insert (birth or
    // reopen) records its actual destination and final ID, a line edit
    // records the edited line, and adoption/unchanged work records
    // nothing. Failures never reach this report.
    let mut reading_created = None;
    let mut reading_updated = None;
    if let Some(execution) = reading.execution.as_ref() {
        let outcome = ReadingTaskOutcome {
            dest: vault_rel_forward(config, &execution.destination),
            id: execution.block_id.clone(),
        };
        match execution.action {
            ReadingTaskExecutionAction::Inserted => {
                reading_created = Some(outcome);
            }
            ReadingTaskExecutionAction::LineEdited => {
                reading_updated = Some(outcome);
            }
        }
    }
    Ok(SyncWriteReport {
        note_action,
        marker_action: if plan.marker_write_needed {
            "updated"
        } else {
            "none"
        },
        image_count: plan
            .rendered_highlights
            .as_ref()
            .map(|rendered| rendered.image_count)
            .unwrap_or(0),
        image_note_count: plan
            .rendered_highlights
            .as_ref()
            .map(|rendered| rendered.image_note_count)
            .unwrap_or(0),
        image_notes_unresolved: plan
            .rendered_highlights
            .as_ref()
            .map(|rendered| rendered.image_notes_unresolved.clone())
            .unwrap_or_default(),
        continuation_count: plan
            .rendered_highlights
            .as_ref()
            .map(|rendered| rendered.continuation_count)
            .unwrap_or(0),
        dangling_continuations: plan
            .rendered_highlights
            .as_ref()
            .map(|rendered| rendered.dangling_continuations.clone())
            .unwrap_or_default(),
        underlines_merged: plan
            .rendered_highlights
            .as_ref()
            .map(|rendered| rendered.underlines_merged)
            .unwrap_or(0),
        standalone_underlines: plan
            .rendered_highlights
            .as_ref()
            .map(|rendered| rendered.standalone_underlines.clone())
            .unwrap_or_default(),
        section_diagnostics: plan
            .rendered_highlights
            .as_ref()
            .map(|rendered| rendered.section_diagnostics.clone())
            .unwrap_or_default(),
        section_count: plan
            .rendered_highlights
            .as_ref()
            .map(|rendered| rendered.sections.len())
            .unwrap_or(0),
        image_assets_written,
        image_assets_skipped,
        routed_note_actions: intent_outcome.note_actions,
        annotation_tasks_created: plan
            .annotation_tasks_created
            .saturating_sub(intent_outcome.deduped),
        annotation_tasks_skipped: plan
            .annotation_tasks_skipped
            .saturating_add(intent_outcome.deduped),
        reading_created,
        reading_updated,
        marker_write: None,
    })
}

/// What the v2 reading-task action did for this PDF's note rendering.
struct V2ReadingOutcome {
    execution: Option<ReadingTaskExecution>,
    residence: Option<String>,
    embed: Option<(String, String)>,
}

/// Execute one PDF's v2 reading-task action. Inserts allocate against fresh
/// destination bytes; line edits reread immediately before writing; adopted
/// or otherwise selected tasks revalidate even when no edit is needed so a
/// moved task never produces a stale healed embed or parent.
fn execute_v2_reading_action(
    config: &Config,
    plan: &PdfSyncPlan,
) -> Result<V2ReadingOutcome> {
    let reading = plan.reading_task_plan.as_ref().expect("v2 plan");
    let blank = V2ReadingOutcome {
        execution: None,
        residence: plan.v2_residence.clone(),
        embed: plan_embed_pair(plan),
    };
    match &reading.action {
        ReadingTaskAction::NoWrite => {
            let Some(located) = plan.reading_located.as_ref() else {
                return Ok(blank);
            };
            let refreshed = revalidate_located_task(&config.bob_dir, located)?;
            let embed = refreshed
                .block_id
                .clone()
                .map(|block_id| (managed_embed_target(&refreshed), block_id));
            let residence =
                refreshed.residence.clone().or(plan.v2_residence.clone());
            Ok(V2ReadingOutcome {
                execution: None,
                residence,
                embed: embed.or(blank.embed),
            })
        }
        ReadingTaskAction::Insert {
            destination_route,
            warning_child,
            mark,
            prefer_block_id,
            reading,
            ..
        } => {
            let ref_rel = vault_rel_forward(config, &plan.note_path);
            let ref_target = ref_rel
                .strip_suffix(".md")
                .or_else(|| ref_rel.strip_suffix(".MD"))
                .unwrap_or(&ref_rel)
                .to_string();
            let ref_stem = plan
                .note_path
                .file_stem()
                .and_then(|stem| stem.to_str())
                .unwrap_or("ref")
                .to_string();
            let title = note_title(&plan.pdf, &plan.synced_projection);
            // Birth dates honor `BOB_NOW` so fixed-clock fixtures stay
            // deterministic; the annotation-task clock is unchanged.
            let created = bob_env::current_datetime()
                .date()
                .format("%Y-%m-%d")
                .to_string();
            let execution = execute_reading_insert(ReadingInsertInputs {
                bob_dir: &config.bob_dir,
                destination_route,
                ref_target: &ref_target,
                ref_stem: &ref_stem,
                reading: reading.clone(),
                title_raw: &title,
                created: &created,
                mark: *mark,
                warning_child: warning_child.clone(),
                prefer_block_id: prefer_block_id.as_deref(),
            })?;
            let target = execution
                .destination
                .strip_prefix(&config.bob_dir)
                .map(|rel| rel.to_string_lossy().replace('\\', "/"))
                .unwrap_or_else(|_| destination_route.clone());
            let target = target
                .strip_suffix(".md")
                .or_else(|| target.strip_suffix(".MD"))
                .unwrap_or(&target)
                .to_string();
            let embed = (target, execution.block_id.clone());
            Ok(V2ReadingOutcome {
                execution: Some(execution),
                residence: Some(destination_route.clone()),
                embed: Some(embed),
            })
        }
        ReadingTaskAction::LineEdit { target_mark } => {
            let Some(located) = plan.reading_located.as_ref() else {
                return Err(CommandError::new(
                    "reading task line edit planned without a located task; rerun",
                ));
            };
            let execution = execute_reading_line_edit(
                &config.bob_dir,
                located,
                *target_mark,
            )?;
            Ok(V2ReadingOutcome {
                execution: Some(execution),
                residence: plan.v2_residence.clone(),
                embed: blank.embed,
            })
        }
    }
}

/// Planned embed as a `(target, id)` pair for execution comparison.
fn plan_embed_pair(plan: &PdfSyncPlan) -> Option<(String, String)> {
    plan.reading_task_plan
        .as_ref()?
        .embed
        .as_ref()
        .map(|embed| (embed.target.clone(), embed.block_id.clone()))
}

/// Rebuilt body when execution moved past the planning preview: a fresh
/// insert's actual ID, or a revalidated task's refreshed address. `None`
/// keeps the planned body.
fn v2_execution_body(
    plan: &PdfSyncPlan,
    reading: &V2ReadingOutcome,
) -> Option<String> {
    let planned = plan_embed_pair(plan);
    let actual = reading.embed.clone()?;
    if Some(&actual) == planned.as_ref() {
        return None;
    }
    let (planned_target, planned_id) =
        planned.unwrap_or_else(|| actual.clone());
    let mut body = plan.rendered_body.clone();
    let old_embed =
        ref_tasks_mod::managed_embed_line(&planned_target, &planned_id);
    let new_embed = ref_tasks_mod::managed_embed_line(&actual.0, &actual.1);
    if body.contains(old_embed.as_str()) {
        body = body.replace(old_embed.as_str(), new_embed.as_str());
        return Some(body);
    }
    // Fall back to healing the slot when the preview left no exact line.
    Some(heal_managed_embed(&body, &actual.0, &actual.1))
}

pub(super) fn execute_image_asset_write(
    write: &ImageAssetWrite,
) -> Result<bool> {
    match fs::read(&write.dest_path) {
        Ok(bytes) => {
            let dest_sha256 = hex::encode(Sha256::digest(bytes));
            if dest_sha256 == write.source_sha256 {
                return Ok(false);
            }
            return Err(CommandError::new(format!(
                "image asset destination exists with different bytes: {}",
                write.dest_path.display()
            )));
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(CommandError::new(format!(
                "read image asset destination {}: {error}",
                write.dest_path.display()
            )));
        }
    }

    atomic_copy(&write.source_path, &write.dest_path)?;
    Ok(true)
}

pub(super) fn ensure_note_unchanged_for_write(
    plan: &PdfSyncPlan,
) -> Result<()> {
    if note_contents_match_plan(
        &plan.note_path,
        plan.note.contents().as_deref(),
    )? {
        Ok(())
    } else {
        Err(CommandError::new(format!(
            "reference note changed during sync; rerun: {}",
            plan.note_path.display()
        )))
    }
}

pub(super) fn ensure_routed_note_unchanged_for_write(
    write: &RoutedTaskNoteWrite,
) -> Result<()> {
    if note_contents_match_plan(&write.path, Some(&write.original_contents))? {
        Ok(())
    } else {
        Err(CommandError::new(format!(
            "routed task note changed during sync; rerun: {}",
            write.path.display()
        )))
    }
}

pub(super) fn note_contents_match_plan(
    path: &Path,
    expected: Option<&str>,
) -> Result<bool> {
    match (expected, fs::read_to_string(path)) {
        (Some(expected), Ok(current)) => Ok(current == expected),
        (None, Err(error)) if error.kind() == std::io::ErrorKind::NotFound => {
            Ok(true)
        }
        (None, Ok(_)) => Ok(false),
        (Some(_), Err(error))
            if error.kind() == std::io::ErrorKind::NotFound =>
        {
            Ok(false)
        }
        (_, Err(error)) => Err(CommandError::new(format!(
            "read note {} before write: {error}",
            path.display()
        ))),
    }
}

pub(super) fn ensure_pdf_unchanged_for_write(plan: &PdfSyncPlan) -> Result<()> {
    let current_hash = sha256_file(&plan.pdf)?;
    if current_hash == plan.marker.source_pdf_sha256 {
        Ok(())
    } else {
        Err(CommandError::new(format!(
            "PDF changed during sync; rerun: {}",
            plan.pdf.display()
        )))
    }
}
