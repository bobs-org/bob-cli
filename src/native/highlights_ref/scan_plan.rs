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

#[cfg(test)]
use std::cell::Cell;

#[cfg(test)]
thread_local! {
    static TEST_INVOCATION_DATE: Cell<Option<&'static str>> = const { Cell::new(None) };
    static TEST_INVOCATION_DATE_READS: Cell<usize> = const { Cell::new(0) };
}

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

fn track_file(
    file_states: &mut BTreeMap<PathBuf, EvidenceFileState>,
    path: PathBuf,
) -> Result<()> {
    if !file_states.contains_key(&path) {
        let state = fingerprint_file(&path)?;
        file_states.insert(path, state);
    }
    Ok(())
}

/// Pin one invocation date for every retry, rebuild, and recovery path.
/// `None` resolves the clock once; `Some` is reused verbatim.
fn pin_invocation_date(invocation_date: Option<String>) -> String {
    invocation_date.unwrap_or_else(scan_invocation_date)
}

fn scan_invocation_date() -> String {
    #[cfg(test)]
    {
        TEST_INVOCATION_DATE_READS.with(|reads| reads.set(reads.get() + 1));
        if let Some(date) = TEST_INVOCATION_DATE.with(|cell| cell.get()) {
            return date.to_string();
        }
    }
    current_local_date()
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
    .with_code("inputs_changed")
}

pub(super) fn destination_asset_changed_error(reason: &str) -> CommandError {
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
    // Real comparison against the bytes used by each plan: the stored
    // `pdf_hashes` are the planner's read-time hashes (what each plan was
    // built from), while `current.pdf_hashes` would naively repopulate the
    // same old planner hashes for Planned outcomes. Re-hash the current
    // bytes on disk so a PDF edit after preparation invalidates instead of
    // comparing old hashes to themselves.
    {
        let mut fresh_pdf_hashes: BTreeMap<PathBuf, String> = BTreeMap::new();
        for pdf in prepared.evidence.pdf_hashes.keys() {
            if let Ok(hash) = super::io::sha256_file(pdf) {
                fresh_pdf_hashes.insert(pdf.clone(), hash);
            }
        }
        // PDFs that vanished are covered by file_states membership, but a
        // missing hash entry must also count as stale when the stored plan
        // expected bytes.
        if fresh_pdf_hashes != prepared.evidence.pdf_hashes {
            return PreparedValidation::Stale {
                reason: "PDF contents changed".to_string(),
            };
        }
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
        let dest_roots = dest_asset_roots(&prepared.config, &prepared.pdfs);
        let reason = first_state_mismatch(
            &prepared.evidence.file_states,
            &current.file_states,
            &dest_roots,
        );
        return PreparedValidation::Stale { reason };
    }
    PreparedValidation::Unchanged
}

fn dest_asset_roots(config: &Config, pdfs: &[PathBuf]) -> Vec<PathBuf> {
    let mut roots = Vec::new();
    for pdf in pdfs {
        if let Ok(note_path) = super::io::ref_note_path(config, pdf)
            && let Ok(assets_dir) = ref_note_assets_dir(&note_path)
        {
            roots.push(assets_dir);
        }
    }
    roots
}

fn is_dest_asset_path(path: &Path, dest_roots: &[PathBuf]) -> bool {
    dest_roots
        .iter()
        .any(|root| path == root || path.starts_with(root))
}

fn file_mismatch_reason(
    path: &Path,
    kind: &str,
    dest_roots: &[PathBuf],
) -> String {
    if is_dest_asset_path(path, dest_roots) {
        format!("destination asset changed: {}", path.display())
    } else {
        format!("input {kind}: {}", path.display())
    }
}

fn first_state_mismatch(
    prepared: &BTreeMap<PathBuf, EvidenceFileState>,
    current: &BTreeMap<PathBuf, EvidenceFileState>,
    dest_roots: &[PathBuf],
) -> String {
    for (path, old) in prepared {
        match current.get(path) {
            Some(new) if new == old => {}
            Some(_) => {
                return file_mismatch_reason(path, "changed", dest_roots);
            }
            None => {
                return file_mismatch_reason(path, "removed", dest_roots);
            }
        }
    }
    for path in current.keys() {
        if !prepared.contains_key(path) {
            return file_mismatch_reason(path, "added", dest_roots);
        }
    }
    "inputs changed".to_string()
}

/// Compare a pre-planning snapshot to post-planning evidence.
///
/// Post evidence tracks additional absent (`exists: false`) paths the
/// planner names; those are not creations. Newly present files that the
/// snapshot missed — including `text.markdown`, nested assets, and dest
/// copies — are stale. Intake destinations that were absent before a
/// move are expected and skipped.
fn read_set_changed(
    pre: &BTreeMap<PathBuf, EvidenceFileState>,
    post: &BTreeMap<PathBuf, EvidenceFileState>,
    intake: &[IntakeMove],
    dest_roots: &[PathBuf],
) -> Option<String> {
    for (path, old) in pre {
        match post.get(path) {
            Some(new) if new == old => {}
            Some(_) => {
                return Some(file_mismatch_reason(path, "changed", dest_roots));
            }
            None => {
                return Some(file_mismatch_reason(path, "removed", dest_roots));
            }
        }
    }
    for (path, new) in post {
        if pre.contains_key(path) || !new.exists {
            continue;
        }
        let was_intake_dest = intake.iter().any(|item| {
            &item.destination == path
                || item.companions.iter().any(|(_, dest)| dest == path)
        });
        if !was_intake_dest {
            return Some(file_mismatch_reason(path, "added", dest_roots));
        }
    }
    None
}

