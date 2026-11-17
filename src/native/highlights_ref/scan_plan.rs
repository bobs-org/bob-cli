//! Shared prepared full-scan planning and optimistic validation inputs.
//!
//! Human (`sync.rs`) and JSON (`scan_json.rs`) full scans share one owned
//! preparation result: ordered PDF discovery, one [`ScanContext`] (one
//! locator index plus one invocation date), parallel planning, annotation
//! finalization, preview-ID reservation, and collision checks. Planning
//! never prints and never executes; output and execution stay with the
//! callers. [`scan_apply`] runs this substrate outside the writer locks
//! and applies the result inside a bounded mutation window.
use super::*;

/// Content fingerprint for one evidence file: existence plus, when present,
/// the full content hash. Existence and path membership are checked, never
/// only mtime/size.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct EvidenceFileState {
    pub(super) exists: bool,
    pub(super) sha256: Option<String>,
}

fn fingerprint_bytes(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn fingerprint_file(path: &Path) -> Result<EvidenceFileState> {
    match fs::metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            Ok(EvidenceFileState {
                exists: false,
                sha256: None,
            })
        }
        Err(error) => Err(CommandError::new(format!(
            "stat evidence file {}: {error}",
            path.display()
        ))),
        Ok(meta) if meta.is_dir() => {
            // TextBundle sidecars and asset folders are membership, not
            // file bytes. Nested files the planner reads (text.md, notes)
            // are tracked through their own paths.
            Ok(EvidenceFileState {
                exists: true,
                sha256: None,
            })
        }
        Ok(_) => match fs::read(path) {
            Ok(bytes) => Ok(EvidenceFileState {
                exists: true,
                sha256: Some(fingerprint_bytes(&bytes)),
            }),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                Ok(EvidenceFileState {
                    exists: false,
                    sha256: None,
                })
            }
            Err(error) => Err(CommandError::new(format!(
                "read evidence file {}: {error}",
                path.display()
            ))),
        },
    }
}

/// Validation evidence captured together with the bytes/index used for
/// planning.
///
/// Inventory (explicit trees, see [`capture_scan_evidence`]):
/// - PDF contents (planner's `source_pdf_sha256`, reused without reparsing)
/// - present/absent sidecars and source image/audio companions
/// - ref-note preimages
/// - target-note preimages (routed intents, routed writes, reading
///   destinations)
/// - parent/routing/config inputs (highlights config file)
/// - reading-task locator and ID-occupancy inputs (locator fingerprint plus
///   destination/archive contents, which carry the occupied IDs)
/// - archive/capture metadata consulted by holds (`replaced_capture_hold`)
/// - library and xlib membership plus prospective destination absence
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ScanInputEvidence {
    pub(super) pdf_hashes: BTreeMap<PathBuf, String>,
    pub(super) file_states: BTreeMap<PathBuf, EvidenceFileState>,
    pub(super) lib_pdfs: Vec<PathBuf>,
    pub(super) xlib_members: Vec<PathBuf>,
    pub(super) intake_destinations: Vec<PathBuf>,
    pub(super) index_fingerprint: String,
}

