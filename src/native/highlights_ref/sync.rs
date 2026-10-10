//! PDF sync planning and execution.
use super::*;
use crate::native::parent_notes as parent_notes_mod;
use crate::native::ref_tasks as ref_tasks_mod;

pub(super) fn sync_pdf(
    config: &Config,
    pdf: &Path,
    options: SyncOptions,
) -> Result<()> {
    let ctx = ScanContext::build(config);
    let mut plan = plan_pdf_sync(config, pdf, options, &ctx)?;
    let mut plans_to_finalize = [&mut plan];
    finalize_annotation_task_plans(config, &mut plans_to_finalize, &ctx)?;
    reserve_preview_ids(config, &mut plans_to_finalize, &ctx);
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

    ensure_safe_to_write(config, iter::once(&plan))?;
    let mut run_state = RunWriteState::default();
    let report = execute_pdf_sync(config, &plan, &mut run_state)?;
    print_sync_write_report(&report);
    Ok(())
}

pub(super) fn scan_library(
    config: &Config,
    options: SyncOptions,
    jobs: usize,
    verbose: bool,
    no_hooks: bool,
) -> Result<()> {
    validate_library_layout(config)?;
    let _scan_lock = acquire_scan_writer_lock(options.dry_run)?;
    let pre_scan_hook = configured_pre_scan_hook(no_hooks)?;
    run_pre_scan_hook(config, pre_scan_hook.as_ref(), options.dry_run)?;
    let intake = plan_xlib_intake(config)?;
    if !options.dry_run {
        execute_xlib_intake(&intake)?;
    }
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
    finalize_annotation_task_plans(config, &mut plans_to_finalize, &ctx)?;
    reserve_preview_ids(config, &mut plans_to_finalize, &ctx);
    let plans = plan_outcomes
        .iter()
        .filter_map(|outcome| match outcome {
            ScanPlanOutcome::Planned(plan) => Some(plan.as_ref()),
            ScanPlanOutcome::Failed(_) => None,
        })
        .collect::<Vec<_>>();
    validate_planned_asset_collisions(&plans)?;
    let plan_failures = plan_outcomes
        .iter()
        .filter_map(|outcome| match outcome {
            ScanPlanOutcome::Planned(_) => None,
            ScanPlanOutcome::Failed(failure) => Some(failure),
        })
        .collect::<Vec<_>>();

    let styler = Styler::detect();
    if verbose {
        print_verbose_scan_plan_report(
            config,
            options,
            pdfs.len(),
            &intake,
            &plan_outcomes,
        );
    } else {
        print_concise_intake_report(config, &intake, options.dry_run, &styler);
        print_scan_header(config, pdfs.len(), options.dry_run, &styler);
    }

    if options.dry_run {
        if verbose {
            print_scan_plan_summary(&plans, plan_failures.len());
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
        return if plan_failures.is_empty() {
            Ok(())
        } else {
            Err(scan_partial_failure_error(&plan_failures, &[]))
        };
    }

    ensure_safe_to_write(config, plans.iter().copied())?;
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
    let write_failures = write_outcomes
        .iter()
        .filter_map(|outcome| match outcome {
            ScanWriteOutcome::Written(_) => None,
            ScanWriteOutcome::Failed(failure) => Some(failure),
        })
        .collect::<Vec<_>>();
    if verbose {
        for failure in &write_failures {
            print_scan_write_failure_entry(failure);
        }
        print_scan_write_summary(
            &reports,
            &plans,
            plan_failures.len(),
            write_failures.len(),
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
) -> Vec<Result<PdfSyncPlan>> {
    if jobs <= 1 || pdfs.len() <= 1 {
        return pdfs
            .iter()
            .map(|pdf| plan_pdf_sync(config, pdf, options, ctx))
            .collect();
    }

    let next = AtomicUsize::new(0);
    let worker_count = jobs.min(pdfs.len());
    let mut indexed: Vec<(usize, Result<PdfSyncPlan>)> =
        thread::scope(|scope| {
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
                                plan_pdf_sync(config, pdf, options, ctx),
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

/// Vault-relative forward-slash path for locator-index lookups.
fn vault_rel_forward(config: &Config, path: &Path) -> String {
    path.strip_prefix(&config.bob_dir)
        .map(|rel| rel.to_string_lossy().replace('\\', "/"))
        .unwrap_or_else(|_| path.to_string_lossy().replace('\\', "/"))
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
    let mut resolution = resolve_sync_projection(SyncInputs {
        last_hash: last_hash.as_deref(),
        base_projection: base_projection.as_ref(),
        marker_projection: &marker_projection,
        marker_hash: &marker_hash,
        frontmatter_projection: &frontmatter_projection,
        frontmatter_hash: &frontmatter_hash,
        note_exists: note.exists(),
        prefer: options.prefer,
    })?;
    // Capture intake eligibility before the `^ref` task signal so the final
    // closing run still imports pending annotation tasks (pre-signal `wip`),
    // then re-check after the signal so a reopen to `wip` also intakes in the
    // same run instead of waiting for the next pass.
    let intake_allowed_before_signal =
        projection_status_is(&resolution.projection, STATUS_WIP);
    let pdf_task_signal = apply_pdf_task_status_signal(
        &mut resolution,
        &pdf_task_line,
        base_projection.as_ref(),
        &marker_projection,
        &frontmatter_projection,
    )?;
    let annotation_task_intake_allowed = intake_allowed_before_signal
        || projection_status_is(&resolution.projection, STATUS_WIP);
    let decision = resolution.decision;
    let synced_projection = resolution.projection;
    validate_required_marker_keys(
        &synced_projection,
        decision.source.as_str(),
    )?;
    let synced_hash = projection_hash(&synced_projection)?;
    let rendered_marker = render_marker(&synced_projection)?;
    let marker_write_needed = (decision.frontmatter_contributed
        || status_normalization.marker.is_some())
        && normalize_line_endings(&rendered_marker)
            != normalize_line_endings(&marker.contents);

    if marker_write_needed && !options.write_pdf && !options.dry_run {
        return Err(CommandError::new(
            "reference note changed but --write-pdf was not supplied; refusing to update the PDF marker",
        ));
    }

    let sidecar = read_sidecar_for_pdf(pdf)?;
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
    let stable_metadata = pipeline_metadata(
        config,
        pdf,
        &marker.source_pdf_sha256,
        &note,
        sidecar.as_ref(),
        rendered_highlights.as_ref(),
        false,
    )?;
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
        )?
    } else {
        Vec::new()
    };
    let stable_rendered_note = note.render_with_projection(
        &synced_projection,
        &synced_hash,
        &stable_metadata,
        &rendered_body,
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
    let parent_hint = marker_projection
        .get(FIELD_PARENT)
        .and_then(MarkerValue::as_string)
        .unwrap_or("")
        .to_string();
    let frontmatter_input = note.synced_projection_with_normalization()?;
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
    let mut resolution = resolve_sync_projection(SyncInputs {
        last_hash: last_hash.as_deref(),
        base_projection: base_pf.as_ref(),
        marker_projection: &marker_pf,
        marker_hash: &marker_hash,
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
    let marker_status = status_of(&marker_pf);
    let frontmatter_status = status_of(&frontmatter_pf);
    // Preliminary selection to size the reopen fallback: when the selected
    // task is archived-terminal, its source residence decides the reopen
    // parent. The planner re-selects internally; this only feeds
    // `archive_source_open`.
    let preliminary = ref_tasks_mod::select_for_ref(&candidates, &v1_hits);
    let archive_source_open = match &preliminary.task {
        Some(ref_tasks_mod::Selected::V2(located)) if located.archived => {
            located.residence.as_deref().is_some_and(|route| {
                parent_notes_mod::resolve_parent(&config.bob_dir, route).is_ok()
            })
        }
        _ => false,
    };
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
        resolver: &|hint| {
            parent_notes_mod::resolve_parent(&config.bob_dir, hint)
        },
    })?;
    // Eligibility from the merged projection before the reading-task signal,
    // mirroring the v1 path: a WIP merged status before the signal keeps the
    // closing run's intake, and a WIP status after the signal keeps a reopen
    // intake in the same run.
    let pre_signal_wip =
        projection_status_is(&resolution.projection, STATUS_WIP);
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
    // Preserve annotation eligibility before and after the status signal so
    // closing and reopening runs import the same pending work as today.
    let annotation_task_intake_allowed = pre_signal_wip
        || projection_status_is(&resolution.projection, STATUS_WIP);
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
        return Err(CommandError::new(format!(
            "refusing to sync {}: {detail}",
            pdf.display()
        )));
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
        || status_normalization.marker.is_some())
        && normalize_line_endings(&semantic_rendered)
            != normalize_line_endings(&semantic_current);
    if semantic_needed && !options.write_pdf && !options.dry_run {
        return Err(CommandError::new(
            "reference note changed but --write-pdf was not supplied; refusing to update the PDF marker",
        ));
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
            if options.write_pdf || options.dry_run {
                (rendered, needed)
            } else {
                (marker.contents.clone(), false)
            }
        }
        None => (marker.contents.clone(), false),
    };
    let sidecar = read_sidecar_for_pdf(pdf)?;
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
    let stable_metadata = pipeline_metadata(
        config,
        pdf,
        &marker.source_pdf_sha256,
        &note,
        sidecar.as_ref(),
        rendered_highlights.as_ref(),
        false,
    )?;
    let highlights_content = rendered_highlights
        .as_ref()
        .map(|rendered| rendered.content.as_str())
        .unwrap_or("");
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
    let mut stable_rendered_note = note.render_with_projection(
        &render_projection,
        &synced_hash,
        &stable_metadata,
        &rendered_body,
    );
    stable_rendered_note =
        restamp_parent_free_base(&stable_rendered_note, &synced_pf);
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
        let (task, _) = select_birth_task(candidates);
        return match task {
            BirthTask::Adopt(located) => Some(located),
            BirthTask::Insert | BirthTask::Refused => None,
        };
    }
    match ref_tasks_mod::select_for_ref(candidates, v1_hits).task {
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
        let (destination_route, prefer_block_id) = match plan
            .reading_task_plan
            .as_ref()
            .map(|reading_plan| &reading_plan.action)
        {
            Some(ReadingTaskAction::Insert {
                destination_route,
                prefer_block_id,
                ..
            }) => (destination_route.clone(), prefer_block_id.clone()),
            _ => continue,
        };
        let destination =
            config.bob_dir.join(format!("{destination_route}.md"));
        let taken_set = taken.entry(destination.clone()).or_insert_with(|| {
            preview_taken_for_destination(&config.bob_dir, &destination)
        });
        let is_taken = |id: &str| taken_set.contains(id);
        let preview = match prefer_block_id.as_deref() {
            Some(preferred) if !preferred.is_empty() => {
                ref_tasks_mod::allocate_unique_block_id(preferred, &is_taken)
            }
            _ => {
                let stem = plan
                    .pdf
                    .file_stem()
                    .and_then(|stem| stem.to_str())
                    .unwrap_or("ref");
                ref_tasks_mod::allocate_ref_block_id(stem, &is_taken)
            }
        };
        taken_set.insert(preview.clone());
        // Patch the planning placeholder (fresh `ref-<slug>` or the reopen
        // old ID) to the reserved preview so human display names the ID.
        // Execution reallocates against fresh disk and reports the actual.
        let placeholder = match prefer_block_id.as_deref() {
            Some(preferred) if !preferred.is_empty() => preferred.to_string(),
            _ => {
                let stem = plan
                    .pdf
                    .file_stem()
                    .and_then(|stem| stem.to_str())
                    .unwrap_or("ref");
                format!("ref-{}", ref_tasks_mod::slug_ref_stem(stem))
            }
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
        if trimmed.starts_with("done_tasks:") {
            if let Some(start) = trimmed.find("[[") {
                if let Some(end) = trimmed[start..].find("]]") {
                    let target = &trimmed[start + 2..start + end];
                    let mut rel = target.trim().to_string();
                    if !rel.ends_with(".md") {
                        rel.push_str(".md");
                    }
                    return Some(rel);
                }
            }
        }
    }
    let relative = destination.strip_prefix(bob_dir).unwrap_or(destination);
    if relative.components().count() == 1 {
        if let Some(stem) = destination.file_stem().and_then(|s| s.to_str()) {
            return Some(format!("done/{stem}_done.md"));
        }
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
    // common `sync`/`scan` path (non-`wip` PDFs, or `wip` PDFs with no `#task`
    // bullets) free of the vault-wide scan.
    if plans
        .iter()
        .all(|plan| plan.annotation_task_candidates.is_empty())
    {
        return Ok(());
    }

    let disk_index = processed_task_index(config)?;
    let created_date = ctx.invocation_date.clone();

    for plan in plans.iter_mut() {
        let plan = &mut **plan;
        plan.annotation_tasks_created = 0;
        plan.annotation_tasks_skipped = 0;
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
                }
                AnnotationTaskTarget::RoutedNote(path) => {
                    let intent =
                        intents.entry(path.clone()).or_insert_with(|| {
                            RoutedInsertionIntent {
                                path: path.clone(),
                                lines: Vec::new(),
                                candidates: Vec::new(),
                                owner: plan.pdf.clone(),
                                is_v2_default,
                            }
                        });
                    // A destination shared by explicit and default routes
                    // keeps the legacy append behavior unless every line is
                    // a v2 default.
                    intent.is_v2_default &= is_v2_default;
                    intent.lines.push(task_line);
                    intent.candidates.push(candidate);
                }
            }
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
        &plan.stable_metadata,
        &plan.rendered_body,
    );
    plan.stable_note_action = change_action(
        plan.note.exists(),
        plan.note.contents().as_deref(),
        &plan.stable_rendered_note,
    );
}

pub(super) fn execute_pdf_sync(
    config: &Config,
    plan: &PdfSyncPlan,
    run_state: &mut RunWriteState,
) -> Result<SyncWriteReport> {
    let is_v2 = plan
        .reading_task_plan
        .as_ref()
        .is_some_and(|reading| reading.branch == NoteBranch::V2);
    if is_v2 {
        execute_pdf_sync_v2(config, plan, run_state)
    } else {
        execute_pdf_sync_v1(config, plan, run_state)
    }
}

fn execute_pdf_sync_v1(
    config: &Config,
    plan: &PdfSyncPlan,
    run_state: &mut RunWriteState,
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
        ensure_pdf_unchanged_for_write(plan)?;
        write_pdf_marker(
            &plan.pdf,
            plan.marker.annotation_id,
            &plan.rendered_marker,
        )?;
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
        plan.note.render_with_projection(
            &plan.synced_projection,
            &plan.synced_hash,
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
    let intent_outcome = execute_routed_intents(config, plan, run_state)?;
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
    })
}

/// Outcome of executing one plan's routed insertion intentions.
struct IntentOutcome {
    note_actions: usize,
    /// Planned lines skipped at execution (already present or just written).
    deduped: usize,
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
fn execute_routed_intents(
    config: &Config,
    plan: &PdfSyncPlan,
    run_state: &mut RunWriteState,
) -> Result<IntentOutcome> {
    let _ = config;
    let mut outcome = IntentOutcome {
        note_actions: 0,
        deduped: 0,
    };
    for intent in &plan.routed_intents {
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
            let mut missing_lines = Vec::new();
            let mut missing_keys = Vec::new();
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
                missing_lines.push(line.clone());
                missing_keys.push((
                    candidate.processed_id.clone(),
                    candidate.identity.clone(),
                    anchor,
                ));
            }
            if missing_lines.is_empty() {
                break;
            }
            let existed = current.is_some();
            let base = current.as_deref().unwrap_or("");
            let rendered = if intent.is_v2_default {
                match current.as_deref() {
                    Some(contents) => {
                        let mut updated = contents.to_string();
                        for line in &missing_lines {
                            let (next, _) =
                                crate::native::capture::insert_task_line(
                                    &updated, line,
                                );
                            updated = next;
                        }
                        updated
                    }
                    None => {
                        let mut rendered = missing_lines.join("\n");
                        rendered.push('\n');
                        rendered
                    }
                }
            } else {
                append_task_lines(base, &missing_lines)
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
    if plan.marker_write_needed {
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
    let intent_outcome = execute_routed_intents(config, plan, run_state)?;
    if plan.marker_write_needed {
        ensure_pdf_unchanged_for_write(plan)?;
        write_pdf_marker(
            &plan.pdf,
            plan.marker.annotation_id,
            &plan.rendered_marker,
        )?;
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
        let body = execution_body.as_deref().unwrap_or(&plan.rendered_body);
        let rendered = plan.note.render_with_projection(
            &render_projection,
            &plan.synced_hash,
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
    })
}

/// What the v2 reading-task action did for this PDF's note rendering.
struct V2ReadingOutcome {
    execution: Option<ReadingTaskExecution>,
    refreshed: Option<ref_tasks_mod::LocatedRefTask>,
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
        refreshed: None,
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
                refreshed: Some(refreshed),
                residence,
                embed: embed.or(blank.embed),
            })
        }
        ReadingTaskAction::Insert {
            destination_route,
            warning_child,
            mark,
            prefer_block_id,
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
                refreshed: None,
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
                refreshed: None,
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
