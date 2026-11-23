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
/// - reading-task locator and ID-occupancy inputs (per-PDF candidates plus
///   the non-candidate index rest fingerprint, destination/archive contents)
/// - archive/capture metadata consulted by holds (`replaced_capture_hold`)
/// - library and xlib membership plus prospective destination absence
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ScanInputEvidence {
    pub(super) pdf_hashes: BTreeMap<PathBuf, String>,
    pub(super) file_states: BTreeMap<PathBuf, EvidenceFileState>,
    pub(super) lib_pdfs: Vec<PathBuf>,
    pub(super) xlib_members: Vec<PathBuf>,
    pub(super) intake_destinations: Vec<PathBuf>,
    /// Path → owning PDFs. Shared targets keep every owner. Vault Markdown
    /// is never assigned to every PDF.
    pub(super) owners: BTreeMap<PathBuf, BTreeSet<PathBuf>>,
    pub(super) markdown_paths: BTreeSet<PathBuf>,
    pub(super) source_store_paths: BTreeSet<PathBuf>,
    pub(super) candidates:
        BTreeMap<PathBuf, Vec<crate::native::ref_tasks::LocatedRefTask>>,
    pub(super) observations: BTreeMap<PathBuf, PlanDependencyObservations>,
    pub(super) index_rest_fingerprint: String,
    pub(super) planned_pdfs: Vec<PathBuf>,
}

/// Typed coherence result. Unlocked preparation converges through
/// [`settle_prepared_scan`]; the lock-window stale branch classifies with
/// the same type and refreshes outside the writer locks on the shared
/// invocation budget.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum ScanChange {
    Unchanged,
    Refresh {
        pdfs: BTreeSet<PathBuf>,
        reason: String,
    },
    Rebuild {
        reason: String,
    },
    DestinationAsset {
        reason: String,
    },
}

/// One invocation-wide coherence allowance, carried with the prepared scan
/// so lock-window validation can continue the same bounded policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct ScanRefreshBudget {
    pub(super) refreshes_remaining: u8,
    pub(super) full_rebuild_available: bool,
}

impl Default for ScanRefreshBudget {
    fn default() -> Self {
        Self {
            refreshes_remaining: 4,
            full_rebuild_available: true,
        }
    }
}

/// Owned shared preparation result for one full scan.
pub(super) struct PreparedFullScan {
    pub(super) config: Config,
    pub(super) invocation_date: String,
    pub(super) intake: Vec<IntakeMove>,
    pub(super) pdfs: Vec<PathBuf>,
    pub(super) plan_outcomes: Vec<ScanPlanOutcome>,
    pub(super) evidence: ScanInputEvidence,
    pub(super) refresh_budget: ScanRefreshBudget,
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

fn index_rest_fingerprint(ctx: &ScanContext) -> String {
    fingerprint_bytes(ctx.index.non_candidate_debug().as_bytes())
}

fn vault_rel_forward(config: &Config, path: &Path) -> String {
    path.strip_prefix(&config.bob_dir)
        .map(|rel| rel.to_string_lossy().replace('\\', "/"))
        .unwrap_or_else(|_| path.to_string_lossy().replace('\\', "/"))
}

fn candidates_by_pdf(
    config: &Config,
    pdfs: &[PathBuf],
    ctx: &ScanContext,
) -> BTreeMap<PathBuf, Vec<crate::native::ref_tasks::LocatedRefTask>> {
    let mut candidates = BTreeMap::new();
    for pdf in pdfs {
        let rel = match super::io::ref_note_path(config, pdf) {
            Ok(note_path) => vault_rel_forward(config, &note_path),
            Err(_) => continue,
        };
        candidates.insert(pdf.clone(), ctx.index.candidates(&rel).to_vec());
    }
    candidates
}

fn add_owner(
    owners: &mut BTreeMap<PathBuf, BTreeSet<PathBuf>>,
    path: &Path,
    pdf: &Path,
) {
    owners
        .entry(path.to_path_buf())
        .or_default()
        .insert(pdf.to_path_buf());
}

fn owners_for_path<'a>(
    owners: &'a BTreeMap<PathBuf, BTreeSet<PathBuf>>,
    path: &Path,
) -> Option<&'a BTreeSet<PathBuf>> {
    let mut current = Some(path);
    while let Some(candidate) = current {
        if let Some(pdfs) = owners.get(candidate) {
            return Some(pdfs);
        }
        current = candidate.parent();
    }
    None
}

fn track_owned(
    file_states: &mut BTreeMap<PathBuf, EvidenceFileState>,
    owners: &mut BTreeMap<PathBuf, BTreeSet<PathBuf>>,
    path: PathBuf,
    pdf: &Path,
) -> Result<()> {
    track_file(file_states, path.clone())?;
    add_owner(owners, &path, pdf);
    Ok(())
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

/// Return the complete typed change classification for a prepared scan.
/// The PDF subset lets unlocked and lock-window callers refresh only the
/// plans whose inputs changed.
pub(super) fn classify_prepared_scan(
    prepared: &PreparedFullScan,
) -> Result<ScanChange> {
    let fresh_index = crate::native::ref_tasks::RefTaskIndex::build(
        &prepared.config.bob_dir,
        &prepared.config.ref_dir,
    );
    let fresh_ctx = ScanContext {
        index: fresh_index,
        invocation_date: prepared.invocation_date.clone(),
    };
    let mut current = capture_scan_evidence(
        &prepared.config,
        &prepared.pdfs,
        &prepared.intake,
        &prepared.plan_outcomes,
        &fresh_ctx,
    )?;
    // Real comparison against the bytes used by each plan: the stored
    // `pdf_hashes` are the planner's read-time hashes (what each plan was
    // built from), while `current.pdf_hashes` would naively repopulate the
    // same old planner hashes for Planned outcomes. Re-hash the current
    // bytes on disk so a PDF edit after preparation invalidates instead of
    // comparing old hashes to themselves.
    refresh_evidence_pdf_hashes(&mut current)?;
    let dest_roots = dest_asset_roots(&prepared.config, &prepared.pdfs);
    Ok(classify_scan_change_with_plan_hashes(
        &prepared.evidence,
        &current,
        &prepared.config,
        &dest_roots,
        &[],
        false,
        &prepared.plan_outcomes,
    ))
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

fn is_intake_dest(path: &Path, intake: &[IntakeMove]) -> bool {
    intake.iter().any(|item| {
        item.destination == path
            || item.companions.iter().any(|(_, dest)| dest == path)
    })
}

fn is_under(path: &Path, root: &Path) -> bool {
    path == root || path.starts_with(root)
}

fn is_root_markdown(config: &Config, path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("md"))
        && path.parent() == Some(config.bob_dir.as_path())
}

fn is_source_store_path(config: &Config, path: &Path) -> bool {
    is_under(path, &config.bob_dir.join(super::source_store::STORE_ROOT))
}

fn collect_link_markdown_paths(
    root: &Path,
    directory: &Path,
    paths: &mut Vec<PathBuf>,
) -> Result<()> {
    let entries = fs::read_dir(directory).map_err(|error| {
        CommandError::new(format!(
            "scan vault Markdown {}: {error}",
            directory.display()
        ))
    })?;
    let mut entries = entries
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(|error| {
            CommandError::new(format!(
                "scan vault Markdown {}: {error}",
                directory.display()
            ))
        })?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let path = entry.path();
        let file_type = entry.file_type().map_err(|error| {
            CommandError::new(format!(
                "stat vault Markdown {}: {error}",
                path.display()
            ))
        })?;
        if file_type.is_dir() {
            if entry
                .file_name()
                .to_str()
                .is_some_and(|name| name.starts_with('.'))
            {
                continue;
            }
            collect_link_markdown_paths(root, &path, paths)?;
        } else if file_type.is_file()
            && path
                .extension()
                .and_then(|ext| ext.to_str())
                .is_some_and(|ext| ext.eq_ignore_ascii_case("md"))
            && let Ok(relative) = path.strip_prefix(root)
        {
            paths.push(relative.to_path_buf());
        }
    }
    Ok(())
}

/// Union of Markdown paths planning and finalization actually read:
/// `vault_note_paths` plus the `vault_links` walker (dot-directories only).
/// Errors propagate so an unreadable tree cannot look unchanged.
fn collect_coherence_markdown_paths(config: &Config) -> Result<Vec<PathBuf>> {
    let (dep_paths, warnings) =
        crate::native::capture_dependency_tasks::vault_note_paths(
            &config.bob_dir,
        );
    if !warnings.is_empty() {
        return Err(CommandError::new(format!(
            "scan vault Markdown enumeration: {}",
            warnings.join("; ")
        )));
    }
    let mut link_paths = Vec::new();
    if config.bob_dir.is_dir() {
        collect_link_markdown_paths(
            &config.bob_dir,
            &config.bob_dir,
            &mut link_paths,
        )?;
    }
    let mut all = dep_paths.into_iter().collect::<BTreeSet<_>>();
    all.extend(link_paths);
    Ok(all.into_iter().collect())
}

fn snapshot_source_store_states(
    config: &Config,
) -> Result<BTreeMap<PathBuf, EvidenceFileState>> {
    let mut states = BTreeMap::new();
    let root = config.bob_dir.join(super::source_store::STORE_ROOT);
    track_tree_contents(&mut states, &root, "source store")?;
    Ok(states)
}

fn first_reason(reasons: &[String], fallback: &str) -> String {
    reasons
        .iter()
        .min()
        .cloned()
        .unwrap_or_else(|| fallback.to_string())
}