/// Owned shared preparation result for one full scan.
pub(super) struct PreparedFullScan {
    pub(super) config: Config,
    pub(super) invocation_date: String,
    pub(super) intake: Vec<IntakeMove>,
    pub(super) pdfs: Vec<PathBuf>,
    pub(super) plan_outcomes: Vec<ScanPlanOutcome>,
    pub(super) evidence: ScanInputEvidence,
    /// Raw annotation candidates per PDF, retained before finalization so a
    /// later combined intake/library finalization can reset without
    /// duplicating intentions or retaining stale reservations.
    draft_annotation_candidates:
        BTreeMap<PathBuf, Vec<AnnotationTaskCandidate>>,
    /// Pre-finalization rendered bodies (body, stable note, action) per PDF,
    /// retained so [`reset_prepared_finalization`] can undo preview-ID
    /// patches and annotation insertions.
    draft_rendered: BTreeMap<PathBuf, (String, String, &'static str)>,
}

fn locator_fingerprint(ctx: &ScanContext) -> String {
    // `RefTaskIndex` has no stable digest API; its `Debug` covers the
    // candidate/follow-up/orphan/archive-pair state that planning reads. A
    // shared locator change alters this text, so every plan (even one
    // previously classified unchanged) revalidates to stale.
    fingerprint_bytes(format!("{:?}", ctx.index).as_bytes())
}

/// True when one PDF plan still needs a vault or PDF write.
///
/// Covers note/marker changes, missing or changed image assets
/// (`Copy` only; `None` means an already-present identical asset),
/// routed insertions whose identity is still absent
/// ([`intent_write_planned`]), reading-task `Insert`/`LineEdit`
/// (`NoWrite` alone is not work), annotation-task creations, and legacy
/// routed note writes. Already-present identical assets and `NoWrite`
/// reading actions never count.
pub(super) fn plan_has_actual_work(plan: &PdfSyncPlan) -> bool {
    if plan.stable_note_action != "none" || plan.marker_write_needed {
        return true;
    }
    if planned_image_asset_write_count(plan) > 0 {
        return true;
    }
    if plan.annotation_tasks_created > 0 {
        return true;
    }
    if planned_routed_note_write_count(plan) > 0 {
        return true;
    }
    for intent in &plan.routed_intents {
        if intent_write_planned(&intent.path, intent) {
            return true;
        }
    }
    if let Some(reading) = plan.reading_task_plan.as_ref() {
        match reading.action {
            ReadingTaskAction::Insert { .. }
            | ReadingTaskAction::LineEdit { .. } => return true,
            ReadingTaskAction::NoWrite => {}
        }
    }
    false
}

/// True when the prepared scan still needs any vault or PDF write.
/// Pending xlib intake is work even when the existing library is unchanged.
#[cfg(test)]
pub(super) fn prepared_scan_has_actual_work(
    prepared: &PreparedFullScan,
) -> bool {
    if !prepared.intake.is_empty() {
        return true;
    }
    prepared.plan_outcomes.iter().any(|outcome| match outcome {
        ScanPlanOutcome::Planned(plan) => plan_has_actual_work(plan),
        ScanPlanOutcome::Held(_) | ScanPlanOutcome::Failed(_) => true,
    })
}

/// True when the prepared scan must acquire writer locks. Pending intake
/// or a planned vault/PDF write needs the mutation window; per-PDF holds
/// and plan failures are reported on the lock-free path.
pub(super) fn prepared_scan_needs_mutation(
    prepared: &PreparedFullScan,
) -> bool {
    if !prepared.intake.is_empty() {
        return true;
    }
    prepared.plan_outcomes.iter().any(|outcome| match outcome {
        ScanPlanOutcome::Planned(plan) => plan_has_actual_work(plan),
        ScanPlanOutcome::Held(_) | ScanPlanOutcome::Failed(_) => false,
    })
}

pub(super) fn scan_inputs_changed_error(reason: &str) -> CommandError {
    CommandError::new(format!(
        "scan inputs changed during planning ({reason}); rerun bob ref scan"
    ))
    .with_code("scan_failed")
}

/// Read-only assembly of equivalent unchanged write outcomes without calling
/// the mutation executor. Only `Planned` PDFs produce a write outcome
/// (`Written` with `none` actions and the plan's diagnostic counts), the
/// same shape the mutation executor emits. Plan-time holds and failures
/// stay on the plan outcomes so presenters and JSON assembly do not
/// double-count them as write holds or write failures.
pub(super) fn unchanged_write_outcomes_for(
    plan_outcomes: &[ScanPlanOutcome],
) -> Vec<ScanWriteOutcome> {
    plan_outcomes
        .iter()
        .filter_map(|outcome| match outcome {
            ScanPlanOutcome::Planned(plan) => {
                Some(ScanWriteOutcome::Written(unchanged_report_for_plan(plan)))
            }
            ScanPlanOutcome::Held(_) | ScanPlanOutcome::Failed(_) => None,
        })
        .collect()
}

fn unchanged_report_for_plan(plan: &PdfSyncPlan) -> SyncWriteReport {
    let rendered = plan.rendered_highlights.as_ref();
    SyncWriteReport {
        note_action: "none",
        marker_action: "none",
        image_count: rendered.map(|value| value.image_count).unwrap_or(0),
        image_note_count: rendered
            .map(|value| value.image_note_count)
            .unwrap_or(0),
        image_notes_unresolved: rendered
            .map(|value| value.image_notes_unresolved.clone())
            .unwrap_or_default(),
        continuation_count: rendered
            .map(|value| value.continuation_count)
            .unwrap_or(0),
        dangling_continuations: rendered
            .map(|value| value.dangling_continuations.clone())
            .unwrap_or_default(),
        underlines_merged: rendered
            .map(|value| value.underlines_merged)
            .unwrap_or(0),
        standalone_underlines: rendered
            .map(|value| value.standalone_underlines.clone())
            .unwrap_or_default(),
        section_diagnostics: rendered
            .map(|value| value.section_diagnostics.clone())
            .unwrap_or_default(),
        section_count: rendered.map(|value| value.sections.len()).unwrap_or(0),
        image_assets_written: 0,
        image_assets_skipped: plan.image_assets.len(),
        routed_note_actions: 0,
        annotation_tasks_created: 0,
        annotation_tasks_skipped: plan.annotation_tasks_skipped,
        reading_created: None,
        reading_updated: None,
        marker_write: None,
    }
}

/// Reset finalized reservations so a later combined intake/library
/// finalization pass can rerun without duplicating intentions or retaining
/// stale preview IDs. Restores raw annotation candidates and
/// pre-finalization rendered bodies captured in [`prepare_from_inputs`].
pub(super) fn reset_prepared_finalization(prepared: &mut PreparedFullScan) {
    for outcome in &mut prepared.plan_outcomes {
        let ScanPlanOutcome::Planned(plan) = outcome else {
            continue;
        };
        if let Some(candidates) =
            prepared.draft_annotation_candidates.get(&plan.pdf).cloned()
        {
            plan.annotation_task_candidates = candidates;
        }
        plan.annotation_tasks_created = 0;
        plan.annotation_tasks_skipped = 0;
        plan.annotation_task_relation = None;
        plan.routed_task_note_writes.clear();
        plan.routed_intents.clear();
        plan.preview_block_id = None;
        if let Some((body, stable, action)) =
            prepared.draft_rendered.get(&plan.pdf).cloned()
        {
            plan.rendered_body = body;
            plan.stable_rendered_note = stable;
            plan.stable_note_action = action;
        }
    }
}

/// Validation of a prepared result against current disk: `Unchanged` means
/// the evidence still matches (the prepared plans remain usable);
/// `Stale` names the first mismatch (created/deleted/renamed or edited
/// inputs, including shared locator changes); `Failed` carries a hard
/// revalidation error.
#[derive(Debug, Clone)]
pub(super) enum PreparedValidation {
    Unchanged,
    Stale { reason: String },
    Failed(CommandError),
}

/// Recompute evidence and compare with the prepared snapshot.
pub(super) fn validate_prepared_scan(
    prepared: &PreparedFullScan,
) -> PreparedValidation {
    // Shared locator changes affect every plan, even one previously
    // classified unchanged: rebuild the locator index fresh and compare
    // before comparing file evidence.
    let fresh_index = crate::native::ref_tasks::RefTaskIndex::build(
        &prepared.config.bob_dir,
        &prepared.config.ref_dir,
    );
    let fresh_ctx = ScanContext {
        index: fresh_index,
        invocation_date: prepared.invocation_date.clone(),
    };
    let fresh_fingerprint = locator_fingerprint(&fresh_ctx);
    if fresh_fingerprint != prepared.evidence.index_fingerprint {
        return PreparedValidation::Stale {
            reason: "reading-task locator changed".to_string(),
        };
    }
    let current = match capture_scan_evidence(
        &prepared.config,
        &prepared.pdfs,
        &prepared.intake,
        &prepared.plan_outcomes,
        &fresh_fingerprint,
    ) {
        Ok(current) => current,
        Err(error) => return PreparedValidation::Failed(error),
    };
    if current.pdf_hashes != prepared.evidence.pdf_hashes {
        return PreparedValidation::Stale {
            reason: "PDF contents changed".to_string(),
        };
    }
    if current.lib_pdfs != prepared.evidence.lib_pdfs {
        return PreparedValidation::Stale {
            reason: "library membership changed".to_string(),
        };
    }
    if current.xlib_members != prepared.evidence.xlib_members {
        return PreparedValidation::Stale {
            reason: "xlib membership changed".to_string(),
        };
    }
    if current.intake_destinations != prepared.evidence.intake_destinations {
        return PreparedValidation::Stale {
            reason: "intake destinations changed".to_string(),
        };
    }
    if current.file_states != prepared.evidence.file_states {
        let reason = first_state_mismatch(
            &prepared.evidence.file_states,
            &current.file_states,
        );
        return PreparedValidation::Stale { reason };
    }
    PreparedValidation::Unchanged
}

fn first_state_mismatch(
    prepared: &BTreeMap<PathBuf, EvidenceFileState>,
    current: &BTreeMap<PathBuf, EvidenceFileState>,
) -> String {
    for (path, old) in prepared {
        match current.get(path) {
            Some(new) if new == old => {}
            Some(_) => {
                return format!("input changed: {}", path.display());
            }
            None => {
                return format!("input removed: {}", path.display());
            }
        }
    }
    for path in current.keys() {
        if !prepared.contains_key(path) {
            return format!("input added: {}", path.display());
        }
    }
    "inputs changed".to_string()
}

/// Ordered shared preparation from explicit intake and PDF lists.
///
/// `invocation_date` pins one date across retries; pass `None` for a fresh
/// scan and `Some(date)` to rebuild under the same date. Never prints and
/// never executes vault or PDF writes. Finalization order is canonical
/// (annotation tasks, then preview IDs) for both human and JSON paths.
pub(super) fn prepare_from_inputs(
    config: &Config,
    options: SyncOptions,
    jobs: usize,
    intake: Vec<IntakeMove>,
    pdfs: Vec<PathBuf>,
    invocation_date: Option<String>,
) -> Result<PreparedFullScan> {
    validate_output_collisions(config, &pdfs)?;
    let ctx = match invocation_date {
        Some(invocation_date) => ScanContext {
            index: crate::native::ref_tasks::RefTaskIndex::build(
                &config.bob_dir,
                &config.ref_dir,
            ),
            invocation_date,
        },
        None => ScanContext::build(config),
    };
    let invocation_date = ctx.invocation_date.clone();
    let mut plan_outcomes = plan_pdfs(config, &pdfs, options, jobs, &ctx);
    let mut drafts_candidates: BTreeMap<PathBuf, Vec<AnnotationTaskCandidate>> =
        BTreeMap::new();
    let mut drafts_rendered: BTreeMap<PathBuf, (String, String, &'static str)> =
        BTreeMap::new();
    for outcome in &plan_outcomes {
        if let ScanPlanOutcome::Planned(plan) = outcome {
            drafts_candidates.insert(
                plan.pdf.clone(),
                plan.annotation_task_candidates.clone(),
            );
            drafts_rendered.insert(
                plan.pdf.clone(),
                (
                    plan.rendered_body.clone(),
                    plan.stable_rendered_note.clone(),
                    plan.stable_note_action,
                ),
            );
        }
    }
    {
        let mut plans_to_finalize = plan_outcomes
            .iter_mut()
            .filter_map(|outcome| match outcome {
                ScanPlanOutcome::Planned(plan) => Some(plan.as_mut()),
                ScanPlanOutcome::Held(_) | ScanPlanOutcome::Failed(_) => None,
            })
            .collect::<Vec<_>>();
        finalize_annotation_task_plans(config, &mut plans_to_finalize, &ctx)?;
        reserve_preview_ids(config, &mut plans_to_finalize, &ctx)?;
    }
    let plans = plan_outcomes
        .iter()
        .filter_map(|outcome| match outcome {
            ScanPlanOutcome::Planned(plan) => Some(plan.as_ref()),
            ScanPlanOutcome::Held(_) | ScanPlanOutcome::Failed(_) => None,
        })
        .collect::<Vec<_>>();
    validate_planned_asset_collisions(&plans)?;
    let index_fingerprint = locator_fingerprint(&ctx);
    let evidence = capture_scan_evidence(
        config,
        &pdfs,
        &intake,
        &plan_outcomes,
        &index_fingerprint,
    )?;
    Ok(PreparedFullScan {
        config: config.clone(),
        invocation_date,
        intake,
        pdfs,
        plan_outcomes,
        evidence,
        draft_annotation_candidates: drafts_candidates,
        draft_rendered: drafts_rendered,
    })
}

/// Convenience read-only preparation: discover intake plus ordered PDFs,
/// then plan. Dry runs include xlib in the PDF list. Write mode lists the
/// existing library only (empty when `lib/` is missing and intake is
/// pending, so an xlib-only vault can still prepare outside the locks).
pub(super) fn prepare_full_scan(
    config: &Config,
    options: SyncOptions,
    jobs: usize,
    invocation_date: Option<String>,
) -> Result<PreparedFullScan> {
    let intake = plan_xlib_intake(config)?;
    let pdfs = if options.dry_run {
        collect_pdf_paths(config, true)?
    } else if config.lib_dir.is_dir() {
        collect_pdf_paths(config, false)?
    } else if intake.is_empty() {
        collect_pdf_paths(config, false)?
    } else {
        Vec::new()
    };
    prepare_from_inputs(config, options, jobs, intake, pdfs, invocation_date)
}

/// Bounded preparation retry: on an immediately stale validation (a source
/// changing during the pure planning pass), rebuild once outside any new
/// locks under the same invocation date. Persistent churn returns the
/// existing generic `scan_failed` envelope with a rerun hint; it never
/// spins and never parses the whole library while locked.
pub(super) fn prepare_full_scan_with_retry(
    config: &Config,
    options: SyncOptions,
    jobs: usize,
) -> Result<PreparedFullScan> {
    prepare_full_scan_with_retry_date(config, options, jobs, None)
}

pub(super) fn prepare_full_scan_with_retry_date(
    config: &Config,
    options: SyncOptions,
    jobs: usize,
    invocation_date: Option<String>,
) -> Result<PreparedFullScan> {
    let mut prepared =
        prepare_full_scan(config, options, jobs, invocation_date)?;
    match validate_prepared_scan(&prepared) {
        PreparedValidation::Unchanged => Ok(prepared),
        PreparedValidation::Failed(error) => Err(error),
        PreparedValidation::Stale { .. } => {
            // Clear stale reservations before the single rebuild so a
            // combined intake/library finalization later cannot duplicate
            // intentions or retain stale preview IDs.
            reset_prepared_finalization(&mut prepared);
            let date = prepared.invocation_date.clone();
            let rebuilt = prepare_full_scan(config, options, jobs, Some(date))?;
            match validate_prepared_scan(&rebuilt) {
                PreparedValidation::Unchanged => Ok(rebuilt),
                PreparedValidation::Stale { reason } => {
                    Err(scan_inputs_changed_error(&reason))
                }
                PreparedValidation::Failed(error) => Err(error),
            }
        }
    }
}

fn plan_outcome_pdf(outcome: &ScanPlanOutcome) -> &Path {
    match outcome {
        ScanPlanOutcome::Planned(plan) => plan.pdf.as_path(),
        ScanPlanOutcome::Held(hold) => hold.pdf.as_path(),
        ScanPlanOutcome::Failed(failure) => failure.pdf.as_path(),
    }
}

/// PDFs that intake moved into the library, plus existing library PDFs
/// that received late audio and must be replanned.
pub(super) fn intake_affected_pdfs(intake: &[IntakeMove]) -> Vec<PathBuf> {
    let mut pdfs = Vec::new();
    for item in intake {
        if is_pdf_path(&item.destination) {
            pdfs.push(item.destination.clone());
        } else {
            let pdf = item.destination.with_extension("pdf");
            if pdf.is_file() {
                pdfs.push(pdf);
            }
        }
    }
    pdfs.sort();
    pdfs.dedup();
    pdfs
}

/// Move and plan only newly intaken / late-audio-affected PDFs, then
/// rebuild shared ID reservations over the combined ordered set. Existing
/// library plans are reused. Must run after intake moves, while the
/// mutation locks are still held; never retries.
pub(super) fn integrate_intake_plans(
    prepared: &mut PreparedFullScan,
    intake: &[IntakeMove],
    options: SyncOptions,
    jobs: usize,
) -> Result<()> {
    let new_pdfs = intake_affected_pdfs(intake);
    if new_pdfs.is_empty() {
        return Ok(());
    }
    reset_prepared_finalization(prepared);
    let ctx = ScanContext {
        index: crate::native::ref_tasks::RefTaskIndex::build(
            &prepared.config.bob_dir,
            &prepared.config.ref_dir,
        ),
        invocation_date: prepared.invocation_date.clone(),
    };
    let new_outcomes =
        plan_pdfs(&prepared.config, &new_pdfs, options, jobs, &ctx);
    for outcome in &new_outcomes {
        if let ScanPlanOutcome::Planned(plan) = outcome {
            prepared.draft_annotation_candidates.insert(
                plan.pdf.clone(),
                plan.annotation_task_candidates.clone(),
            );
            prepared.draft_rendered.insert(
                plan.pdf.clone(),
                (
                    plan.rendered_body.clone(),
                    plan.stable_rendered_note.clone(),
                    plan.stable_note_action,
                ),
            );
        }
    }
    let replan = new_pdfs.iter().cloned().collect::<BTreeSet<_>>();
    prepared
        .plan_outcomes
        .retain(|outcome| !replan.contains(plan_outcome_pdf(outcome)));
    prepared.plan_outcomes.extend(new_outcomes);
    let pdfs = if prepared.config.lib_dir.is_dir() {
        collect_pdf_paths(&prepared.config, false)?
    } else {
        Vec::new()
    };
    prepared.plan_outcomes.sort_by_key(|outcome| {
        pdfs.iter()
            .position(|pdf| pdf == plan_outcome_pdf(outcome))
            .unwrap_or(usize::MAX)
    });
    prepared.pdfs = pdfs;
    {
        let mut plans_to_finalize = prepared
            .plan_outcomes
            .iter_mut()
            .filter_map(|outcome| match outcome {
                ScanPlanOutcome::Planned(plan) => Some(plan.as_mut()),
                ScanPlanOutcome::Held(_) | ScanPlanOutcome::Failed(_) => None,
            })
            .collect::<Vec<_>>();
        finalize_annotation_task_plans(
            &prepared.config,
            &mut plans_to_finalize,
            &ctx,
        )?;
        reserve_preview_ids(&prepared.config, &mut plans_to_finalize, &ctx)?;
    }
    Ok(())
}

fn audio_companion_candidates(pdf: &Path) -> Vec<PathBuf> {
    AUDIO_COMPANION_EXTENSIONS
        .iter()
        .map(|extension| {
            let mut path = pdf.to_path_buf();
            path.set_extension(extension.trim_start_matches('.'));
            path
        })
        .collect()
}

/// Capture validation evidence together with the bytes/index used for
/// planning. Explicit trees (all vault-absolute):
/// `lib_dir` PDFs, `xlib_dir` members plus companions, each plan's
/// sidecar/companions, ref-note preimages, routed/reading target-note
/// preimages, the highlights config file, archive records consulted by
/// `replaced_capture_hold`, and prospective intake-destination absence.
/// PDF hashes reuse the planner's `source_pdf_sha256`; existing library
/// PDFs are fingerprinted without repeating PDF parsing/rendering, and no
/// unrelated vault media is hashed.
fn capture_scan_evidence(
    config: &Config,
    pdfs: &[PathBuf],
    intake: &[IntakeMove],
    plan_outcomes: &[ScanPlanOutcome],
    index_fingerprint: &str,
) -> Result<ScanInputEvidence> {
    let mut pdf_hashes: BTreeMap<PathBuf, String> = BTreeMap::new();
    let mut file_states: BTreeMap<PathBuf, EvidenceFileState> = BTreeMap::new();
    fn track(
        file_states: &mut BTreeMap<PathBuf, EvidenceFileState>,
        path: PathBuf,
    ) -> Result<()> {
        if !file_states.contains_key(&path) {
            let state = fingerprint_file(&path)?;
            file_states.insert(path, state);
        }
        Ok(())
    }

    for outcome in plan_outcomes {
        if let ScanPlanOutcome::Planned(plan) = outcome {
            pdf_hashes.insert(
                plan.pdf.clone(),
                plan.marker.source_pdf_sha256.clone(),
            );
        }
    }
    for pdf in pdfs {
        if !pdf_hashes.contains_key(pdf) {
            // Held/failed PDFs have no planner hash; fingerprint the bytes
            // directly so membership/content changes still invalidate.
            if let Ok(hash) = super::io::sha256_file(pdf) {
                pdf_hashes.insert(pdf.clone(), hash);
            }
        }
        track(&mut file_states, pdf.clone())?;
        // Present/absent sidecar and source companions for this PDF.
        let mut sidecar = pdf.clone();
        sidecar.set_extension("md");
        track(&mut file_states, sidecar)?;
        let mut textbundle = pdf.clone();
        textbundle.set_extension("textbundle");
        file_states.entry(textbundle.clone()).or_insert_with(|| {
            EvidenceFileState {
                exists: textbundle.exists(),
                sha256: None,
            }
        });
        for companion in audio_companion_candidates(pdf) {
            track(&mut file_states, companion)?;
        }
    }
    for outcome in plan_outcomes {
        let (plan, hold_note) = match outcome {
            ScanPlanOutcome::Planned(plan) => (Some(plan.as_ref()), None),
            ScanPlanOutcome::Held(hold) => (None, Some(hold)),
            ScanPlanOutcome::Failed(_) => (None, None),
        };
        if let Some(plan) = plan {
            track(&mut file_states, plan.note_path.clone())?;
            if let Some(sidecar) = plan.sidecar_path.as_ref() {
                track(&mut file_states, sidecar.clone())?;
            }
            for asset in &plan.image_assets {
                track(&mut file_states, asset.dest_path.clone())?;
                track(&mut file_states, asset.source_path.clone())?;
            }
            for write in &plan.routed_task_note_writes {
                track(&mut file_states, write.path.clone())?;
            }
            for intent in &plan.routed_intents {
                track(&mut file_states, intent.path.clone())?;
            }
            if let Some(reading) = plan.reading_task_plan.as_ref() {
                match &reading.action {
                    ReadingTaskAction::Insert {
                        destination_route, ..
                    } => {
                        track(
                            &mut file_states,
                            config
                                .bob_dir
                                .join(format!("{destination_route}.md")),
                        )?;
                    }
                    ReadingTaskAction::LineEdit { .. } => {
                        if let Some(located) = plan.reading_located.as_ref() {
                            track(
                                &mut file_states,
                                config.bob_dir.join(&located.path),
                            )?;
                        }
                    }
                    ReadingTaskAction::NoWrite => {}
                }
            }
            // Archive/capture metadata consulted by `replaced_capture_hold`.
            let note_rel = plan
                .note_path
                .strip_prefix(&config.bob_dir)
                .map(|rel| rel.to_string_lossy().replace('\\', "/"))
                .unwrap_or_default();
            if !note_rel.is_empty()
                && let Ok(records) =
                    super::overwrite::archive::records_for_live_note(
                        &config.bob_dir,
                        &note_rel,
                    )
            {
                for record in records {
                    track(
                        &mut file_states,
                        config.bob_dir.join(&record.note_path),
                    )?;
                    if let Some(pdf) = record.annotated_pdf.as_ref() {
                        track(
                            &mut file_states,
                            config.bob_dir.join(&pdf.path),
                        )?;
                    }
                    if let Some(assets) = record.assets_path.as_ref() {
                        let assets_abs = config.bob_dir.join(assets);
                        file_states.entry(assets_abs.clone()).or_insert_with(
                            || EvidenceFileState {
                                exists: assets_abs.exists(),
                                sha256: None,
                            },
                        );
                    }
                }
            }
        }
        if let Some(hold) = hold_note {
            track(&mut file_states, hold.note_path.clone())?;
        }
    }
    // Parent/routing/config inputs: the highlights config file.
    track(&mut file_states, super::bob_config::config_path())?;
    // Library and xlib membership plus prospective destination absence.
    let mut lib_pdfs = Vec::new();
    if config.lib_dir.is_dir()
        && let Ok(mut paths) = collect_lib_membership(config)
    {
        paths.sort();
        lib_pdfs = paths;
    }
    let mut xlib_members = Vec::new();
    if config.xlib_dir.is_dir() {
        let mut members = Vec::new();
        collect_xlib_membership(config, &mut members);
        members.sort();
        xlib_members = members;
    }
    let mut intake_destinations = intake
        .iter()
        .flat_map(|item| {
            let mut paths = vec![item.destination.clone()];
            paths.extend(item.companions.iter().map(|(_, dest)| dest.clone()));
            paths
        })
        .collect::<Vec<_>>();
    intake_destinations.sort();
    for dest in &intake_destinations {
        track(&mut file_states, dest.clone())?;
    }
    for item in intake {
        track(&mut file_states, item.source.clone())?;
        for (source, dest) in &item.companions {
            track(&mut file_states, source.clone())?;
            track(&mut file_states, dest.clone())?;
        }
    }
    Ok(ScanInputEvidence {
        pdf_hashes,
        file_states,
        lib_pdfs,
        xlib_members,
        intake_destinations,
        index_fingerprint: index_fingerprint.to_string(),
    })
}

fn collect_lib_membership(config: &Config) -> Result<Vec<PathBuf>> {
    let mut paths = Vec::new();
    super::doctor::collect_pdf_paths_from_dir(&config.lib_dir, &mut paths)?;
    // Late-audio membership also invalidates: an `.mp3` arriving beside a
    // planned PDF changes the plan.
    collect_audio_under(&config.lib_dir, &mut paths);
    Ok(paths)
}

fn collect_xlib_membership(config: &Config, out: &mut Vec<PathBuf>) {
    let mut pdfs = Vec::new();
    if super::doctor::collect_pdf_paths_from_dir(&config.xlib_dir, &mut pdfs)
        .is_ok()
    {
        out.extend(pdfs);
    }
    collect_audio_under(&config.xlib_dir, out);
}

fn collect_audio_under(directory: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    let mut entries =
        entries.filter_map(|entry| entry.ok()).collect::<Vec<_>>();
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let path = entry.path();
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_dir() {
            if path
                .file_name()
                .is_some_and(|name| name == ".git" || name == ".bob")
            {
                continue;
            }
            collect_audio_under(&path, out);
        } else if kind.is_file()
            && path.extension().and_then(|ext| ext.to_str()).is_some_and(
                |ext| {
                    AUDIO_COMPANION_EXTENSIONS.iter().any(|candidate| {
                        candidate
                            .trim_start_matches('.')
                            .eq_ignore_ascii_case(ext)
                    })
                },
            )
        {
            out.push(path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn test_config(bob_dir: &Path) -> Config {
        Config::for_vault(bob_dir)
    }

    fn minimal_plan(pdf: &Path, note_path: &Path) -> PdfSyncPlan {
        PdfSyncPlan {
            pdf: pdf.to_path_buf(),
            note_path: note_path.to_path_buf(),
            sidecar_path: None,
            marker: PdfMarker {
                annotation_id: (0, 0),
                contents: String::new(),
                page_number: 1,
                note_number: 0,
                source_pdf_sha256: "abc".to_string(),
            },
            decision: SyncDecision {
                source: SyncSource::Marker,
                reason: String::new(),
                marker_contributed: false,
                frontmatter_contributed: false,
            },
            rendered_highlights_count: None,
            synced_projection: Projection::new(),
            synced_hash: String::new(),
            rendered_marker: String::new(),
            marker_write_needed: false,
            stale_restore: false,
            replaced_hash: None,
            note: ParsedNote::empty(),
            sidecar: None,
            rendered_highlights: None,
            stable_metadata: PipelineMetadata {
                source_pdf: String::new(),
                source_pdf_sha256: "abc".to_string(),
                ref_type: None,
                highlights_sidecar: None,
                highlights_count: None,
                highlights_synced_at: None,
                created: None,
                audio: None,
                birth_lineage: None,
                source_markdown: None,
            },
            rendered_body: String::new(),
            stable_rendered_note: String::new(),
            stable_note_action: "none",
            image_assets: Vec::new(),
            annotation_task_candidates: Vec::new(),
            annotation_tasks_created: 0,
            annotation_tasks_skipped: 0,
            annotation_task_relation: None,
            routed_task_note_writes: Vec::new(),
            routed_intents: Vec::new(),
            reading_task_plan: Some(ReadingTaskPlan {
                branch: NoteBranch::V1,
                kind: ReadingTaskKind::V1,
                action: ReadingTaskAction::NoWrite,
                residence: None,
                embed: None,
                status_target: None,
                task_changed: false,
                refuse_status_parent_writes: false,
                diagnostics: Vec::new(),
            }),
            reading_located: None,
            reading_diagnostics: Vec::new(),
            preview_block_id: None,
            v2_residence: None,
            pdf_task_signal: PdfTaskStatusSignal {
                status: PdfTaskStatus::Missing,
                status_contributed: None,
            },
            status_normalization: StatusNormalization::default(),
        }
    }

    fn routed_intent(dest: PathBuf, owner: PathBuf) -> RoutedInsertionIntent {
        RoutedInsertionIntent {
            path: dest,
            lines: vec!["- [ ] follow up ^ann-1".to_string()],
            candidates: vec![AnnotationTaskCandidate {
                identity: "^ann-1".to_string(),
                task_text: "follow up".to_string(),
                source_block_id: "src".to_string(),
                target: AnnotationTaskTarget::RoutedNote(PathBuf::from(
                    "sase.md",
                )),
                source_ref_note_path: PathBuf::from("ref/papers/src.md"),
                processed_id: "ann-1".to_string(),
            }],
            owner,
            is_v2_default: false,
            annotation_task_relation: None,
        }
    }

    #[test]
    fn actual_work_covers_note_and_marker_changes() {
        let temp = tempfile::tempdir().expect("tempdir");
        let pdf = temp.path().join("a.pdf");
        let note = temp.path().join("a.md");
        let mut plan = minimal_plan(&pdf, &note);
        assert!(!plan_has_actual_work(&plan));
        plan.stable_note_action = "update";
        assert!(plan_has_actual_work(&plan));
        plan.stable_note_action = "none";
        plan.marker_write_needed = true;
        assert!(plan_has_actual_work(&plan));
    }

    #[test]
    fn actual_work_ignores_identical_assets_and_nowrite_reading() {
        let temp = tempfile::tempdir().expect("tempdir");
        let pdf = temp.path().join("a.pdf");
        let note = temp.path().join("a.md");
        let mut plan = minimal_plan(&pdf, &note);
        plan.image_assets.push(ImageAssetWrite {
            annotation_order: 0,
            source_path: temp.path().join("src.png"),
            dest_path: temp.path().join("dest.png"),
            vault_relative_dest_path: PathBuf::from("dest.png"),
            source_sha256: "x".to_string(),
            block_id: "b".to_string(),
            action: ImageAssetAction::None,
        });
        assert!(
            !plan_has_actual_work(&plan),
            "an already-present identical asset is not work"
        );
        plan.image_assets[0].action = ImageAssetAction::Copy;
        assert!(plan_has_actual_work(&plan));
    }

    #[test]
    fn actual_work_covers_routed_reading_and_annotation_tasks() {
        let temp = tempfile::tempdir().expect("tempdir");
        let pdf = temp.path().join("a.pdf");
        let note = temp.path().join("a.md");
        let dest = temp.path().join("dest.md");
        fs::write(&dest, "# Dest\n").expect("write dest");
        let mut plan = minimal_plan(&pdf, &note);
        plan.routed_intents
            .push(routed_intent(dest.clone(), pdf.clone()));
        assert!(
            plan_has_actual_work(&plan),
            "a routed insertion absent from the destination is work"
        );
        fs::write(&dest, "# Dest\n\n- [ ] follow up ^ann-1\n")
            .expect("write dest with identity");
        assert!(
            !plan_has_actual_work(&plan),
            "a routed insertion already present is not work"
        );
        plan.routed_intents.clear();
        plan.reading_task_plan = Some(ReadingTaskPlan {
            branch: NoteBranch::V2,
            kind: ReadingTaskKind::Birth,
            action: ReadingTaskAction::Insert {
                destination_route: "sase".to_string(),
                destination_label: "SASE".to_string(),
                warning_child: None,
                mark: ' ',
                prefer_block_id: None,
                reading: None,
            },
            residence: None,
            embed: None,
            status_target: None,
            task_changed: false,
            refuse_status_parent_writes: false,
            diagnostics: Vec::new(),
        });
        assert!(plan_has_actual_work(&plan));
        plan.reading_task_plan = Some(ReadingTaskPlan {
            branch: NoteBranch::V1,
            kind: ReadingTaskKind::V1,
            action: ReadingTaskAction::NoWrite,
            residence: None,
            embed: None,
            status_target: None,
            task_changed: false,
            refuse_status_parent_writes: false,
            diagnostics: Vec::new(),
        });
        assert!(!plan_has_actual_work(&plan));
        plan.annotation_tasks_created = 1;
        assert!(plan_has_actual_work(&plan));
    }

    #[test]
    fn stable_repeat_scan_stays_unchanged_without_executing() {
        let temp = tempfile::tempdir().expect("tempdir");
        let pdf = temp.path().join("a.pdf");
        let note = temp.path().join("a.md");
        let plan = minimal_plan(&pdf, &note);
        assert!(!plan_has_actual_work(&plan));
        let outcomes = vec![ScanPlanOutcome::Planned(Box::new(plan))];
        let writes = unchanged_write_outcomes_for(&outcomes);
        assert_eq!(writes.len(), 1);
        match &writes[0] {
            ScanWriteOutcome::Written(report) => {
                assert_eq!(report.note_action, "none");
                assert_eq!(report.marker_action, "none");
                assert_eq!(report.image_assets_written, 0);
                assert_eq!(report.routed_note_actions, 0);
            }
            _ => panic!("expected written outcome"),
        }
    }

    #[test]
    fn validation_detects_edits_missing_and_new_inputs() {
        let temp = tempfile::tempdir().expect("tempdir");
        let bob_dir = temp.path();
        let config = test_config(bob_dir);
        let pdf = bob_dir.join("lib/papers/a.pdf");
        fs::create_dir_all(pdf.parent().expect("parent")).expect("mkdir");
        fs::write(&pdf, "%PDF-1.4 fake").expect("write pdf");
        let note = bob_dir.join("ref/papers/a.md");
        fs::create_dir_all(note.parent().expect("parent")).expect("mkdir");
        fs::write(&note, "# A\n").expect("write note");
        let plan = minimal_plan(&pdf, &note);
        let outcomes = vec![ScanPlanOutcome::Planned(Box::new(plan))];
        let ctx = ScanContext {
            index: crate::native::ref_tasks::RefTaskIndex::build(
                &config.bob_dir,
                &config.ref_dir,
            ),
            invocation_date: "2026-11-17".to_string(),
        };
        let fingerprint = locator_fingerprint(&ctx);
        let evidence = capture_scan_evidence(
            &config,
            std::slice::from_ref(&pdf),
            &[],
            &outcomes,
            &fingerprint,
        )
        .expect("capture evidence");
        let prepared = PreparedFullScan {
            config: config.clone(),
            invocation_date: "2026-11-17".to_string(),
            intake: Vec::new(),
            pdfs: vec![pdf.clone()],
            plan_outcomes: outcomes,
            evidence,
            draft_annotation_candidates: BTreeMap::new(),
            draft_rendered: BTreeMap::new(),
        };
        assert!(matches!(
            validate_prepared_scan(&prepared),
            PreparedValidation::Unchanged
        ));
        // Same length, different bytes must still invalidate.
        fs::write(&note, "# B\n").expect("edit note same length");
        assert!(matches!(
            validate_prepared_scan(&prepared),
            PreparedValidation::Stale { .. }
        ));
        fs::write(&note, "# A\n").expect("restore note");
        assert!(matches!(
            validate_prepared_scan(&prepared),
            PreparedValidation::Unchanged
        ));
        fs::remove_file(&note).expect("remove note");
        assert!(matches!(
            validate_prepared_scan(&prepared),
            PreparedValidation::Stale { .. }
        ));
    }

    #[test]
    fn validation_detects_shared_locator_changes() {
        let temp = tempfile::tempdir().expect("tempdir");
        let bob_dir = temp.path();
        let config = test_config(bob_dir);
        let pdf = bob_dir.join("lib/papers/a.pdf");
        fs::create_dir_all(pdf.parent().expect("parent")).expect("mkdir");
        fs::write(&pdf, "%PDF-1.4 fake").expect("write pdf");
        let note = bob_dir.join("ref/papers/a.md");
        fs::create_dir_all(note.parent().expect("parent")).expect("mkdir");
        fs::write(&note, "# A\n").expect("write note");
        // Unrelated ref note that the plan does not own but the locator
        // index reads: mutating it must invalidate even an otherwise
        // unchanged plan.
        let other = bob_dir.join("ref/papers/b.md");
        fs::write(&other, "# B\n").expect("write other note");
        let plan = minimal_plan(&pdf, &note);
        let outcomes = vec![ScanPlanOutcome::Planned(Box::new(plan))];
        let ctx = ScanContext {
            index: crate::native::ref_tasks::RefTaskIndex::build(
                &config.bob_dir,
                &config.ref_dir,
            ),
            invocation_date: "2026-11-17".to_string(),
        };
        let fingerprint = locator_fingerprint(&ctx);
        let evidence = capture_scan_evidence(
            &config,
            std::slice::from_ref(&pdf),
            &[],
            &outcomes,
            &fingerprint,
        )
        .expect("capture evidence");
        let prepared = PreparedFullScan {
            config: config.clone(),
            invocation_date: "2026-11-17".to_string(),
            intake: Vec::new(),
            pdfs: vec![pdf.clone()],
            plan_outcomes: outcomes,
            evidence,
            draft_annotation_candidates: BTreeMap::new(),
            draft_rendered: BTreeMap::new(),
        };
        assert!(matches!(
            validate_prepared_scan(&prepared),
            PreparedValidation::Unchanged
        ));
        // Add an orphan ref task in the unrelated note: the plan's own
        // files are untouched, so only the shared locator fingerprint can
        // turn this stale.
        fs::write(&other, "# B\n\n- [ ] orphan #ref ^abc123\n")
            .expect("mutate other note with orphan");
        assert!(matches!(
            validate_prepared_scan(&prepared),
            PreparedValidation::Stale { .. }
        ));
    }

    #[test]
    fn reset_clears_finalization_without_duplicating() {
        let temp = tempfile::tempdir().expect("tempdir");
        let bob_dir = temp.path();
        let config = test_config(bob_dir);
        let pdf = bob_dir.join("lib/papers/a.pdf");
        let note = bob_dir.join("ref/papers/a.md");
        let mut plan = minimal_plan(&pdf, &note);
        let raw_candidates = vec![AnnotationTaskCandidate {
            identity: "^ann-9".to_string(),
            task_text: "follow up".to_string(),
            source_block_id: "src".to_string(),
            target: AnnotationTaskTarget::ReferenceNote,
            source_ref_note_path: note.clone(),
            processed_id: "ann-9".to_string(),
        }];
        plan.annotation_task_candidates = Vec::new();
        plan.annotation_tasks_created = 1;
        plan.annotation_tasks_skipped = 2;
        plan.annotation_task_relation = Some("related".to_string());
        plan.routed_intents
            .push(routed_intent(bob_dir.join("sase.md"), pdf.clone()));
        plan.preview_block_id = Some("ref-stale".to_string());
        plan.rendered_body = "mutated body".to_string();
        plan.stable_rendered_note = "mutated note".to_string();
        plan.stable_note_action = "update";
        let mut drafts_candidates = BTreeMap::new();
        drafts_candidates.insert(pdf.clone(), raw_candidates.clone());
        let mut drafts_rendered = BTreeMap::new();
        drafts_rendered.insert(
            pdf.clone(),
            ("body".to_string(), "note".to_string(), "none"),
        );
        let mut prepared = PreparedFullScan {
            config: config.clone(),
            invocation_date: "2026-11-17".to_string(),
            intake: Vec::new(),
            pdfs: vec![pdf.clone()],
            plan_outcomes: vec![ScanPlanOutcome::Planned(Box::new(plan))],
            evidence: capture_scan_evidence(
                &config,
                std::slice::from_ref(&pdf),
                &[],
                &[],
                "index",
            )
            .expect("capture evidence"),
            draft_annotation_candidates: drafts_candidates,
            draft_rendered: drafts_rendered,
        };
        reset_prepared_finalization(&mut prepared);
        let ScanPlanOutcome::Planned(plan) = &prepared.plan_outcomes[0] else {
            panic!("expected planned outcome");
        };
        assert_eq!(plan.annotation_task_candidates, raw_candidates);
        assert_eq!(plan.annotation_tasks_created, 0);
        assert_eq!(plan.annotation_tasks_skipped, 0);
        assert_eq!(plan.annotation_task_relation, None);
        assert!(plan.routed_intents.is_empty());
        assert_eq!(plan.preview_block_id, None);
        assert_eq!(plan.rendered_body, "body");
        assert_eq!(plan.stable_rendered_note, "note");
        assert_eq!(plan.stable_note_action, "none");
    }

    #[test]
    fn mutation_predicate_ignores_holds_and_plan_failures() {
        let temp = tempfile::tempdir().expect("tempdir");
        let pdf = temp.path().join("a.pdf");
        let note = temp.path().join("a.md");
        let plan = minimal_plan(&pdf, &note);
        assert!(!plan_has_actual_work(&plan));
        let prepared = PreparedFullScan {
            config: test_config(temp.path()),
            invocation_date: "2026-11-17".to_string(),
            intake: Vec::new(),
            pdfs: vec![pdf.clone()],
            plan_outcomes: vec![
                ScanPlanOutcome::Planned(Box::new(plan)),
                ScanPlanOutcome::Held(ScanHold {
                    pdf: pdf.clone(),
                    note_path: note.clone(),
                    title: "A".to_string(),
                    reason: HoldReason::DirtyTargets,
                    message: "held".to_string(),
                }),
                ScanPlanOutcome::Failed(ScanFailure::new(
                    pdf.clone(),
                    CommandError::new("plan failed"),
                )),
            ],
            evidence: ScanInputEvidence {
                pdf_hashes: BTreeMap::new(),
                file_states: BTreeMap::new(),
                lib_pdfs: Vec::new(),
                xlib_members: Vec::new(),
                intake_destinations: Vec::new(),
                index_fingerprint: String::new(),
            },
            draft_annotation_candidates: BTreeMap::new(),
            draft_rendered: BTreeMap::new(),
        };
        assert!(
            prepared_scan_has_actual_work(&prepared),
            "holds and failures still count as reportable work"
        );
        assert!(
            !prepared_scan_needs_mutation(&prepared),
            "holds and failures alone do not take writer locks"
        );
        let writes = unchanged_write_outcomes_for(&prepared.plan_outcomes);
        assert_eq!(
            writes.len(),
            1,
            "lock-free assembly emits one write outcome per Planned PDF"
        );
        match &writes[0] {
            ScanWriteOutcome::Written(report) => {
                assert_eq!(report.note_action, "none");
                assert_eq!(report.marker_action, "none");
            }
            other => panic!(
                "expected Written(none) for the unchanged Planned PDF, got {other:?}"
            ),
        }
    }

    #[test]
    fn intake_affected_pdfs_include_late_audio_library_pdf() {
        let temp = tempfile::tempdir().expect("tempdir");
        let lib = temp.path().join("lib/chat");
        fs::create_dir_all(&lib).expect("mkdir");
        let library_pdf = lib.join("listen.pdf");
        fs::write(&library_pdf, b"%PDF").expect("write pdf");
        let intake = vec![
            IntakeMove {
                source: temp.path().join("xlib/chat/fresh.pdf"),
                destination: lib.join("fresh.pdf"),
                companions: Vec::new(),
            },
            IntakeMove {
                source: temp.path().join("xlib/chat/listen.mp3"),
                destination: lib.join("listen.mp3"),
                companions: Vec::new(),
            },
        ];
        let affected = intake_affected_pdfs(&intake);
        assert_eq!(affected, vec![lib.join("fresh.pdf"), library_pdf],);
    }
}
