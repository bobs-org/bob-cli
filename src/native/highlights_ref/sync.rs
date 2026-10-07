//! PDF sync planning and execution.
use super::*;

pub(super) fn sync_pdf(
    config: &Config,
    pdf: &Path,
    options: SyncOptions,
) -> Result<()> {
    let mut plan = plan_pdf_sync(config, pdf, options)?;
    finalize_annotation_task_plans(config, &mut [&mut plan])?;
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
    let report = execute_pdf_sync(config, &plan)?;
    print_sync_write_report(report);
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
    let pre_scan_hook = configured_pre_scan_hook(no_hooks)?;
    run_pre_scan_hook(config, pre_scan_hook.as_ref(), options.dry_run)?;
    let intake = plan_xlib_intake(config)?;
    if !options.dry_run {
        execute_xlib_intake(&intake)?;
    }
    let pdfs = collect_pdf_paths(config, options.dry_run)?;
    validate_output_collisions(config, &pdfs)?;

    // `plan_pdf_sync` is a pure, read-only computation over an independent
    // `&Config` and one PDF path, so planning is embarrassingly parallel. We
    // collect into a position-keyed vector and reassemble in `pdfs` order so
    // reporting output stays deterministic regardless of completion order.
    let mut plan_outcomes = pdfs
        .iter()
        .zip(plan_pdfs(config, &pdfs, options, jobs))
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
    finalize_annotation_task_plans(config, &mut plans_to_finalize)?;
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
    let mut write_outcomes = Vec::new();
    for plan in &plans {
        match execute_pdf_sync(config, plan) {
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
) -> Vec<Result<PdfSyncPlan>> {
    if jobs <= 1 || pdfs.len() <= 1 {
        return pdfs
            .iter()
            .map(|pdf| plan_pdf_sync(config, pdf, options))
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
                                plan_pdf_sync(config, pdf, options),
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

pub(super) fn plan_pdf_sync(
    config: &Config,
    pdf: &Path,
    options: SyncOptions,
) -> Result<PdfSyncPlan> {
    let marker = read_pdf_marker(pdf)?;
    let marker_input = parse_marker_with_normalization(&marker.contents)?;
    let marker_projection = marker_input.projection;
    let note_path = ref_note_path(config, pdf)?;
    validate_note_target(&note_path)?;
    let note = read_note(&note_path)?;
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
        pdf_task_signal,
        status_normalization,
    })
}

pub(super) fn finalize_annotation_task_plans(
    config: &Config,
    plans: &mut [&mut PdfSyncPlan],
) -> Result<()> {
    // Building the processed-task index walks the entire vault. Skip it
    // entirely when no plan carries annotation-task candidates: with nothing to
    // accept/reject there are no routed groups to form and no reference-note
    // bodies to mutate, so the index would never be consulted. This keeps the
    // common `sync`/`scan` path (non-`wip` PDFs, or `wip` PDFs with no `#task`
    // bullets) free of the vault-wide scan.
    if plans
        .iter()
        .all(|plan| plan.annotation_task_candidates.is_empty())
    {
        return Ok(());
    }

    let mut processed = processed_task_index(config)?;
    let created_date = current_local_date();
    let mut routed_groups: BTreeMap<PathBuf, RoutedTaskGroup> = BTreeMap::new();

    for (plan_index, plan) in plans.iter_mut().enumerate() {
        let plan = &mut **plan;
        plan.annotation_tasks_created = 0;
        plan.annotation_tasks_skipped = 0;
        plan.routed_task_note_writes.clear();

        let mut reference_task_lines = Vec::new();
        for candidate in std::mem::take(&mut plan.annotation_task_candidates) {
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
                    let group = routed_groups
                        .entry(path.clone())
                        .or_insert_with(|| RoutedTaskGroup {
                            owner_plan_index: plan_index,
                            lines: Vec::new(),
                        });
                    group.lines.push(task_line);
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
    }

    for (path, group) in routed_groups {
        if group.lines.is_empty() {
            continue;
        }
        let original_contents = fs::read_to_string(&path).map_err(|error| {
            CommandError::new(format!(
                "read routed task note {}: {error}",
                path.display()
            ))
        })?;
        let rendered_contents =
            append_task_lines(&original_contents, &group.lines);
        let action =
            change_action(true, Some(&original_contents), &rendered_contents);
        if action == "none" {
            continue;
        }
        plans[group.owner_plan_index].routed_task_note_writes.push(
            RoutedTaskNoteWrite {
                path,
                original_contents,
                rendered_contents,
                action,
            },
        );
    }

    Ok(())
}

#[derive(Debug, Clone)]
pub(super) struct RoutedTaskGroup {
    pub(super) owner_plan_index: usize,
    pub(super) lines: Vec<String>,
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
        annotation_tasks_created: plan.annotation_tasks_created,
        annotation_tasks_skipped: plan.annotation_tasks_skipped,
    })
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