fn classify_scan_change(
    old: &ScanInputEvidence,
    new: &ScanInputEvidence,
    config: &Config,
    dest_roots: &[PathBuf],
    intake: &[IntakeMove],
    allow_intake_dest_adds: bool,
) -> ScanChange {
    let mut dest_reasons = Vec::new();
    let mut global_reasons = Vec::new();
    let mut refresh_pdfs = BTreeSet::new();
    let mut refresh_reasons = Vec::new();
    let mut finalization_reasons = Vec::new();

    if old.lib_pdfs != new.lib_pdfs {
        global_reasons.push("library membership changed".to_string());
    }
    if old.xlib_members != new.xlib_members {
        global_reasons.push("xlib membership changed".to_string());
    }
    if old.planned_pdfs != new.planned_pdfs {
        global_reasons.push(
            "planned PDF list does not match snapshot membership".to_string(),
        );
    }
    if old.intake_destinations != new.intake_destinations {
        global_reasons.push("intake destinations changed".to_string());
    }
    if old.markdown_paths != new.markdown_paths {
        global_reasons.push("Markdown membership changed".to_string());
    }
    if old.source_store_paths != new.source_store_paths {
        global_reasons.push("source-store membership changed".to_string());
    }

    let config_path = super::bob_config::config_path();
    match (
        old.file_states.get(&config_path),
        new.file_states.get(&config_path),
    ) {
        (Some(before), Some(after)) if before != after => {
            global_reasons.push("config changed".to_string());
        }
        (None, Some(_)) | (Some(_), None) => {
            global_reasons.push("config changed".to_string());
        }
        _ => {}
    }

    if old.index_rest_fingerprint != new.index_rest_fingerprint {
        finalization_reasons.push("reading-task locator changed".to_string());
    }

    let pdfs: BTreeSet<&PathBuf> = old
        .planned_pdfs
        .iter()
        .chain(new.planned_pdfs.iter())
        .collect();
    for pdf in pdfs {
        let before = old.candidates.get(pdf);
        let after = new.candidates.get(pdf);
        if before != after {
            refresh_pdfs.insert(pdf.clone());
            refresh_reasons.push(format!(
                "reading-task candidates changed: {}",
                pdf.display()
            ));
        }
    }

    for pdf in old.pdf_hashes.keys().chain(new.pdf_hashes.keys()) {
        if old.pdf_hashes.get(pdf) != new.pdf_hashes.get(pdf) {
            refresh_pdfs.insert(pdf.clone());
            refresh_reasons
                .push(format!("PDF contents changed: {}", pdf.display()));
        }
    }

    let mut diffs: Vec<(&Path, &str, bool)> = Vec::new();
    for (path, old_state) in &old.file_states {
        match new.file_states.get(path) {
            Some(new_state) if new_state == old_state => {}
            Some(new_state) => {
                diffs.push((path, "changed", new_state.exists));
            }
            None => diffs.push((path, "removed", false)),
        }
    }
    for (path, new_state) in &new.file_states {
        if old.file_states.contains_key(path) {
            continue;
        }
        if !new_state.exists {
            continue;
        }
        if allow_intake_dest_adds && is_intake_dest(path, intake) {
            continue;
        }
        diffs.push((path, "added", new_state.exists));
    }
    diffs.sort_by(|left, right| left.0.cmp(right.0).then(left.1.cmp(right.1)));

    let vault_link_consumers = old
        .observations
        .iter()
        .filter(|(_, obs)| obs.consulted_vault_links)
        .map(|(pdf, _)| pdf.clone())
        .collect::<BTreeSet<_>>();
    let lineage_consumers = old
        .observations
        .iter()
        .filter(|(_, obs)| obs.consulted_lineage)
        .map(|(pdf, _)| pdf.clone())
        .collect::<BTreeSet<_>>();
    let parent_consumers = old
        .observations
        .iter()
        .filter(|(_, obs)| obs.consulted_parent_targets)
        .map(|(pdf, _)| pdf.clone())
        .collect::<BTreeSet<_>>();
    let incomplete_failures = old
        .observations
        .iter()
        .filter(|(_, obs)| !obs.complete)
        .map(|(pdf, _)| pdf.clone())
        .collect::<BTreeSet<_>>();

    for (path, kind, _) in diffs {
        let reason = file_mismatch_reason(path, kind, dest_roots);
        if is_dest_asset_path(path, dest_roots) {
            dest_reasons.push(reason);
            continue;
        }
        if path == config_path.as_path() {
            continue;
        }
        if is_source_store_path(config, path)
            || old.source_store_paths.contains(path)
            || new.source_store_paths.contains(path)
        {
            global_reasons.push(reason);
            continue;
        }
        if is_under(path, &config.ref_dir) {
            global_reasons.push(reason);
            continue;
        }
        let markdown_content = old.markdown_paths.contains(path)
            && new.markdown_paths.contains(path)
            && kind == "changed";
        if markdown_content {
            let mut selected = BTreeSet::new();
            if let Some(owners) = owners_for_path(&old.owners, path) {
                selected.extend(owners.iter().cloned());
            }
            selected.extend(vault_link_consumers.iter().cloned());
            selected.extend(lineage_consumers.iter().cloned());
            if is_root_markdown(config, path) {
                selected.extend(parent_consumers.iter().cloned());
            }
            selected.extend(incomplete_failures.iter().cloned());
            if selected.is_empty() {
                finalization_reasons.push(reason);
            } else {
                refresh_pdfs.extend(selected);
                refresh_reasons.push(reason);
            }
            continue;
        }
        if let Some(owners) = owners_for_path(&old.owners, path) {
            refresh_pdfs.extend(owners.iter().cloned());
            refresh_reasons.push(reason);
            continue;
        }
        if let Some(owners) = owners_for_path(&new.owners, path) {
            refresh_pdfs.extend(owners.iter().cloned());
            refresh_reasons.push(reason);
            continue;
        }
        if old.markdown_paths.contains(path)
            || new.markdown_paths.contains(path)
        {
            global_reasons.push(reason);
            continue;
        }
        global_reasons.push(reason);
    }

    if !incomplete_failures.is_empty()
        && (!finalization_reasons.is_empty()
            || old.markdown_paths != new.markdown_paths
            || old.index_rest_fingerprint != new.index_rest_fingerprint)
    {
        refresh_pdfs.extend(incomplete_failures);
        if refresh_reasons.is_empty() {
            refresh_reasons.push(first_reason(
                &finalization_reasons,
                "reading-task locator changed",
            ));
        }
    }

    if let Some(reason) = dest_reasons.into_iter().min() {
        return ScanChange::DestinationAsset { reason };
    }
    if !global_reasons.is_empty() {
        return ScanChange::Rebuild {
            reason: first_reason(&global_reasons, "inputs changed"),
        };
    }
    if !refresh_pdfs.is_empty() {
        return ScanChange::Refresh {
            pdfs: refresh_pdfs,
            reason: first_reason(&refresh_reasons, "inputs changed"),
        };
    }
    if !finalization_reasons.is_empty() {
        return ScanChange::Refresh {
            pdfs: BTreeSet::new(),
            reason: first_reason(&finalization_reasons, "inputs changed"),
        };
    }
    ScanChange::Unchanged
}

/// Add a plan whose parsed PDF bytes no longer match the current disk hash.
/// This catches an edit that happened during parsing even if the bytes later
/// returned to the pre-planning snapshot; destination and global precedence
/// from the ordinary classifier remains authoritative.
fn classify_scan_change_with_plan_hashes(
    old: &ScanInputEvidence,
    new: &ScanInputEvidence,
    config: &Config,
    dest_roots: &[PathBuf],
    intake: &[IntakeMove],
    allow_intake_dest_adds: bool,
    outcomes: &[ScanPlanOutcome],
) -> ScanChange {
    let change = classify_scan_change(
        old,
        new,
        config,
        dest_roots,
        intake,
        allow_intake_dest_adds,
    );
    let stale_plans = outcomes
        .iter()
        .filter_map(|outcome| match outcome {
            ScanPlanOutcome::Planned(plan)
                if new.pdf_hashes.get(&plan.pdf)
                    != Some(&plan.marker.source_pdf_sha256) =>
            {
                Some(plan.pdf.clone())
            }
            ScanPlanOutcome::Planned(_)
            | ScanPlanOutcome::Held(_)
            | ScanPlanOutcome::Failed(_) => None,
        })
        .collect::<BTreeSet<_>>();
    if stale_plans.is_empty() {
        return change;
    }
    let reason = format!(
        "PDF changed while planning: {}",
        stale_plans
            .iter()
            .map(|pdf| pdf.display().to_string())
            .collect::<Vec<_>>()
            .join(", ")
    );
    match change {
        ScanChange::Unchanged => ScanChange::Refresh {
            pdfs: stale_plans,
            reason,
        },
        ScanChange::Refresh {
            mut pdfs,
            reason: existing_reason,
        } => {
            pdfs.extend(stale_plans);
            ScanChange::Refresh {
                pdfs,
                reason: reason.min(existing_reason),
            }
        }
        change @ (ScanChange::Rebuild { .. }
        | ScanChange::DestinationAsset { .. }) => change,
    }
}