fn pdfs_only(members: &[PathBuf]) -> Vec<PathBuf> {
    let mut pdfs = members
        .iter()
        .filter(|path| is_pdf_path(path))
        .cloned()
        .collect::<Vec<_>>();
    pdfs.sort();
    pdfs
}

fn missing_library_error(config: &Config) -> CommandError {
    CommandError::new(format!(
        "library directory does not exist or is not a directory: {}",
        config.lib_dir.display()
    ))
}

/// Derive the planned PDF list from the membership snapshot so discovery
/// and the pre-planning read-set are the same enumeration. Audio members
/// stay on the membership lists and never enter `pdfs`.
fn planned_pdfs_from_membership(
    config: &Config,
    options: SyncOptions,
    intake: &[IntakeMove],
    lib_members: &[PathBuf],
    xlib_members: &[PathBuf],
) -> Result<Vec<PathBuf>> {
    let lib_pdfs = pdfs_only(lib_members);
    let xlib_pdfs = pdfs_only(xlib_members);
    if options.dry_run {
        if config.lib_dir.is_dir() {
            let mut paths = lib_pdfs;
            paths.extend(xlib_pdfs);
            paths.sort();
            return Ok(paths);
        }
        if !config.xlib_dir.is_dir() {
            return Err(missing_library_error(config));
        }
        return Ok(xlib_pdfs);
    }
    if config.lib_dir.is_dir() {
        return Ok(lib_pdfs);
    }
    if intake.is_empty() {
        return Err(missing_library_error(config));
    }
    Ok(Vec::new())
}