/// Compare a pre-planning snapshot to post-planning evidence.
///
/// Post evidence tracks additional absent (`exists: false`) paths the
/// planner names; those are not creations. Newly present files that the
/// snapshot missed — including `text.markdown`, nested assets, and dest
/// copies — are stale. Intake destinations that were absent before a
/// move are expected and skipped.
#[cfg(test)]
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
            item.destination == *path
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
    refresh_budget: ScanRefreshBudget,
) -> Result<PreparedFullScan> {
    let attempt = prepare_from_inputs_once(
        config,
        options,
        jobs,
        intake,
        invocation_date,
        refresh_budget,
    )?;
    let PreparationAttempt {
        mut prepared,
        observed,
        change,
    } = attempt;
    if change == ScanChange::Unchanged {
        prepared.evidence = observed;
    }
    settle_prepared_scan(prepared, change, options, jobs)
}

struct PreparationAttempt {
    prepared: PreparedFullScan,
    observed: ScanInputEvidence,
    change: ScanChange,
}

/// One full initial plan. Coherence mismatches are returned alongside the
/// provisional drafts so the caller can refresh just affected PDFs.
fn prepare_from_inputs_once(
    config: &Config,
    options: SyncOptions,
    jobs: usize,
    intake: Vec<IntakeMove>,
    invocation_date: Option<String>,
    refresh_budget: ScanRefreshBudget,
) -> Result<PreparationAttempt> {
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
    // metadata, vault Markdown, source store, and config — before the pure
    // planning pass. Compared after planning; a source mismatch is a stale
    // read that the bounded retry rebuilds, and a dest-asset mismatch fails
    // closed because overwrite protection cannot restore.
    let mut pre_evidence = capture_pre_scan_evidence(config, &pdfs, &intake)?;
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
    pre_evidence.candidates = candidates_by_pdf(config, &pdfs, &ctx);
    pre_evidence.index_rest_fingerprint = index_rest_fingerprint(&ctx);
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
    // Private test seam: mutate sidecar/ref/config/parent inputs after
    // their reads but before evidence capture. Production builds compile
    // this into a no-op; tests use the handshake to plant same-length
    // edits or newly created inputs that the old plan did not select.
    scan_test::pause("before_evidence");
    let mut evidence =
        capture_scan_evidence(config, &pdfs, &intake, &plan_outcomes, &ctx)?;
    let fresh_ctx = ScanContext {
        index: crate::native::ref_tasks::RefTaskIndex::build(
            &config.bob_dir,
            &config.ref_dir,
        ),
        invocation_date: invocation_date.clone(),
    };
    evidence.candidates = candidates_by_pdf(config, &pdfs, &fresh_ctx);
    evidence.index_rest_fingerprint = index_rest_fingerprint(&fresh_ctx);
    evidence.planned_pdfs = planned_pdfs_from_membership(
        config,
        options,
        &intake,
        &evidence.lib_pdfs,
        &evidence.xlib_members,
    )?;
    refresh_evidence_pdf_hashes(&mut evidence)?;
    complete_comparison_evidence(&pre_evidence, &mut evidence)?;
    // Planning can discover routed targets, parent/link reads, and lineage
    // consumers that the conservative pre-plan snapshot could not name.
    // Keep those owners and observations attached to the old byte baseline
    // so a concurrent Markdown edit selects every affected plan.
    for (path, owners) in &evidence.owners {
        pre_evidence
            .owners
            .entry(path.clone())
            .or_default()
            .extend(owners.iter().cloned());
    }
    pre_evidence.observations = evidence.observations.clone();
    let change = classify_scan_change_with_plan_hashes(
        &pre_evidence,
        &evidence,
        config,
        &dest_asset_roots(config, &pdfs),
        &intake,
        true,
        &plan_outcomes,
    );
    Ok(PreparationAttempt {
        prepared: PreparedFullScan {
            config: config.clone(),
            invocation_date,
            intake,
            pdfs,
            plan_outcomes,
            // Keep the pre-plan snapshot as the last trusted baseline when
            // this pass is stale. Stable attempts adopt `evidence` above.
            evidence: pre_evidence,
            refresh_budget,
            draft_annotation_candidates: drafts_candidates,
            draft_rendered: drafts_rendered,
        },
        observed: evidence,
        change,
    })
}

fn refresh_evidence_pdf_hashes(evidence: &mut ScanInputEvidence) -> Result<()> {
    let pdfs = evidence.pdf_hashes.keys().cloned().collect::<Vec<_>>();
    for pdf in pdfs {
        let state = fingerprint_file(&pdf)?;
        if let Some(hash) = state.sha256 {
            evidence.pdf_hashes.insert(pdf, hash);
        } else {
            evidence.pdf_hashes.remove(&pdf);
        }
    }
    Ok(())
}

/// Keep prior dependencies in the comparison snapshot for this pass, even
/// when a replacement outcome is Held or Failed and no longer reports them.
/// Re-read the bytes so a removed or edited destination remains visible;
/// stable prepared evidence still adopts only the new outcomes.
fn complete_comparison_evidence(
    previous: &ScanInputEvidence,
    current: &mut ScanInputEvidence,
) -> Result<()> {
    for path in previous.file_states.keys() {
        current
            .file_states
            .insert(path.clone(), fingerprint_file(path)?);
    }
    for (path, owners) in &previous.owners {
        current
            .owners
            .entry(path.clone())
            .or_default()
            .extend(owners.iter().cloned());
    }
    Ok(())
}

/// Union live outcome dependencies into a pre-snapshot copy so its bytes are
/// compared with the flags/owners actually used by retained and newly
/// produced outcomes. Prior dependencies are preserved (never narrowed) so
/// removed inputs, destination edits, and Planned-to-Failed/Held transitions
/// stay visible; per-PDF granularity is kept instead of assigning every
/// Markdown file to every PDF.
fn merge_live_dependencies_into_comparison_old(
    old: &mut ScanInputEvidence,
    live: &ScanInputEvidence,
) {
    for (path, owners) in &live.owners {
        old.owners
            .entry(path.clone())
            .or_default()
            .extend(owners.iter().cloned());
    }
    for (pdf, live_obs) in &live.observations {
        old.observations
            .entry(pdf.clone())
            .and_modify(|old_obs| {
                old_obs.consulted_vault_links |= live_obs.consulted_vault_links;
                old_obs.consulted_parent_targets |=
                    live_obs.consulted_parent_targets;
                old_obs.consulted_lineage |= live_obs.consulted_lineage;
                old_obs.complete &= live_obs.complete;
            })
            .or_insert_with(|| live_obs.clone());
    }
}

fn validate_prepared_asset_collisions(
    prepared: &PreparedFullScan,
) -> Result<()> {
    let plans = prepared
        .plan_outcomes
        .iter()
        .filter_map(|outcome| match outcome {
            ScanPlanOutcome::Planned(plan) => Some(plan.as_ref()),
            ScanPlanOutcome::Held(_) | ScanPlanOutcome::Failed(_) => None,
        })
        .collect::<Vec<_>>();
    validate_planned_asset_collisions(&plans)
}

pub(super) fn full_rebuild_prepared(
    prepared: PreparedFullScan,
    options: SyncOptions,
    jobs: usize,
    reason: &str,
) -> Result<PreparedFullScan> {
    if !prepared.refresh_budget.full_rebuild_available {
        return Err(scan_inputs_changed_error(reason));
    }
    let budget = ScanRefreshBudget {
        full_rebuild_available: false,
        ..prepared.refresh_budget
    };
    prepare_full_scan(
        &prepared.config,
        options,
        jobs,
        Some(prepared.invocation_date.clone()),
        budget,
    )
}

/// Converge a provisional or freshly prepared scan using the invocation's
/// shared four-pass subset refresh budget and one full rebuild allowance.
pub(super) fn settle_prepared_scan(
    mut prepared: PreparedFullScan,
    mut change: ScanChange,
    options: SyncOptions,
    jobs: usize,
) -> Result<PreparedFullScan> {
    loop {
        match change {
            ScanChange::Unchanged => {
                validate_prepared_asset_collisions(&prepared)?;
                return Ok(prepared);
            }
            ScanChange::DestinationAsset { reason } => {
                return Err(destination_asset_changed_error(&reason));
            }
            ScanChange::Rebuild { reason } => {
                prepared =
                    full_rebuild_prepared(prepared, options, jobs, &reason)?;
                change = classify_prepared_scan(&prepared)?;
            }
            ScanChange::Refresh { pdfs, reason } => {
                if prepared.refresh_budget.refreshes_remaining == 0 {
                    prepared = full_rebuild_prepared(
                        prepared, options, jobs, &reason,
                    )?;
                    change = classify_prepared_scan(&prepared)?;
                    continue;
                }
                let (updated, next_change) =
                    refresh_prepared_once(prepared, &pdfs, options, jobs)?;
                prepared = updated;
                change = next_change;
            }
        }
    }
}