/// Ordered shared preparation from intake plus a coherent pre-planning
/// read-set snapshot. Library/xlib discovery is that snapshot: the planned
/// PDF list is the PDF subset of the same membership enumeration.
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
    invocation_date: Option<String>,
) -> Result<PreparedFullScan> {
    // Membership snapshot is discovery. A PDF that arrives after this
    // enumeration cannot sit in both pre/post snapshots while missing
    // from the planned list.
    let pre_lib = collect_lib_membership(config)?;
    let pre_xlib = collect_xlib_membership(config)?;
    let pdfs = planned_pdfs_from_membership(
        config, options, &intake, &pre_lib, &pre_xlib,
    )?;
    // Prospective output check for the combined intake/library set before
    // any planning: two queued PDFs (or a queued and an existing PDF) that
    // map to the same ref note must fail here, matching dry-run, without
    // moving anything. The mutation-window recheck covers races where a
    // colliding PDF arrives after this point. Dry runs already list xlib
    // sources in `pdfs`, so adding prospective lib destinations would
    // double-count the same logical PDFs; only write mode needs the
    // combined set here.
    {
        if options.dry_run {
            validate_output_collisions(config, &pdfs)?;
        } else {
            let mut combined = pdfs.clone();
            combined.extend(intake_affected_pdfs(&intake));
            combined.sort();
            combined.dedup();
            validate_output_collisions(config, &combined)?;
        }
    }
    // Coherent optimistic read set: snapshot membership and every input
    // the planner may read — both TextBundle text filenames, present or
    // absent sidecars, nested source assets, dest asset trees, image-note
    // source files named in those sidecars, audio companions, archive
    // metadata, vault Markdown, and config — before the pure planning
    // pass. Compared after planning; a source mismatch is a stale read
    // that the bounded retry rebuilds, and a dest-asset mismatch fails
    // closed because overwrite protection cannot restore.
    let mut pre_file_states = snapshot_vault_md_states(config)?;
    let pre_config = fingerprint_file(&super::bob_config::config_path())?;
    pre_file_states
        .insert(super::bob_config::config_path(), pre_config.clone());
    snapshot_pdf_companion_states(config, &pdfs, &mut pre_file_states)?;
    snapshot_intake_states(&intake, &mut pre_file_states)?;
    let mut pre_pdf_hashes: BTreeMap<PathBuf, String> = BTreeMap::new();
    for pdf in &pdfs {
        if let Ok(hash) = super::io::sha256_file(pdf) {
            pre_pdf_hashes.insert(pdf.clone(), hash);
        }
    }
    scan_test::pause("after_snapshot");
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
    // Private test seam: mutate sidecar/ref/config/parent inputs after
    // their reads but before evidence capture. Production builds compile
    // this into a no-op; tests use the handshake to plant same-length
    // edits or newly created inputs that the old plan did not select.
    scan_test::pause("before_evidence");
    let evidence = capture_scan_evidence(
        config,
        &pdfs,
        &intake,
        &plan_outcomes,
        &index_fingerprint,
    )?;
    // During-preparation coherence: the pure planning pass must not attach
    // newer evidence to an older plan. Compare the pre-planning read set
    // to the post-planning evidence; any membership or content mismatch
    // (including a PDF arriving mid-planning, same-length sidecar/ref/
    // config/parent/asset edits, or a newly created input the old plan
    // did not select) is stale so the bounded retry rebuilds with current
    // data.
    {
        let mut pre_sorted = pre_lib.clone();
        pre_sorted.sort();
        if pre_sorted != evidence.lib_pdfs {
            return Err(scan_inputs_changed_error(
                "library membership changed during planning",
            ));
        }
        let mut pre_x_sorted = pre_xlib.clone();
        pre_x_sorted.sort();
        if pre_x_sorted != evidence.xlib_members {
            return Err(scan_inputs_changed_error(
                "xlib membership changed during planning",
            ));
        }
        let expected_pdfs = planned_pdfs_from_membership(
            config,
            options,
            &intake,
            &evidence.lib_pdfs,
            &evidence.xlib_members,
        )?;
        if expected_pdfs != pdfs {
            return Err(scan_inputs_changed_error(
                "planned PDF list does not match snapshot membership",
            ));
        }
        let dest_roots = dest_asset_roots(config, &pdfs);
        if let Some(reason) = read_set_changed(
            &pre_file_states,
            &evidence.file_states,
            &intake,
            &dest_roots,
        ) {
            if is_destination_asset_stale(&reason) {
                return Err(destination_asset_changed_error(&format!(
                    "{reason} during planning"
                )));
            }
            return Err(scan_inputs_changed_error(&format!(
                "{reason} during planning"
            )));
        }
        if fingerprint_file(&super::bob_config::config_path())? != pre_config {
            return Err(scan_inputs_changed_error(
                "config changed during planning",
            ));
        }
        // Locator changes during planning invalidate every plan, even one
        // previously classified unchanged.
        let fresh_index = crate::native::ref_tasks::RefTaskIndex::build(
            &config.bob_dir,
            &config.ref_dir,
        );
        let fresh_ctx = ScanContext {
            index: fresh_index,
            invocation_date: invocation_date.clone(),
        };
        if locator_fingerprint(&fresh_ctx) != index_fingerprint {
            return Err(scan_inputs_changed_error(
                "reading-task locator changed during planning",
            ));
        }
        // Planner PDF hashes must match the post-planning bytes actually on
        // disk; otherwise the planner read older bytes than the evidence
        // attaches. Retain a real comparison against the bytes used by
        // each plan instead of repopulating both sides from the old plans.
        for outcome in &plan_outcomes {
            if let ScanPlanOutcome::Planned(plan) = outcome {
                let post_hash =
                    super::io::sha256_file(&plan.pdf).map_err(|error| {
                        scan_inputs_changed_error(&format!(
                            "PDF changed during planning: {}: {error}",
                            plan.pdf.display()
                        ))
                    })?;
                if post_hash != plan.marker.source_pdf_sha256 {
                    return Err(scan_inputs_changed_error(&format!(
                        "PDF contents changed during planning: {}",
                        plan.pdf.display()
                    )));
                }
                // Same-length content changes to the PDF itself that the
                // planner somehow missed are still caught here via the
                // pre hash: the pre bytes must equal the post bytes.
                if let Some(pre_hash) = pre_pdf_hashes.get(&plan.pdf)
                    && pre_hash != &post_hash
                {
                    return Err(scan_inputs_changed_error(&format!(
                        "PDF contents changed during planning: {}",
                        plan.pdf.display()
                    )));
                }
            }
        }
        // Input PDFs that were held/failed still need pre vs post coherence:
        // a same-length edit during planning must not be accepted.
        for pdf in &pdfs {
            if let Some(pre_hash) = pre_pdf_hashes.get(pdf) {
                if let Ok(post_hash) = super::io::sha256_file(pdf) {
                    if pre_hash != &post_hash {
                        return Err(scan_inputs_changed_error(&format!(
                            "PDF contents changed during planning: {}",
                            pdf.display()
                        )));
                    }
                } else {
                    return Err(scan_inputs_changed_error(&format!(
                        "input removed during planning: {}",
                        pdf.display()
                    )));
                }
            }
        }
    }
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

/// Convenience read-only preparation: discover intake, then snapshot
/// membership and plan from that same enumeration. Dry runs include xlib
/// PDFs. Write mode lists the existing library only (empty when `lib/` is
/// missing and intake is pending, so an xlib-only vault can still prepare
/// outside the locks).
pub(super) fn prepare_full_scan(
    config: &Config,
    options: SyncOptions,
    jobs: usize,
    invocation_date: Option<String>,
) -> Result<PreparedFullScan> {
    let intake = plan_xlib_intake(config)?;
    prepare_from_inputs(config, options, jobs, intake, invocation_date)
}

/// Bounded preparation retry: on an immediately stale validation (a source
/// changing during the pure planning pass), rebuild once outside any new
/// locks under the same invocation date. Destination-asset existence or
/// bytes changing during that pass fail closed without a retry: overwrite
/// protection cannot restore dest from source, and a retry would treat the
/// vandalized dest as the new baseline then collide at write. Persistent
/// churn returns the `inputs_changed` envelope with a rerun hint (destination
/// assets stay `scan_failed`); it never spins and never parses the whole
/// library while locked.
pub(super) fn prepare_full_scan_with_retry(
    config: &Config,
    options: SyncOptions,
    jobs: usize,
) -> Result<PreparedFullScan> {
    prepare_full_scan_with_retry_date(config, options, jobs, None)
}

pub(super) fn is_destination_asset_stale(reason: &str) -> bool {
    reason.contains("destination asset changed")
}

fn is_inputs_changed_during_planning(error: &CommandError) -> bool {
    error.code == Some("inputs_changed")
        && error
            .message
            .contains("scan inputs changed during planning")
        && !is_destination_asset_stale(&error.message)
}

pub(super) fn prepare_full_scan_with_retry_date(
    config: &Config,
    options: SyncOptions,
    jobs: usize,
    invocation_date: Option<String>,
) -> Result<PreparedFullScan> {
    // Resolve the clock once. Every retry, rebuild, and recovery path
    // reuses this exact value so a None call cannot pick a second date
    // across midnight. The finite retry budget stays one coherence retry
    // here plus the mutation-window rebuild in scan_apply; no retry runs
    // after an owned intake move.
    let invocation_date = pin_invocation_date(invocation_date);
    let first =
        prepare_full_scan(config, options, jobs, Some(invocation_date.clone()));
    let mut prepared = match first {
        Ok(prepared) => prepared,
        Err(error) => {
            // During-preparation coherence failure (a source changing during
            // the pure planning pass) rebuilds once outside any new locks
            // under the same invocation date. Destination-asset changes and
            // other errors (output collisions, intake collisions, hard
            // snapshot failures) fail immediately without a retry.
            if !is_inputs_changed_during_planning(&error) {
                return Err(error);
            }
            prepare_full_scan(config, options, jobs, Some(invocation_date))?
        }
    };
    match validate_prepared_scan(&prepared) {
        PreparedValidation::Unchanged => Ok(prepared),
        PreparedValidation::Failed(error) => Err(error),
        PreparedValidation::Stale { reason }
            if is_destination_asset_stale(&reason) =>
        {
            Err(destination_asset_changed_error(&reason))
        }
        PreparedValidation::Stale { .. } => {
            // Clear stale reservations before the single rebuild so a
            // combined intake/library finalization later cannot duplicate
            // intentions or retain stale preview IDs.
            reset_prepared_finalization(&mut prepared);
            let date = prepared.invocation_date.clone();
            let rebuilt = prepare_full_scan(config, options, jobs, Some(date))?;
            match validate_prepared_scan(&rebuilt) {
                PreparedValidation::Unchanged => Ok(rebuilt),
                PreparedValidation::Stale { reason }
                    if is_destination_asset_stale(&reason) =>
                {
                    Err(destination_asset_changed_error(&reason))
                }
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
    // Combined collision guards: intake planning alone cannot see conflicts
    // across the combined intake/library set. Validate prospective note
    // outputs and planned assets over the combined final plans before any
    // execution, using the existing guard helpers.
    validate_output_collisions(&prepared.config, &prepared.pdfs)?;
    {
        let combined_plans = prepared
            .plan_outcomes
            .iter()
            .filter_map(|outcome| match outcome {
                ScanPlanOutcome::Planned(plan) => Some(plan.as_ref()),
                ScanPlanOutcome::Held(_) | ScanPlanOutcome::Failed(_) => None,
            })
            .collect::<Vec<_>>();
        validate_planned_asset_collisions(&combined_plans)?;
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
    }
    snapshot_pdf_companion_states(config, pdfs, &mut file_states)?;
    for outcome in plan_outcomes {
        let (plan, hold_note, failed_pdf) = match outcome {
            ScanPlanOutcome::Planned(plan) => (Some(plan.as_ref()), None, None),
            ScanPlanOutcome::Held(hold) => (None, Some(hold), None),
            ScanPlanOutcome::Failed(failure) => {
                (None, None, Some(&failure.pdf))
            }
        };
        if let Some(plan) = plan {
            track_file(&mut file_states, plan.note_path.clone())?;
            if let Some(sidecar) = plan.sidecar_path.as_ref() {
                track_file(&mut file_states, sidecar.clone())?;
            }
            for asset in &plan.image_assets {
                track_file(&mut file_states, asset.dest_path.clone())?;
                track_file(&mut file_states, asset.source_path.clone())?;
            }
            for write in &plan.routed_task_note_writes {
                track_file(&mut file_states, write.path.clone())?;
            }
            for intent in &plan.routed_intents {
                track_file(&mut file_states, intent.path.clone())?;
            }
            if let Some(reading) = plan.reading_task_plan.as_ref() {
                match &reading.action {
                    ReadingTaskAction::Insert {
                        destination_route, ..
                    } => {
                        track_file(
                            &mut file_states,
                            config
                                .bob_dir
                                .join(format!("{destination_route}.md")),
                        )?;
                    }
                    ReadingTaskAction::LineEdit { .. } => {
                        if let Some(located) = plan.reading_located.as_ref() {
                            track_file(
                                &mut file_states,
                                config.bob_dir.join(&located.path),
                            )?;
                        }
                    }
                    ReadingTaskAction::NoWrite => {}
                }
            }
            // Archive/capture metadata consulted by `replaced_capture_hold`
            // and by reading/annotation finalization. Shared helper covers
            // the note, its annotated PDF, and its assets path.
            track_archive_for_note(config, &mut file_states, &plan.note_path)?;
        }
        if let Some(hold) = hold_note {
            track_file(&mut file_states, hold.note_path.clone())?;
            // Holds consult the same archive metadata (replaced-capture
            // holds) and ID-occupancy inputs; a hold caused by an earlier
            // version being absent must still invalidate when that input
            // appears.
            track_archive_for_note(config, &mut file_states, &hold.note_path)?;
        }
        if let Some(failed_pdf) = failed_pdf {
            // Failures carry the inputs that made the plan a failure: the
            // PDF itself (already tracked via the pdfs loop) plus its
            // prospective note and archive metadata, so a missing input
            // that later appears invalidates instead of staying failed.
            if let Ok(note_path) = super::io::ref_note_path(config, failed_pdf)
            {
                track_file(&mut file_states, note_path.clone())?;
                track_archive_for_note(config, &mut file_states, &note_path)?;
            }
        }
    }
    // Parent/routing/config inputs: the highlights config file plus every
    // vault Markdown note (ref-note preimages, routed/reading destinations,
    // parent/alias/routing inputs, archive/capture metadata, ID-occupancy
    // inputs). Explicitly enumerated relevant trees; unrelated vault media
    // is never hashed.
    track_file(&mut file_states, super::bob_config::config_path())?;
    {
        let vault_md = snapshot_vault_md_states(config)?;
        for (path, state) in vault_md {
            file_states.entry(path).or_insert(state);
        }
    }
    // Library and xlib membership plus prospective destination absence.
    // Propagate membership errors instead of treating unreadable trees as
    // empty/unchanged.
    let mut lib_pdfs = collect_lib_membership(config)?;
    lib_pdfs.sort();
    let mut xlib_members = collect_xlib_membership(config)?;
    xlib_members.sort();
    let mut intake_destinations = intake
        .iter()
        .flat_map(|item| {
            let mut paths = vec![item.destination.clone()];
            paths.extend(item.companions.iter().map(|(_, dest)| dest.clone()));
            paths
        })
        .collect::<Vec<_>>();
    intake_destinations.sort();
    snapshot_intake_states(intake, &mut file_states)?;
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
    if !config.lib_dir.is_dir() {
        return Ok(Vec::new());
    }
    let mut paths = Vec::new();
    super::doctor::collect_pdf_paths_from_dir(&config.lib_dir, &mut paths)?;
    // Late-audio membership also invalidates: an `.mp3` arriving beside a
    // planned PDF changes the plan. Propagate read errors instead of
    // treating unreadable membership as empty/unchanged.
    collect_audio_under(&config.lib_dir, &mut paths)?;
    Ok(paths)
}

fn collect_xlib_membership(config: &Config) -> Result<Vec<PathBuf>> {
    if !config.xlib_dir.is_dir() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    super::doctor::collect_pdf_paths_from_dir(&config.xlib_dir, &mut out)?;
    collect_audio_under(&config.xlib_dir, &mut out)?;
    Ok(out)
}

/// Conservative content-and-membership snapshot of the relevant Markdown
/// trees: every vault `.md` (ref notes, routed/reading destinations, parent
/// notes, archive records) plus sidecar `.md` companions. Explicitly
/// enumerated; unrelated vault media is never hashed. Errors propagate so
/// unreadable inputs fail closed instead of reading as empty/unchanged.
fn snapshot_vault_md_states(
    config: &Config,
) -> Result<BTreeMap<PathBuf, EvidenceFileState>> {
    let (relative_paths, warnings) =
        crate::native::capture_dependency_tasks::vault_note_paths(
            &config.bob_dir,
        );
    if !warnings.is_empty() {
        return Err(CommandError::new(format!(
            "scan vault Markdown enumeration: {}",
            warnings.join("; ")
        )));
    }
    let mut states = BTreeMap::new();
    for relative in relative_paths {
        let absolute = config.bob_dir.join(&relative);
        let state = fingerprint_file(&absolute)?;
        states.insert(absolute, state);
    }
    Ok(states)
}

/// Snapshot present/absent sidecars, both TextBundle text filenames,
/// nested source assets, dest asset trees, image-note source files named
/// in those sidecars, audio companions, prospective notes, and archive
/// metadata for each planned PDF. Conservative: inputs the planner may
/// later name are hashed here so a post-planning capture cannot attach
/// newer bytes to an older plan.
fn snapshot_pdf_companion_states(
    config: &Config,
    pdfs: &[PathBuf],
    file_states: &mut BTreeMap<PathBuf, EvidenceFileState>,
) -> Result<()> {
    let home = bob_env::home_dir();
    for pdf in pdfs {
        track_file(file_states, pdf.clone())?;
        let mut sidecar = pdf.clone();
        sidecar.set_extension("md");
        track_file(file_states, sidecar)?;
        let mut textbundle = pdf.clone();
        textbundle.set_extension("textbundle");
        track_tree_contents(file_states, &textbundle, "TextBundle")?;
        snapshot_image_note_sources(pdf, &home, file_states)?;
        for companion in audio_companion_candidates(pdf) {
            track_file(file_states, companion)?;
        }
        if let Ok(note_path) = super::io::ref_note_path(config, pdf) {
            track_file(file_states, note_path.clone())?;
            if let Ok(assets_dir) = ref_note_assets_dir(&note_path) {
                track_tree_contents(file_states, &assets_dir, "asset tree")?;
            }
            track_archive_for_note(config, file_states, &note_path)?;
        }
    }
    Ok(())
}

/// Fingerprint image-note source files the sidecar already names, including
/// a present or absent `~/tmp/screenshots/...` path. Planning later tracks
/// those same paths on `plan.image_assets`; without this, a file that
/// existed before planning looks like `input added` in the post evidence.
fn snapshot_image_note_sources(
    pdf: &Path,
    home: &Path,
    file_states: &mut BTreeMap<PathBuf, EvidenceFileState>,
) -> Result<()> {
    let mut candidates = vec![pdf.with_extension("md")];
    let bundle = pdf.with_extension("textbundle");
    for name in super::TEXTBUNDLE_TEXT_FILES {
        candidates.push(bundle.join(name));
    }
    for path in candidates {
        let text = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                continue;
            }
            Err(error) if error.kind() == std::io::ErrorKind::InvalidData => {
                continue;
            }
            Err(error) => {
                return Err(CommandError::new(format!(
                    "read image-note sidecar {}: {error}",
                    path.display()
                )));
            }
        };
        for line in text.lines() {
            for image_note in parse_image_note_refs(line) {
                let source =
                    expand_image_note_source(&image_note.source_text, home);
                track_file(file_states, source)?;
            }
        }
    }
    Ok(())
}

fn snapshot_intake_states(
    intake: &[IntakeMove],
    file_states: &mut BTreeMap<PathBuf, EvidenceFileState>,
) -> Result<()> {
    for item in intake {
        track_file(file_states, item.source.clone())?;
        track_file(file_states, item.destination.clone())?;
        for (source, dest) in &item.companions {
            track_file(file_states, source.clone())?;
            track_file(file_states, dest.clone())?;
        }
    }
    Ok(())
}

/// Track a directory tree's nested files with content hashes, not just
/// directory existence. `text.md`/`text.markdown` plus source or dest
/// assets the planner reads must invalidate on a same-length edit.
/// Enumeration errors fail closed instead of reading as unchanged.
fn track_tree_contents(
    file_states: &mut BTreeMap<PathBuf, EvidenceFileState>,
    root: &Path,
    what: &str,
) -> Result<()> {
    let state = fingerprint_file(root)?;
    file_states.entry(root.to_path_buf()).or_insert(state);
    if !root.is_dir() {
        return Ok(());
    }
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let entries = fs::read_dir(&dir).map_err(|error| {
            CommandError::new(format!("scan {what} {}: {error}", dir.display()))
        })?;
        for entry in entries {
            let entry = entry.map_err(|error| {
                CommandError::new(format!(
                    "scan {what} {}: {error}",
                    dir.display()
                ))
            })?;
            let path = entry.path();
            let kind = entry.file_type().map_err(|error| {
                CommandError::new(format!(
                    "stat {what} input {}: {error}",
                    path.display()
                ))
            })?;
            if kind.is_dir() {
                if path
                    .file_name()
                    .is_some_and(|name| name == ".git" || name == ".bob")
                {
                    continue;
                }
                stack.push(path);
            } else if kind.is_file() && !file_states.contains_key(&path) {
                let state = fingerprint_file(&path)?;
                file_states.insert(path, state);
            }
        }
    }
    Ok(())
}

/// Track archive/capture metadata consulted by `replaced_capture_hold` for
/// one live note: the archive note, its annotated PDF, and its assets path.
/// Shared for Planned, Held, and Failed outcomes so holds and failures do
/// not rely only on the plan's selected output paths.
fn track_archive_for_note(
    config: &Config,
    file_states: &mut BTreeMap<PathBuf, EvidenceFileState>,
    note_path: &Path,
) -> Result<()> {
    let note_rel = note_path
        .strip_prefix(&config.bob_dir)
        .map(|rel| rel.to_string_lossy().replace('\\', "/"))
        .unwrap_or_default();
    if note_rel.is_empty() {
        return Ok(());
    }
    let Ok(records) = super::overwrite::archive::records_for_live_note(
        &config.bob_dir,
        &note_rel,
    ) else {
        return Ok(());
    };
    for record in records {
        let note_abs = config.bob_dir.join(&record.note_path);
        if !file_states.contains_key(&note_abs) {
            file_states.insert(note_abs.clone(), fingerprint_file(&note_abs)?);
        }
        if let Some(pdf) = record.annotated_pdf.as_ref() {
            let pdf_abs = config.bob_dir.join(&pdf.path);
            if !file_states.contains_key(&pdf_abs) {
                file_states
                    .insert(pdf_abs.clone(), fingerprint_file(&pdf_abs)?);
            }
        }
        if let Some(assets) = record.assets_path.as_ref() {
            let assets_abs = config.bob_dir.join(assets);
            file_states.entry(assets_abs.clone()).or_insert_with(|| {
                EvidenceFileState {
                    exists: assets_abs.exists(),
                    sha256: None,
                }
            });
        }
    }
    Ok(())
}

fn collect_audio_under(directory: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    let entries = fs::read_dir(directory).map_err(|error| {
        CommandError::new(format!(
            "scan audio membership {}: {error}",
            directory.display()
        ))
    })?;
    let mut entries = entries
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(|error| {
            CommandError::new(format!(
                "scan audio membership {}: {error}",
                directory.display()
            ))
        })?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let path = entry.path();
        let kind = entry.file_type().map_err(|error| {
            CommandError::new(format!(
                "stat audio membership {}: {error}",
                path.display()
            ))
        })?;
        if kind.is_dir() {
            if path
                .file_name()
                .is_some_and(|name| name == ".git" || name == ".bob")
            {
                continue;
            }
            collect_audio_under(&path, out)?;
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
    Ok(())
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
        let mut plan = minimal_plan(&pdf, &note);
        // The stored planner hash must match the real bytes on disk;
        // validation now compares against current bytes instead of
        // repopulating both sides from the old plan.
        let real_hash = super::io::sha256_file(&pdf).expect("hash pdf");
        plan.marker.source_pdf_sha256 = real_hash.clone();
        plan.stable_metadata.source_pdf_sha256 = real_hash;
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
        let mut plan = minimal_plan(&pdf, &note);
        let real_hash = super::io::sha256_file(&pdf).expect("hash pdf");
        plan.marker.source_pdf_sha256 = real_hash.clone();
        plan.stable_metadata.source_pdf_sha256 = real_hash;
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

    #[test]
    fn combined_asset_collisions_reject_shared_destinations() {
        // Conflicting image assets across the combined intake/library set
        // must be rejected with the existing `output_collision` guard:
        // two different plans writing the same asset destination cannot
        // both execute, whether both are existing, both are intake, or one
        // of each.
        let temp = tempfile::tempdir().expect("tempdir");
        let existing_pdf = temp.path().join("lib/a.pdf");
        let existing_note = temp.path().join("ref/a.md");
        let mut existing = minimal_plan(&existing_pdf, &existing_note);
        existing.image_assets.push(ImageAssetWrite {
            annotation_order: 0,
            source_path: temp.path().join("src-a.png"),
            dest_path: temp.path().join("shared.png"),
            vault_relative_dest_path: PathBuf::from("shared.png"),
            source_sha256: "x".to_string(),
            block_id: "b".to_string(),
            action: ImageAssetAction::Copy,
        });
        let new_pdf = temp.path().join("lib/b.pdf");
        let new_note = temp.path().join("ref/b.md");
        let mut intake_plan = minimal_plan(&new_pdf, &new_note);
        intake_plan.image_assets.push(ImageAssetWrite {
            annotation_order: 0,
            source_path: temp.path().join("src-b.png"),
            dest_path: temp.path().join("shared.png"),
            vault_relative_dest_path: PathBuf::from("shared.png"),
            source_sha256: "y".to_string(),
            block_id: "b".to_string(),
            action: ImageAssetAction::Copy,
        });
        let plans = [&existing, &intake_plan];
        let error = validate_planned_asset_collisions(&plans)
            .expect_err("shared asset destination must collide");
        assert_eq!(error.code, Some("output_collision"));
        // Distinct destinations must pass.
        intake_plan.image_assets[0].dest_path = temp.path().join("other.png");
        let plans_ok = [&existing, &intake_plan];
        validate_planned_asset_collisions(&plans_ok)
            .expect("distinct assets must not collide");
    }

    fn set_test_invocation_date(date: Option<&'static str>) {
        TEST_INVOCATION_DATE.with(|cell| cell.set(date));
        TEST_INVOCATION_DATE_READS.with(|reads| reads.set(0));
    }

    fn test_invocation_date_reads() -> usize {
        TEST_INVOCATION_DATE_READS.with(|reads| reads.get())
    }

    #[test]
    fn invocation_date_is_pinned_before_the_first_prepare() {
        set_test_invocation_date(Some("2026-11-17"));
        let first = pin_invocation_date(None);
        TEST_INVOCATION_DATE.with(|cell| cell.set(Some("2026-11-18")));
        let retry = pin_invocation_date(Some(first.clone()));
        assert_eq!(first, "2026-11-17");
        assert_eq!(retry, "2026-11-17");
        assert_eq!(
            test_invocation_date_reads(),
            1,
            "a Some retry must not reread the clock"
        );
        set_test_invocation_date(None);
    }

    #[test]
    fn prepare_retry_wrapper_reads_the_clock_once() {
        let temp = tempfile::tempdir().expect("tempdir");
        let bob_dir = temp.path();
        fs::create_dir_all(bob_dir.join("lib")).expect("lib");
        let config = test_config(bob_dir);
        set_test_invocation_date(Some("2026-11-17"));
        let prepared = prepare_full_scan_with_retry_date(
            &config,
            SyncOptions {
                dry_run: false,
                write_pdf: false,
                prefer: None,
                ignore_unannotated_sidecar: false,
            },
            1,
            None,
        )
        .expect("empty library prepares");
        assert_eq!(prepared.invocation_date, "2026-11-17");
        assert_eq!(
            test_invocation_date_reads(),
            1,
            "None must resolve the clock once for every retry path"
        );
        set_test_invocation_date(None);
    }

    #[test]
    fn pre_snapshot_tracks_text_markdown_and_nested_assets() {
        let temp = tempfile::tempdir().expect("tempdir");
        let bob_dir = temp.path();
        let config = test_config(bob_dir);
        let pdf = bob_dir.join("lib/chat/live.pdf");
        fs::create_dir_all(pdf.parent().expect("parent")).expect("mkdir");
        fs::write(&pdf, b"%PDF-1.4 fake").expect("pdf");
        let bundle = pdf.with_extension("textbundle");
        let sidecar = bundle.join("text.markdown");
        let source = bundle.join("assets/figure.png");
        fs::create_dir_all(source.parent().expect("assets")).expect("assets");
        fs::write(&sidecar, "Comment: Old content.\n").expect("sidecar");
        fs::write(&source, b"asset-v1").expect("source");
        let dest_dir = bob_dir.join("ref/chat/live.assets");
        fs::create_dir_all(&dest_dir).expect("dest dir");
        let dest = dest_dir.join("fig.png");
        fs::write(&dest, b"dest-v1").expect("dest");
        let mut states = BTreeMap::new();
        snapshot_pdf_companion_states(
            &config,
            std::slice::from_ref(&pdf),
            &mut states,
        )
        .expect("snapshot");
        assert!(
            states.get(&sidecar).is_some_and(|state| state.exists),
            "text.markdown must be in the pre-planning read set"
        );
        assert!(
            states.get(&source).is_some_and(|state| state.exists),
            "nested source assets must be in the pre-planning read set"
        );
        assert!(
            states.get(&dest).is_some_and(|state| state.exists),
            "destination assets must be in the pre-planning read set"
        );
        let before = states.get(&sidecar).cloned().expect("sidecar state");
        fs::write(&sidecar, "Comment: New content.\n").expect("edit sidecar");
        let mut after = BTreeMap::new();
        snapshot_pdf_companion_states(
            &config,
            std::slice::from_ref(&pdf),
            &mut after,
        )
        .expect("resnapshot");
        assert_ne!(
            after.get(&sidecar),
            Some(&before),
            "same-length text.markdown edits must change the snapshot"
        );
    }

    #[test]
    fn pre_snapshot_tracks_image_note_sources() {
        let temp = tempfile::tempdir().expect("tempdir");
        let home = temp.path().join("home");
        let bob_dir = temp.path().join("vault");
        let _guard = crate::native::env::TestEnvGuard::set(&[(
            "HOME",
            Some(home.as_os_str()),
        )]);
        let screenshot = home.join("tmp/screenshots/20261018_104022.png");
        fs::create_dir_all(screenshot.parent().expect("parent"))
            .expect("screenshot dir");
        fs::write(&screenshot, b"image-note-source-bytes").expect("screenshot");
        let config = test_config(&bob_dir);
        let pdf = bob_dir.join("lib/papers/example.pdf");
        fs::create_dir_all(pdf.parent().expect("parent")).expect("mkdir");
        fs::write(&pdf, b"%PDF-1.4 fake").expect("pdf");
        fs::write(
            pdf.with_extension("md"),
            "\
## Page 1

~/tmp/screenshots/20261018_104022.png
",
        )
        .expect("sidecar");
        let mut states = BTreeMap::new();
        snapshot_pdf_companion_states(
            &config,
            std::slice::from_ref(&pdf),
            &mut states,
        )
        .expect("snapshot");
        assert!(
            states.get(&screenshot).is_some_and(|state| state.exists),
            "image-note source must be in the pre-planning read set"
        );
    }

    #[test]
    fn planned_pdfs_keep_audio_out_of_the_pdf_list() {
        let temp = tempfile::tempdir().expect("tempdir");
        let bob_dir = temp.path();
        let config = test_config(bob_dir);
        let pdf = bob_dir.join("lib/chat/live.pdf");
        let audio = bob_dir.join("lib/chat/live.mp3");
        fs::create_dir_all(pdf.parent().expect("parent")).expect("mkdir");
        fs::write(&pdf, b"%PDF").expect("pdf");
        fs::write(&audio, b"ID3").expect("audio");
        let members = vec![pdf.clone(), audio.clone()];
        let pdfs = planned_pdfs_from_membership(
            &config,
            SyncOptions {
                dry_run: false,
                write_pdf: false,
                prefer: None,
                ignore_unannotated_sidecar: false,
            },
            &[],
            &members,
            &[],
        )
        .expect("pdfs");
        assert_eq!(pdfs, vec![pdf]);
    }

    #[test]
    fn read_set_changed_detects_markdown_sidecar_bytes() {
        let mut pre = BTreeMap::new();
        let path = PathBuf::from("/tmp/live.textbundle/text.markdown");
        pre.insert(
            path.clone(),
            EvidenceFileState {
                exists: true,
                sha256: Some("old".to_string()),
            },
        );
        let mut post = pre.clone();
        assert_eq!(read_set_changed(&pre, &post, &[], &[]), None);
        post.insert(
            path.clone(),
            EvidenceFileState {
                exists: true,
                sha256: Some("new".to_string()),
            },
        );
        assert_eq!(
            read_set_changed(&pre, &post, &[], &[]),
            Some(format!("input changed: {}", path.display()))
        );
    }

    #[test]
    fn dest_asset_byte_change_is_not_retryable() {
        let dest_root = PathBuf::from("/tmp/ref/chat/live.assets");
        let dest = dest_root.join("fig.png");
        let mut pre = BTreeMap::new();
        pre.insert(
            dest.clone(),
            EvidenceFileState {
                exists: true,
                sha256: Some("old".to_string()),
            },
        );
        let mut post = pre.clone();
        post.insert(
            dest.clone(),
            EvidenceFileState {
                exists: true,
                sha256: Some("new".to_string()),
            },
        );
        let reason = read_set_changed(
            &pre,
            &post,
            &[],
            std::slice::from_ref(&dest_root),
        )
        .expect("dest change");
        assert!(
            reason.contains("destination asset changed"),
            "dest vandalism must be classified as a destination-asset change: {reason}"
        );
        let error = destination_asset_changed_error(&format!(
            "{reason} during planning"
        ));
        assert_eq!(error.code, Some("scan_failed"));
        assert!(error.message.contains("scan inputs changed"));
        assert!(
            !is_inputs_changed_during_planning(&error),
            "dest-asset changes must fail closed without a source retry"
        );
        let source = scan_inputs_changed_error(
            "input changed: /tmp/text.markdown during planning",
        );
        assert_eq!(source.code, Some("inputs_changed"));
        assert!(is_inputs_changed_during_planning(&source));
    }
}