/// Refresh one ordered subset outside writer locks. The last trusted
/// evidence is compared to a fresh pre-snapshot before any plan is
/// replaced, so edits arriving between detection and refresh join the same
/// pass. A stable result adopts the clean post-snapshot (live outcomes
/// only); a stale result retains the pre bytes with the union of prior and
/// live dependencies as the next trusted baseline.
pub(super) fn refresh_prepared_once(
    mut prepared: PreparedFullScan,
    pending_pdfs: &BTreeSet<PathBuf>,
    options: SyncOptions,
    jobs: usize,
) -> Result<(PreparedFullScan, ScanChange)> {
    // Let adversarial tests place a second edit after stale detection but
    // before this pass captures its fresh input baseline.
    scan_test::pause("before_refresh");
    let ctx = ScanContext {
        index: crate::native::ref_tasks::RefTaskIndex::build(
            &prepared.config.bob_dir,
            &prepared.config.ref_dir,
        ),
        invocation_date: prepared.invocation_date.clone(),
    };
    let mut pre = capture_scan_evidence(
        &prepared.config,
        &prepared.pdfs,
        &prepared.intake,
        &prepared.plan_outcomes,
        &ctx,
    )?;
    pre.planned_pdfs = planned_pdfs_from_membership(
        &prepared.config,
        options,
        &prepared.intake,
        &pre.lib_pdfs,
        &pre.xlib_members,
    )?;
    refresh_evidence_pdf_hashes(&mut pre)?;
    complete_comparison_evidence(&prepared.evidence, &mut pre)?;
    let dest_roots = dest_asset_roots(&prepared.config, &prepared.pdfs);
    let latest = classify_scan_change_with_plan_hashes(
        &prepared.evidence,
        &pre,
        &prepared.config,
        &dest_roots,
        &prepared.intake,
        false,
        &prepared.plan_outcomes,
    );
    match &latest {
        change @ (ScanChange::DestinationAsset { .. }
        | ScanChange::Rebuild { .. }) => {
            return Ok((prepared, (*change).clone()));
        }
        ScanChange::Unchanged | ScanChange::Refresh { .. } => {}
    }

    let mut selected = pending_pdfs.clone();
    if let ScanChange::Refresh { pdfs, .. } = &latest {
        selected.extend(pdfs.iter().cloned());
    }
    prepared.refresh_budget.refreshes_remaining -= 1;
    let selected = prepared
        .pdfs
        .iter()
        .filter(|pdf| selected.contains(*pdf))
        .cloned()
        .collect::<Vec<_>>();

    // This pre-snapshot is the baseline for plans retained from the
    // previous pass and every replacement produced below.
    prepared.evidence = pre.clone();
    reset_prepared_finalization(&mut prepared);
    let selected_set = selected.iter().cloned().collect::<BTreeSet<_>>();
    prepared
        .draft_annotation_candidates
        .retain(|pdf, _| !selected_set.contains(pdf));
    prepared
        .draft_rendered
        .retain(|pdf, _| !selected_set.contains(pdf));
    prepared
        .plan_outcomes
        .retain(|outcome| !selected_set.contains(plan_outcome_pdf(outcome)));

    let replacements = if selected.is_empty() {
        Vec::new()
    } else {
        plan_pdfs(&prepared.config, &selected, options, jobs, &ctx)
    };
    for outcome in &replacements {
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
    prepared.plan_outcomes.extend(replacements);
    prepared.plan_outcomes.sort_by_key(|outcome| {
        prepared
            .pdfs
            .iter()
            .position(|pdf| pdf == plan_outcome_pdf(outcome))
            .unwrap_or(usize::MAX)
    });
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

    scan_test::pause("before_refresh_evidence");
    // Read the shared index again after finalization. If it changed while
    // drafts were being refreshed, the next pass will re-finalize against
    // the current index without accepting stale reservations.
    let post_ctx = ScanContext {
        index: crate::native::ref_tasks::RefTaskIndex::build(
            &prepared.config.bob_dir,
            &prepared.config.ref_dir,
        ),
        invocation_date: prepared.invocation_date.clone(),
    };
    let mut post = capture_scan_evidence(
        &prepared.config,
        &prepared.pdfs,
        &prepared.intake,
        &prepared.plan_outcomes,
        &post_ctx,
    )?;
    post.planned_pdfs = planned_pdfs_from_membership(
        &prepared.config,
        options,
        &prepared.intake,
        &post.lib_pdfs,
        &post.xlib_members,
    )?;
    refresh_evidence_pdf_hashes(&mut post)?;
    // Live outcome evidence (retained plus newly produced) drives the
    // pre/post comparison; comparison-only owners must not leak into the
    // stored baseline. Keep a clean copy for stable adoption, then compare
    // the pre bytes against the live dependencies.
    let clean_post = post.clone();
    let mut comparison_old = pre.clone();
    merge_live_dependencies_into_comparison_old(
        &mut comparison_old,
        &clean_post,
    );
    let mut comparison_new = clean_post.clone();
    complete_comparison_evidence(&comparison_old, &mut comparison_new)?;
    let change = classify_scan_change_with_plan_hashes(
        &comparison_old,
        &comparison_new,
        &prepared.config,
        &dest_roots,
        &prepared.intake,
        false,
        &prepared.plan_outcomes,
    );
    if change == ScanChange::Unchanged {
        prepared.evidence = clean_post;
    } else {
        // Retain the pre bytes with the union of prior and live dependencies
        // so the next pass classifies the new drafts correctly.
        prepared.evidence = comparison_old;
    }
    Ok((prepared, change))
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
    refresh_budget: ScanRefreshBudget,
) -> Result<PreparedFullScan> {
    let intake = plan_xlib_intake(config)?;
    prepare_from_inputs(
        config,
        options,
        jobs,
        intake,
        invocation_date,
        refresh_budget,
    )
}

/// Bounded unlocked preparation convergence. Ordinary changes refresh only
/// the affected PDFs (or just shared finalization); global changes and an
/// exhausted subset budget use the one full rebuild allowance. Destination
/// asset changes fail closed because overwrite protection cannot safely
/// adopt those bytes as a new baseline. Sustained churn returns
/// `inputs_changed` with a rerun hint, and no refresh runs under writer locks.
pub(super) fn prepare_full_scan_with_retry(
    config: &Config,
    options: SyncOptions,
    jobs: usize,
) -> Result<PreparedFullScan> {
    prepare_full_scan_with_retry_date(config, options, jobs, None)
}

#[cfg(test)]
fn is_inputs_changed_during_planning(error: &CommandError) -> bool {
    error.code == Some("inputs_changed")
        && error
            .message
            .contains("scan inputs changed during planning")
        && !error.message.contains("destination asset changed")
}

pub(super) fn prepare_full_scan_with_retry_date(
    config: &Config,
    options: SyncOptions,
    jobs: usize,
    invocation_date: Option<String>,
) -> Result<PreparedFullScan> {
    // Resolve the clock once. Every subset refresh and rebuild reuses this
    // value. The budget is attached to the prepared scan for continuation
    // during lock revalidation; intake execution itself never retries.
    let invocation_date = pin_invocation_date(invocation_date);
    let prepared = prepare_full_scan(
        config,
        options,
        jobs,
        Some(invocation_date),
        ScanRefreshBudget::default(),
    )?;
    let change = classify_prepared_scan(&prepared)?;
    settle_prepared_scan(prepared, change, options, jobs)
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

fn capture_shared_inventory(
    config: &Config,
    pdfs: &[PathBuf],
    intake: &[IntakeMove],
    file_states: &mut BTreeMap<PathBuf, EvidenceFileState>,
    owners: &mut BTreeMap<PathBuf, BTreeSet<PathBuf>>,
) -> Result<(
    Vec<PathBuf>,
    Vec<PathBuf>,
    Vec<PathBuf>,
    BTreeSet<PathBuf>,
    BTreeSet<PathBuf>,
)> {
    snapshot_pdf_companion_states(config, pdfs, file_states, owners)?;
    track_file(file_states, super::bob_config::config_path())?;
    let vault_md = snapshot_vault_md_states(config)?;
    let markdown_paths = vault_md.keys().cloned().collect::<BTreeSet<_>>();
    for (path, state) in vault_md {
        file_states.entry(path).or_insert(state);
    }
    let source_store = snapshot_source_store_states(config)?;
    let source_store_paths =
        source_store.keys().cloned().collect::<BTreeSet<_>>();
    for (path, state) in source_store {
        file_states.entry(path).or_insert(state);
    }
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
    snapshot_intake_states(intake, file_states)?;
    Ok((
        lib_pdfs,
        xlib_members,
        intake_destinations,
        markdown_paths,
        source_store_paths,
    ))
}

fn capture_pre_scan_evidence(
    config: &Config,
    pdfs: &[PathBuf],
    intake: &[IntakeMove],
) -> Result<ScanInputEvidence> {
    let mut file_states = BTreeMap::new();
    let mut owners = BTreeMap::new();
    let mut pdf_hashes = BTreeMap::new();
    for pdf in pdfs {
        if let Ok(hash) = super::io::sha256_file(pdf) {
            pdf_hashes.insert(pdf.clone(), hash);
        }
    }
    let (
        lib_pdfs,
        xlib_members,
        intake_destinations,
        markdown_paths,
        source_store_paths,
    ) = capture_shared_inventory(
        config,
        pdfs,
        intake,
        &mut file_states,
        &mut owners,
    )?;
    Ok(ScanInputEvidence {
        pdf_hashes,
        file_states,
        lib_pdfs,
        xlib_members,
        intake_destinations,
        owners,
        markdown_paths,
        source_store_paths,
        candidates: BTreeMap::new(),
        observations: BTreeMap::new(),
        index_rest_fingerprint: String::new(),
        planned_pdfs: pdfs.to_vec(),
    })
}

/// Capture validation evidence together with the bytes/index used for
/// planning. Explicit trees (all vault-absolute):
/// `lib_dir` PDFs, `xlib_dir` members plus companions, each plan's
/// sidecar/companions, ref-note preimages, routed/reading target-note
/// preimages, the highlights config file, archive records consulted by
/// `replaced_capture_hold`, the source store, and prospective
/// intake-destination absence.
fn capture_scan_evidence(
    config: &Config,
    pdfs: &[PathBuf],
    intake: &[IntakeMove],
    plan_outcomes: &[ScanPlanOutcome],
    ctx: &ScanContext,
) -> Result<ScanInputEvidence> {
    let mut pdf_hashes: BTreeMap<PathBuf, String> = BTreeMap::new();
    let mut file_states: BTreeMap<PathBuf, EvidenceFileState> = BTreeMap::new();
    let mut owners: BTreeMap<PathBuf, BTreeSet<PathBuf>> = BTreeMap::new();
    let mut observations: BTreeMap<PathBuf, PlanDependencyObservations> =
        BTreeMap::new();

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
    let (
        lib_pdfs,
        xlib_members,
        intake_destinations,
        markdown_paths,
        source_store_paths,
    ) = capture_shared_inventory(
        config,
        pdfs,
        intake,
        &mut file_states,
        &mut owners,
    )?;
    for outcome in plan_outcomes {
        match outcome {
            ScanPlanOutcome::Planned(plan) => {
                observations
                    .insert(plan.pdf.clone(), plan.observations.clone());
                track_owned(
                    &mut file_states,
                    &mut owners,
                    plan.note_path.clone(),
                    &plan.pdf,
                )?;
                if let Some(sidecar) = plan.sidecar_path.as_ref() {
                    track_owned(
                        &mut file_states,
                        &mut owners,
                        sidecar.clone(),
                        &plan.pdf,
                    )?;
                }
                for asset in &plan.image_assets {
                    track_owned(
                        &mut file_states,
                        &mut owners,
                        asset.dest_path.clone(),
                        &plan.pdf,
                    )?;
                    track_owned(
                        &mut file_states,
                        &mut owners,
                        asset.source_path.clone(),
                        &plan.pdf,
                    )?;
                }
                for write in &plan.routed_task_note_writes {
                    track_owned(
                        &mut file_states,
                        &mut owners,
                        write.path.clone(),
                        &plan.pdf,
                    )?;
                }
                for intent in &plan.routed_intents {
                    track_owned(
                        &mut file_states,
                        &mut owners,
                        intent.path.clone(),
                        &plan.pdf,
                    )?;
                }
                if let Some(reading) = plan.reading_task_plan.as_ref() {
                    match &reading.action {
                        ReadingTaskAction::Insert {
                            destination_route, ..
                        } => {
                            track_owned(
                                &mut file_states,
                                &mut owners,
                                config
                                    .bob_dir
                                    .join(format!("{destination_route}.md")),
                                &plan.pdf,
                            )?;
                        }
                        ReadingTaskAction::LineEdit { .. } => {
                            if let Some(located) = plan.reading_located.as_ref()
                            {
                                track_owned(
                                    &mut file_states,
                                    &mut owners,
                                    config.bob_dir.join(&located.path),
                                    &plan.pdf,
                                )?;
                            }
                        }
                        ReadingTaskAction::NoWrite => {}
                    }
                }
                track_archive_for_note(
                    config,
                    &mut file_states,
                    &mut owners,
                    &plan.note_path,
                    Some(&plan.pdf),
                )?;
            }
            ScanPlanOutcome::Held(hold) => {
                observations.insert(
                    hold.pdf.clone(),
                    PlanDependencyObservations::default(),
                );
                track_owned(
                    &mut file_states,
                    &mut owners,
                    hold.note_path.clone(),
                    &hold.pdf,
                )?;
                track_archive_for_note(
                    config,
                    &mut file_states,
                    &mut owners,
                    &hold.note_path,
                    Some(&hold.pdf),
                )?;
            }
            ScanPlanOutcome::Failed(failure) => {
                observations
                    .insert(failure.pdf.clone(), failure.observations.clone());
                if let Ok(note_path) =
                    super::io::ref_note_path(config, &failure.pdf)
                {
                    track_owned(
                        &mut file_states,
                        &mut owners,
                        note_path.clone(),
                        &failure.pdf,
                    )?;
                    track_archive_for_note(
                        config,
                        &mut file_states,
                        &mut owners,
                        &note_path,
                        Some(&failure.pdf),
                    )?;
                }
            }
        }
    }
    Ok(ScanInputEvidence {
        pdf_hashes,
        file_states,
        lib_pdfs,
        xlib_members,
        intake_destinations,
        owners,
        markdown_paths,
        source_store_paths,
        candidates: candidates_by_pdf(config, pdfs, ctx),
        observations,
        index_rest_fingerprint: index_rest_fingerprint(ctx),
        planned_pdfs: pdfs.to_vec(),
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
/// trees: the union of `vault_note_paths` and the `vault_links` walker.
/// Errors propagate so unreadable inputs fail closed instead of reading
/// as empty/unchanged.
fn snapshot_vault_md_states(
    config: &Config,
) -> Result<BTreeMap<PathBuf, EvidenceFileState>> {
    let relative_paths = collect_coherence_markdown_paths(config)?;
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
/// newer bytes to an older plan. Each companion is owned by that PDF.
fn snapshot_pdf_companion_states(
    config: &Config,
    pdfs: &[PathBuf],
    file_states: &mut BTreeMap<PathBuf, EvidenceFileState>,
    owners: &mut BTreeMap<PathBuf, BTreeSet<PathBuf>>,
) -> Result<()> {
    let home = bob_env::home_dir();
    for pdf in pdfs {
        track_owned(file_states, owners, pdf.clone(), pdf)?;
        let mut sidecar = pdf.clone();
        sidecar.set_extension("md");
        track_owned(file_states, owners, sidecar, pdf)?;
        let mut textbundle = pdf.clone();
        textbundle.set_extension("textbundle");
        track_tree_contents(file_states, &textbundle, "TextBundle")?;
        assign_tree_owner(owners, file_states, &textbundle, pdf);
        snapshot_image_note_sources(pdf, &home, file_states, owners)?;
        for companion in audio_companion_candidates(pdf) {
            track_owned(file_states, owners, companion, pdf)?;
        }
        if let Ok(note_path) = super::io::ref_note_path(config, pdf) {
            track_owned(file_states, owners, note_path.clone(), pdf)?;
            if let Ok(assets_dir) = ref_note_assets_dir(&note_path) {
                track_tree_contents(file_states, &assets_dir, "asset tree")?;
                assign_tree_owner(owners, file_states, &assets_dir, pdf);
            }
            track_archive_for_note(
                config,
                file_states,
                owners,
                &note_path,
                Some(pdf),
            )?;
        }
    }
    Ok(())
}

fn assign_tree_owner(
    owners: &mut BTreeMap<PathBuf, BTreeSet<PathBuf>>,
    file_states: &BTreeMap<PathBuf, EvidenceFileState>,
    root: &Path,
    pdf: &Path,
) {
    for path in file_states.keys() {
        if path == root || path.starts_with(root) {
            add_owner(owners, path, pdf);
        }
    }
}

/// Fingerprint image-note source files the sidecar already names, including
/// a present or absent `~/tmp/screenshots/...` path. Planning later tracks
/// those same paths on `plan.image_assets`; without this, a file that
/// existed before planning looks like `input added` in the post evidence.
fn snapshot_image_note_sources(
    pdf: &Path,
    home: &Path,
    file_states: &mut BTreeMap<PathBuf, EvidenceFileState>,
    owners: &mut BTreeMap<PathBuf, BTreeSet<PathBuf>>,
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
                track_owned(file_states, owners, source, pdf)?;
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
    owners: &mut BTreeMap<PathBuf, BTreeSet<PathBuf>>,
    note_path: &Path,
    pdf: Option<&Path>,
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
        if let Some(owner) = pdf {
            add_owner(owners, &note_abs, owner);
        }
        if let Some(annotated) = record.annotated_pdf.as_ref() {
            let pdf_abs = config.bob_dir.join(&annotated.path);
            if !file_states.contains_key(&pdf_abs) {
                file_states
                    .insert(pdf_abs.clone(), fingerprint_file(&pdf_abs)?);
            }
            if let Some(owner) = pdf {
                add_owner(owners, &pdf_abs, owner);
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
            if let Some(owner) = pdf {
                add_owner(owners, &assets_abs, owner);
            }
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
            observations: PlanDependencyObservations::default(),
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
        let evidence = capture_scan_evidence(
            &config,
            std::slice::from_ref(&pdf),
            &[],
            &outcomes,
            &ctx,
        )
        .expect("capture evidence");
        let prepared = PreparedFullScan {
            config: config.clone(),
            invocation_date: "2026-11-17".to_string(),
            intake: Vec::new(),
            pdfs: vec![pdf.clone()],
            plan_outcomes: outcomes,
            evidence,
            refresh_budget: ScanRefreshBudget::default(),
            draft_annotation_candidates: BTreeMap::new(),
            draft_rendered: BTreeMap::new(),
        };
        assert!(matches!(
            classify_prepared_scan(&prepared).expect("classify"),
            ScanChange::Unchanged
        ));
        // Same length, different bytes must still invalidate.
        fs::write(&note, "# B\n").expect("edit note same length");
        assert!(matches!(
            classify_prepared_scan(&prepared).expect("classify"),
            ScanChange::Refresh { .. } | ScanChange::Rebuild { .. }
        ));
        fs::write(&note, "# A\n").expect("restore note");
        assert!(matches!(
            classify_prepared_scan(&prepared).expect("classify"),
            ScanChange::Unchanged
        ));
        fs::remove_file(&note).expect("remove note");
        assert!(matches!(
            classify_prepared_scan(&prepared).expect("classify"),
            ScanChange::Refresh { .. }
                | ScanChange::Rebuild { .. }
                | ScanChange::DestinationAsset { .. }
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
        let evidence = capture_scan_evidence(
            &config,
            std::slice::from_ref(&pdf),
            &[],
            &outcomes,
            &ctx,
        )
        .expect("capture evidence");
        let prepared = PreparedFullScan {
            config: config.clone(),
            invocation_date: "2026-11-17".to_string(),
            intake: Vec::new(),
            pdfs: vec![pdf.clone()],
            plan_outcomes: outcomes,
            evidence,
            refresh_budget: ScanRefreshBudget::default(),
            draft_annotation_candidates: BTreeMap::new(),
            draft_rendered: BTreeMap::new(),
        };
        assert!(matches!(
            classify_prepared_scan(&prepared).expect("classify"),
            ScanChange::Unchanged
        ));
        // Add an orphan ref task in the unrelated note: the plan's own
        // files are untouched, so only the shared locator fingerprint can
        // turn this stale.
        fs::write(&other, "# B\n\n- [ ] orphan #ref ^abc123\n")
            .expect("mutate other note with orphan");
        // The orphan lives under `ref_dir`, so the conservative global rule
        // wins over the shared locator fingerprint.
        assert!(matches!(
            classify_prepared_scan(&prepared).expect("classify"),
            ScanChange::Rebuild { .. }
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
                &ScanContext {
                    index: crate::native::ref_tasks::RefTaskIndex::build(
                        &config.bob_dir,
                        &config.ref_dir,
                    ),
                    invocation_date: "2026-11-17".to_string(),
                },
            )
            .expect("capture evidence"),
            refresh_budget: ScanRefreshBudget::default(),
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
                owners: BTreeMap::new(),
                markdown_paths: BTreeSet::new(),
                source_store_paths: BTreeSet::new(),
                candidates: BTreeMap::new(),
                observations: BTreeMap::new(),
                index_rest_fingerprint: String::new(),
                planned_pdfs: Vec::new(),
            },
            refresh_budget: ScanRefreshBudget::default(),
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
        let mut owners = BTreeMap::new();
        snapshot_pdf_companion_states(
            &config,
            std::slice::from_ref(&pdf),
            &mut states,
            &mut owners,
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
        let mut after_owners = BTreeMap::new();
        snapshot_pdf_companion_states(
            &config,
            std::slice::from_ref(&pdf),
            &mut after,
            &mut after_owners,
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
        let mut owners = BTreeMap::new();
        snapshot_pdf_companion_states(
            &config,
            std::slice::from_ref(&pdf),
            &mut states,
            &mut owners,
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

    fn located_task(
        path: &str,
        line_index: usize,
    ) -> crate::native::ref_tasks::LocatedRefTask {
        crate::native::ref_tasks::LocatedRefTask {
            path: path.to_string(),
            line_index,
            line: format!("- [ ] read {path}"),
            mark: ' ',
            block_id: Some("ref-a".to_string()),
            archived: false,
            residence: Some("sase".to_string()),
            closed_on: None,
            in_capture_target: true,
        }
    }

    fn classify_pair(
        config: &Config,
        pdfs: &[PathBuf],
        old: &ScanInputEvidence,
        new: &ScanInputEvidence,
    ) -> ScanChange {
        let dest_roots = dest_asset_roots(config, pdfs);
        classify_scan_change(old, new, config, &dest_roots, &[], false)
    }

    fn write_two_pdfs(bob_dir: &Path) -> (Config, PathBuf, PathBuf) {
        let config = test_config(bob_dir);
        let pdf_a = bob_dir.join("lib/papers/a.pdf");
        let pdf_b = bob_dir.join("lib/papers/b.pdf");
        fs::create_dir_all(pdf_a.parent().expect("parent")).expect("mkdir");
        fs::write(&pdf_a, b"%PDF-1.4 a").expect("pdf a");
        fs::write(&pdf_b, b"%PDF-1.4 b").expect("pdf b");
        fs::create_dir_all(bob_dir.join("ref/papers")).expect("ref");
        fs::write(bob_dir.join("ref/papers/a.md"), "# A\n").expect("note a");
        fs::write(bob_dir.join("ref/papers/b.md"), "# B\n").expect("note b");
        fs::write(pdf_a.with_extension("md"), "sidecar a\n")
            .expect("sidecar a");
        fs::write(pdf_b.with_extension("md"), "sidecar b\n")
            .expect("sidecar b");
        (config, pdf_a, pdf_b)
    }

    #[test]
    fn companion_and_shared_target_owner_sets() {
        let temp = tempfile::tempdir().expect("tempdir");
        let bob_dir = temp.path();
        let (config, pdf_a, pdf_b) = write_two_pdfs(bob_dir);
        let dest = bob_dir.join("sase.md");
        fs::write(&dest, "# SASE\n").expect("shared dest");
        let note_a = bob_dir.join("ref/papers/a.md");
        let note_b = bob_dir.join("ref/papers/b.md");
        let mut plan_a = minimal_plan(&pdf_a, &note_a);
        let mut plan_b = minimal_plan(&pdf_b, &note_b);
        plan_a
            .routed_intents
            .push(routed_intent(dest.clone(), pdf_a.clone()));
        plan_b
            .routed_intents
            .push(routed_intent(dest.clone(), pdf_b.clone()));
        let ctx = ScanContext {
            index: crate::native::ref_tasks::RefTaskIndex::build(
                &config.bob_dir,
                &config.ref_dir,
            ),
            invocation_date: "2026-11-17".to_string(),
        };
        let evidence = capture_scan_evidence(
            &config,
            &[pdf_a.clone(), pdf_b.clone()],
            &[],
            &[
                ScanPlanOutcome::Planned(Box::new(plan_a)),
                ScanPlanOutcome::Planned(Box::new(plan_b)),
            ],
            &ctx,
        )
        .expect("capture");
        let sidecar_a = pdf_a.with_extension("md");
        assert_eq!(
            evidence.owners.get(&sidecar_a),
            Some(&BTreeSet::from([pdf_a.clone()])),
            "each sidecar is owned only by its PDF"
        );
        assert_eq!(
            evidence.owners.get(&dest),
            Some(&BTreeSet::from([pdf_a.clone(), pdf_b.clone()])),
            "shared targets keep every owner"
        );
        assert!(
            !evidence
                .owners
                .get(&bob_dir.join("ref/papers/a.md"))
                .is_some_and(|owners| owners.contains(&pdf_b)),
            "PDF B does not own PDF A's ref note"
        );
    }

    #[test]
    fn classifier_selects_own_pdf_and_source_changes() {
        let temp = tempfile::tempdir().expect("tempdir");
        let bob_dir = temp.path();
        let (config, pdf_a, pdf_b) = write_two_pdfs(bob_dir);
        let old = capture_pre_scan_evidence(
            &config,
            &[pdf_a.clone(), pdf_b.clone()],
            &[],
        )
        .expect("pre");
        let mut new = old.clone();
        let sidecar_a = pdf_a.with_extension("md");
        new.file_states.insert(
            sidecar_a.clone(),
            EvidenceFileState {
                exists: true,
                sha256: Some("sidecar-changed".to_string()),
            },
        );
        match classify_pair(
            &config,
            &[pdf_a.clone(), pdf_b.clone()],
            &old,
            &new,
        ) {
            ScanChange::Refresh { pdfs, .. } => {
                assert_eq!(pdfs, BTreeSet::from([pdf_a.clone()]));
            }
            other => panic!("expected refresh of PDF A, got {other:?}"),
        }
        let mut new = old.clone();
        new.pdf_hashes
            .insert(pdf_b.clone(), "pdf-b-changed".to_string());
        match classify_pair(
            &config,
            &[pdf_a.clone(), pdf_b.clone()],
            &old,
            &new,
        ) {
            ScanChange::Refresh { pdfs, .. } => {
                assert_eq!(pdfs, BTreeSet::from([pdf_b]));
            }
            other => panic!("expected refresh of PDF B, got {other:?}"),
        }
    }

    #[test]
    fn classifier_selects_unowned_and_root_markdown_consumers() {
        let temp = tempfile::tempdir().expect("tempdir");
        let bob_dir = temp.path();
        let (config, pdf_a, pdf_b) = write_two_pdfs(bob_dir);
        let daily = bob_dir.join("2026/day.md");
        fs::create_dir_all(daily.parent().expect("parent")).expect("day dir");
        fs::write(&daily, "# day\n").expect("daily");
        let root = bob_dir.join("sase.md");
        fs::write(&root, "# SASE\n").expect("root");
        let mut old = capture_pre_scan_evidence(
            &config,
            &[pdf_a.clone(), pdf_b.clone()],
            &[],
        )
        .expect("pre");
        old.markdown_paths.insert(daily.clone());
        old.markdown_paths.insert(root.clone());
        old.file_states.insert(
            daily.clone(),
            EvidenceFileState {
                exists: true,
                sha256: Some("daily-old".to_string()),
            },
        );
        old.file_states.insert(
            root.clone(),
            EvidenceFileState {
                exists: true,
                sha256: Some("root-old".to_string()),
            },
        );
        old.observations.insert(
            pdf_a.clone(),
            PlanDependencyObservations {
                consulted_vault_links: true,
                ..PlanDependencyObservations::default()
            },
        );
        old.observations.insert(
            pdf_b.clone(),
            PlanDependencyObservations {
                consulted_parent_targets: true,
                ..PlanDependencyObservations::default()
            },
        );
        let mut unowned = old.clone();
        unowned.file_states.insert(
            daily.clone(),
            EvidenceFileState {
                exists: true,
                sha256: Some("daily-new".to_string()),
            },
        );
        match classify_pair(
            &config,
            &[pdf_a.clone(), pdf_b.clone()],
            &old,
            &unowned,
        ) {
            ScanChange::Refresh { pdfs, .. } => {
                assert_eq!(pdfs, BTreeSet::from([pdf_a.clone()]));
            }
            other => panic!("expected vault-link consumer, got {other:?}"),
        }
        let mut root_change = old.clone();
        root_change.file_states.insert(
            root,
            EvidenceFileState {
                exists: true,
                sha256: Some("root-new".to_string()),
            },
        );
        match classify_pair(
            &config,
            &[pdf_a.clone(), pdf_b.clone()],
            &old,
            &root_change,
        ) {
            ScanChange::Refresh { pdfs, .. } => {
                assert_eq!(pdfs, BTreeSet::from([pdf_a, pdf_b]));
            }
            other => {
                panic!("expected parent plus link consumers, got {other:?}")
            }
        }
    }

    #[test]
    fn classifier_unions_owned_and_shared_dependencies() {
        let temp = tempfile::tempdir().expect("tempdir");
        let bob_dir = temp.path();
        let (config, pdf_a, pdf_b) = write_two_pdfs(bob_dir);
        let shared = bob_dir.join("area.md");
        fs::write(&shared, "# area\n").expect("area");
        let mut old = capture_pre_scan_evidence(
            &config,
            &[pdf_a.clone(), pdf_b.clone()],
            &[],
        )
        .expect("pre");
        old.markdown_paths.insert(shared.clone());
        old.file_states.insert(
            shared.clone(),
            EvidenceFileState {
                exists: true,
                sha256: Some("area-old".to_string()),
            },
        );
        old.owners
            .entry(shared.clone())
            .or_default()
            .insert(pdf_a.clone());
        old.observations.insert(
            pdf_b.clone(),
            PlanDependencyObservations {
                consulted_vault_links: true,
                ..PlanDependencyObservations::default()
            },
        );
        let mut new = old.clone();
        new.file_states.insert(
            shared,
            EvidenceFileState {
                exists: true,
                sha256: Some("area-new".to_string()),
            },
        );
        match classify_pair(
            &config,
            &[pdf_a.clone(), pdf_b.clone()],
            &old,
            &new,
        ) {
            ScanChange::Refresh { pdfs, .. } => {
                assert_eq!(pdfs, BTreeSet::from([pdf_a, pdf_b]));
            }
            other => panic!("expected owned-plus-shared union, got {other:?}"),
        }
    }

    #[test]
    fn classifier_selects_per_pdf_locator_line_shifts() {
        let temp = tempfile::tempdir().expect("tempdir");
        let bob_dir = temp.path();
        let (config, pdf_a, pdf_b) = write_two_pdfs(bob_dir);
        let mut old = capture_pre_scan_evidence(
            &config,
            &[pdf_a.clone(), pdf_b.clone()],
            &[],
        )
        .expect("pre");
        old.candidates
            .insert(pdf_a.clone(), vec![located_task("sase.md", 3)]);
        old.candidates
            .insert(pdf_b.clone(), vec![located_task("inbox.md", 1)]);
        let mut shifted = old.clone();
        shifted
            .candidates
            .insert(pdf_a.clone(), vec![located_task("sase.md", 8)]);
        match classify_pair(
            &config,
            &[pdf_a.clone(), pdf_b.clone()],
            &old,
            &shifted,
        ) {
            ScanChange::Refresh { pdfs, .. } => {
                assert_eq!(pdfs, BTreeSet::from([pdf_a.clone()]));
            }
            other => {
                panic!("expected candidate line shift of A, got {other:?}")
            }
        }
        let mut rest = old.clone();
        rest.index_rest_fingerprint = "follow-up-changed".to_string();
        match classify_pair(&config, &[pdf_a, pdf_b], &old, &rest) {
            ScanChange::Refresh { pdfs, reason } => {
                assert!(
                    pdfs.is_empty(),
                    "unrelated index data is finalization-only"
                );
                assert_eq!(reason, "reading-task locator changed");
            }
            other => panic!("expected empty refresh, got {other:?}"),
        }
    }

    #[test]
    fn classifier_rebuilds_on_global_membership_config_ref_and_source() {
        let temp = tempfile::tempdir().expect("tempdir");
        let bob_dir = temp.path();
        let (config, pdf_a, pdf_b) = write_two_pdfs(bob_dir);
        let pdfs = [pdf_a.clone(), pdf_b.clone()];
        let old = capture_pre_scan_evidence(&config, &pdfs, &[]).expect("pre");

        let mut membership = old.clone();
        membership.lib_pdfs.push(bob_dir.join("lib/papers/c.pdf"));
        assert!(matches!(
            classify_pair(&config, &pdfs, &old, &membership),
            ScanChange::Rebuild { .. }
        ));

        let mut config_change = old.clone();
        let config_path = super::bob_config::config_path();
        config_change.file_states.insert(
            config_path,
            EvidenceFileState {
                exists: true,
                sha256: Some("config-new".to_string()),
            },
        );
        assert!(matches!(
            classify_pair(&config, &pdfs, &old, &config_change),
            ScanChange::Rebuild { .. }
        ));

        let mut ref_note = old.clone();
        let note_a = bob_dir.join("ref/papers/a.md");
        ref_note.file_states.insert(
            note_a,
            EvidenceFileState {
                exists: true,
                sha256: Some("ref-changed".to_string()),
            },
        );
        assert!(matches!(
            classify_pair(&config, &pdfs, &old, &ref_note),
            ScanChange::Rebuild { .. }
        ));

        let mut source = old.clone();
        let store = bob_dir.join(".bob/ref-sources/web/a--abc/source.md");
        source.source_store_paths.insert(store.clone());
        source.file_states.insert(
            store,
            EvidenceFileState {
                exists: true,
                sha256: Some("source".to_string()),
            },
        );
        assert!(matches!(
            classify_pair(&config, &pdfs, &old, &source),
            ScanChange::Rebuild { .. }
        ));
    }

    #[test]
    fn classifier_dest_asset_precedes_other_mismatches() {
        let temp = tempfile::tempdir().expect("tempdir");
        let bob_dir = temp.path();
        let (config, pdf_a, pdf_b) = write_two_pdfs(bob_dir);
        let dest_dir = bob_dir.join("ref/papers/a.assets");
        fs::create_dir_all(&dest_dir).expect("assets");
        let dest = dest_dir.join("fig.png");
        fs::write(&dest, b"dest-v1").expect("dest");
        let old = capture_pre_scan_evidence(
            &config,
            &[pdf_a.clone(), pdf_b.clone()],
            &[],
        )
        .expect("pre");
        let mut new = old.clone();
        new.file_states.insert(
            dest.clone(),
            EvidenceFileState {
                exists: true,
                sha256: Some("dest-vandalized".to_string()),
            },
        );
        new.lib_pdfs.push(bob_dir.join("lib/papers/c.pdf"));
        new.file_states.insert(
            pdf_a.with_extension("md"),
            EvidenceFileState {
                exists: true,
                sha256: Some("sidecar-too".to_string()),
            },
        );
        match classify_pair(&config, &[pdf_a, pdf_b], &old, &new) {
            ScanChange::DestinationAsset { reason } => {
                assert!(
                    reason.contains("destination asset changed"),
                    "dest-asset must win over global/owned churn: {reason}"
                );
                assert!(reason.contains(&dest.display().to_string()));
            }
            other => panic!("expected dest-asset failure, got {other:?}"),
        }
    }

    #[test]
    fn markdown_snapshot_includes_vault_links_excluded_dirs() {
        let temp = tempfile::tempdir().expect("tempdir");
        let bob_dir = temp.path();
        let config = test_config(bob_dir);
        let excluded = bob_dir.join("_templates/snippet.md");
        fs::create_dir_all(excluded.parent().expect("parent")).expect("mkdir");
        fs::write(&excluded, "# template\n").expect("template");
        let states = snapshot_vault_md_states(&config).expect("snapshot");
        assert!(
            states.get(&excluded).is_some_and(|state| state.exists),
            "notes in directories vault_links reads and the dependency walker excludes must still be snapshotted"
        );
        let (dep_paths, warnings) =
            crate::native::capture_dependency_tasks::vault_note_paths(bob_dir);
        assert!(warnings.is_empty());
        assert!(
            !dep_paths.iter().any(|path| path.ends_with("snippet.md")),
            "the dependency walker must still exclude _templates"
        );
    }

    #[test]
    fn empty_link_set_does_not_consult_vault_links() {
        let temp = tempfile::tempdir().expect("tempdir");
        let bob_dir = temp.path();
        let config = test_config(bob_dir);
        fs::create_dir_all(bob_dir.join("lib/papers")).expect("lib");
        fs::create_dir_all(bob_dir.join("ref/papers")).expect("ref");
        let pdf = bob_dir.join("lib/papers/example.pdf");
        let note = bob_dir.join("ref/papers/example.md");
        fs::write(&pdf, b"%PDF-1.4").expect("pdf");
        fs::write(&note, "# Example\n").expect("note");
        let sidecar = SidecarInput {
            path: pdf.with_extension("md"),
            annotations: parse_sidecar_markdown(
                "## Page 1\n\n> quoted text\ncomment\n",
            ),
        };
        let rendered = render_sidecar_highlights(
            &config,
            &pdf,
            &note,
            &ParsedNote::empty(),
            &sidecar,
            false,
        )
        .expect("render");
        assert!(
            !rendered.consulted_vault_links,
            "an empty ID set must not mark the plan as a vault-link consumer"
        );
    }

    #[test]
    fn incomplete_failures_select_on_markdown_and_index_churn() {
        let temp = tempfile::tempdir().expect("tempdir");
        let bob_dir = temp.path();
        let (config, pdf_a, pdf_b) = write_two_pdfs(bob_dir);
        let daily = bob_dir.join("2026/day.md");
        fs::create_dir_all(daily.parent().expect("parent")).expect("day dir");
        fs::write(&daily, "# day\n").expect("daily");
        let mut old = capture_pre_scan_evidence(
            &config,
            &[pdf_a.clone(), pdf_b.clone()],
            &[],
        )
        .expect("pre");
        old.markdown_paths.insert(daily.clone());
        old.file_states.insert(
            daily.clone(),
            EvidenceFileState {
                exists: true,
                sha256: Some("daily-old".to_string()),
            },
        );
        old.observations
            .insert(pdf_a.clone(), PlanDependencyObservations::unknown());
        old.observations
            .insert(pdf_b.clone(), PlanDependencyObservations::default());
        let mut markdown = old.clone();
        markdown.file_states.insert(
            daily,
            EvidenceFileState {
                exists: true,
                sha256: Some("daily-new".to_string()),
            },
        );
        match classify_pair(
            &config,
            &[pdf_a.clone(), pdf_b.clone()],
            &old,
            &markdown,
        ) {
            ScanChange::Refresh { pdfs, .. } => {
                assert_eq!(pdfs, BTreeSet::from([pdf_a.clone()]));
            }
            other => {
                panic!("expected incomplete failure on markdown, got {other:?}")
            }
        }
        let mut rest = old.clone();
        rest.index_rest_fingerprint = "follow-up-changed".to_string();
        match classify_pair(&config, &[pdf_a.clone(), pdf_b], &old, &rest) {
            ScanChange::Refresh { pdfs, .. } => {
                assert_eq!(pdfs, BTreeSet::from([pdf_a]));
            }
            other => {
                panic!(
                    "expected incomplete failure on index rest, got {other:?}"
                )
            }
        }
    }

    #[test]
    fn live_union_preserves_prior_and_new_dependencies() {
        let pdf_a = PathBuf::from("/tmp/a.pdf");
        let pdf_b = PathBuf::from("/tmp/b.pdf");
        let shared = PathBuf::from("/tmp/shared.md");
        let mut old = ScanInputEvidence {
            pdf_hashes: BTreeMap::new(),
            file_states: BTreeMap::new(),
            lib_pdfs: Vec::new(),
            xlib_members: Vec::new(),
            intake_destinations: Vec::new(),
            owners: BTreeMap::from([(
                shared.clone(),
                BTreeSet::from([pdf_a.clone()]),
            )]),
            markdown_paths: BTreeSet::new(),
            source_store_paths: BTreeSet::new(),
            candidates: BTreeMap::new(),
            observations: BTreeMap::from([(
                pdf_a.clone(),
                PlanDependencyObservations {
                    consulted_vault_links: false,
                    consulted_parent_targets: false,
                    consulted_lineage: false,
                    complete: false,
                },
            )]),
            index_rest_fingerprint: String::new(),
            planned_pdfs: Vec::new(),
        };
        let live = ScanInputEvidence {
            pdf_hashes: BTreeMap::new(),
            file_states: BTreeMap::new(),
            lib_pdfs: Vec::new(),
            xlib_members: Vec::new(),
            intake_destinations: Vec::new(),
            owners: BTreeMap::from([(
                shared.clone(),
                BTreeSet::from([pdf_b.clone()]),
            )]),
            markdown_paths: BTreeSet::new(),
            source_store_paths: BTreeSet::new(),
            candidates: BTreeMap::new(),
            observations: BTreeMap::from([(
                pdf_a.clone(),
                PlanDependencyObservations {
                    consulted_vault_links: true,
                    consulted_parent_targets: true,
                    consulted_lineage: true,
                    complete: true,
                },
            )]),
            index_rest_fingerprint: String::new(),
            planned_pdfs: Vec::new(),
        };
        merge_live_dependencies_into_comparison_old(&mut old, &live);
        let owners = old.owners.get(&shared).expect("shared owners");
        assert!(owners.contains(&pdf_a));
        assert!(owners.contains(&pdf_b));
        let obs = old.observations.get(&pdf_a).expect("obs");
        assert!(obs.consulted_vault_links);
        assert!(obs.consulted_parent_targets);
        assert!(obs.consulted_lineage);
        assert!(
            !obs.complete,
            "prior incomplete stays conservative over a complete replacement"
        );
    }

    #[test]
    fn live_vault_link_acquisition_selects_unowned_markdown() {
        // A sidecar edit that makes a plan start consulting vault links must
        // select an unowned Markdown edit arriving during its refresh. Old
        // evidence has no vault-link consumer; live evidence does; the union
        // drives the pre/post comparison.
        let temp = tempfile::tempdir().expect("tempdir");
        let bob_dir = temp.path();
        let (config, pdf_a, pdf_b) = write_two_pdfs(bob_dir);
        let daily = bob_dir.join("2026/day.md");
        fs::create_dir_all(daily.parent().expect("parent")).expect("day dir");
        fs::write(&daily, "# day\n").expect("daily");
        let mut old = capture_pre_scan_evidence(
            &config,
            &[pdf_a.clone(), pdf_b.clone()],
            &[],
        )
        .expect("pre");
        old.markdown_paths.insert(daily.clone());
        old.file_states.insert(
            daily.clone(),
            EvidenceFileState {
                exists: true,
                sha256: Some("daily-old".to_string()),
            },
        );
        old.observations
            .insert(pdf_a.clone(), PlanDependencyObservations::default());
        old.observations
            .insert(pdf_b.clone(), PlanDependencyObservations::default());
        let mut live = old.clone();
        live.observations.insert(
            pdf_a.clone(),
            PlanDependencyObservations {
                consulted_vault_links: true,
                ..PlanDependencyObservations::default()
            },
        );
        let mut comparison_old = old.clone();
        merge_live_dependencies_into_comparison_old(&mut comparison_old, &live);
        let mut new = comparison_old.clone();
        new.file_states.insert(
            daily.clone(),
            EvidenceFileState {
                exists: true,
                sha256: Some("daily-new".to_string()),
            },
        );
        match classify_pair(
            &config,
            &[pdf_a.clone(), pdf_b],
            &comparison_old,
            &new,
        ) {
            ScanChange::Refresh { pdfs, .. } => {
                assert_eq!(pdfs, BTreeSet::from([pdf_a]));
            }
            other => {
                panic!("live vault-link must select its PDF, got {other:?}")
            }
        }
    }

    #[test]
    fn clean_baseline_drops_obsolete_owners() {
        // After convergence only live owners remain: editing a former target
        // must not replan the PDF.
        let temp = tempfile::tempdir().expect("tempdir");
        let bob_dir = temp.path();
        let (config, pdf_a, pdf_b) = write_two_pdfs(bob_dir);
        let former = bob_dir.join("former.md");
        let current = bob_dir.join("current.md");
        fs::write(&former, "# former\n").expect("former");
        fs::write(&current, "# current\n").expect("current");
        let mut clean = capture_pre_scan_evidence(
            &config,
            &[pdf_a.clone(), pdf_b.clone()],
            &[],
        )
        .expect("pre");
        for path in [&former.clone(), &current.clone()] {
            clean.markdown_paths.insert(path.clone());
            clean.file_states.insert(
                path.clone(),
                EvidenceFileState {
                    exists: true,
                    sha256: Some("v1".to_string()),
                },
            );
        }
        clean
            .owners
            .entry(current.clone())
            .or_default()
            .insert(pdf_a.clone());
        clean
            .observations
            .insert(pdf_a.clone(), PlanDependencyObservations::default());
        clean
            .observations
            .insert(pdf_b.clone(), PlanDependencyObservations::default());
        let mut edited_former = clean.clone();
        edited_former.file_states.insert(
            former.clone(),
            EvidenceFileState {
                exists: true,
                sha256: Some("v2".to_string()),
            },
        );
        match classify_pair(
            &config,
            &[pdf_a.clone(), pdf_b.clone()],
            &clean,
            &edited_former,
        ) {
            ScanChange::Refresh { pdfs, .. } => {
                assert!(
                    pdfs.is_empty(),
                    "former target must not replan after ownership replacement"
                );
            }
            other => panic!("expected finalization-only, got {other:?}"),
        }
        let mut edited_current = clean.clone();
        edited_current.file_states.insert(
            current.clone(),
            EvidenceFileState {
                exists: true,
                sha256: Some("v2".to_string()),
            },
        );
        match classify_pair(
            &config,
            &[pdf_a.clone(), pdf_b.clone()],
            &clean,
            &edited_current,
        ) {
            ScanChange::Refresh { pdfs, .. } => {
                assert_eq!(pdfs, BTreeSet::from([pdf_a]));
            }
            other => panic!("current target must replan, got {other:?}"),
        }
    }

    #[test]
    fn typed_changes_carry_stale_reasons() {
        let refresh = ScanChange::Refresh {
            pdfs: BTreeSet::new(),
            reason: "input changed: day.md".to_string(),
        };
        assert!(matches!(refresh, ScanChange::Refresh { .. }));
        let rebuild = ScanChange::Rebuild {
            reason: "library membership changed".to_string(),
        };
        assert!(matches!(rebuild, ScanChange::Rebuild { .. }));
        let dest = ScanChange::DestinationAsset {
            reason: "destination asset changed: fig.png".to_string(),
        };
        match dest {
            ScanChange::DestinationAsset { reason } => {
                assert!(reason.contains("destination asset changed"));
                let error = destination_asset_changed_error(&reason);
                assert_eq!(error.code, Some("scan_failed"));
            }
            other => panic!("expected dest-asset change, got {other:?}"),
        }
    }
}
