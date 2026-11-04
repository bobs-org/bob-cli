//! Note-pull planner for lone `@route` captures
//! (`docs/task-dependencies.md` §12.20).
//!
//! A lone task-list header offers its note's eligible open residents
//! through the Pull-In protocol. Rows have trigger `note_pull`, no
//! predecessor, and normal typed placement without an anchor.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::{Path, PathBuf};

use chrono::NaiveDate;

use crate::native::capture_dependency_tasks::{
    has_hide_tag, has_prj_is_blocked_tag,
};
use crate::native::capture_task_toggle::set_task_line_status;
use crate::native::freshness::placement::stamp_fresh;
use crate::native::note_tasks::{clean_description, stable_task_key};
use crate::native::task_complete::{
    is_inbox_note, live_link_keys, OfferGroup, SuccessorSelection,
    NOTE_PULL_TRIGGER,
};
use crate::native::task_status_hooks::{self, FileScan};

use super::batch::CaptureBatchPlanner;
use super::dependencies::DependencyContext;
use super::output::{
    CaptureError, CaptureItemResult, NotePullSummaryJson, Placement,
    PullInFoldedJson, TaskCompleteOfferDestinationJson, TaskCompleteOfferJson,
    TaskCompleteSuccessorLinkJson, TaskCompleteUnblockedJson,
};
use super::plan::{capture_kind_label, PlannedCaptureItem};
use super::task_theme::CaptureTaskThemeContext;
use super::{CaptureRequest, CaptureWarning};

/// Plan one lone-`@route` item: classify residents, compute keys, place,
/// apply explicit picks, and report the additive `note_pull` payload.
///
/// `item_index` is the draft position (0-based) used in the offer order
/// key so draft item order precedes lane, path, and line within the
/// note-pull rank. `earlier_keys` holds every offer key already owned by
/// an earlier draft item across all successor classes; `earlier_block_ids`
/// holds their final `(note_path, block_id)` identities so a selected
/// predecessor that minted an ID still owns its row after minting.
#[allow(clippy::too_many_arguments)]
pub(super) fn plan_note_pull_item(
    request: &CaptureRequest,
    parsed: crate::native::capture_language::ParsedCaptureText,
    today: NaiveDate,
    planner: &mut CaptureBatchPlanner,
    dependency_ctx: &mut DependencyContext,
    task_theme_context: &mut CaptureTaskThemeContext,
    warnings: &mut Vec<CaptureWarning>,
    item_index: usize,
    earlier_keys: &BTreeSet<String>,
    earlier_block_ids: &BTreeSet<(PathBuf, String)>,
) -> Result<PlannedCaptureItem, CaptureError> {
    let route = match &parsed.kind {
        crate::native::capture_language::CaptureKind::NotePull { route } => {
            route.clone()
        }
        _ => {
            return Err(CaptureError::io(
                "note-pull planner invariant failed: wrong capture kind",
            ));
        }
    };
    let note = format!("{route}.md");
    let relative = PathBuf::from(&note);
    let absolute = request.bob_dir.join(&relative);

    let Some(source_text) = planner.current_contents(&absolute)? else {
        return Err(pull_note_missing_error(&route, &note));
    };
    // Draft-start preimage for same-draft creation provenance: rows absent
    // here were created by an earlier item and must not stamp fresh.
    // `current_contents` ensures the file is loaded, so `loaded_texts`
    // yields the pre-draft original alongside the staged current text.
    let original_source_text = planner
        .loaded_texts(&absolute)
        .map(|(original, _)| original);

    let day_file = crate::native::pomodoro::day_file_for(&request.bob_dir);
    if !planner.currently_exists(&day_file)? {
        return Err(CaptureError::io(format!(
            "Bob daily note does not exist: {}",
            day_file.display()
        )));
    }
    let day_text = planner.read_existing(&day_file)?;
    let day_relative = crate::native::capture_pomodoros::relative_day_file(
        &day_file,
        &request.bob_dir,
    );

    let tasks_settings =
        crate::native::task_status_hooks::read_tasks_settings(&request.bob_dir);
    let note_settings =
        crate::native::note_tasks::read_settings(&request.bob_dir);
    let tasks = task_status_hooks::parse_tasks(&source_text, &tasks_settings);

    // Most pulls have no existing Task Links in today's Pomodoros, and
    // ordinary area notes have no inbox-parent metadata. Avoid the
    // vault-wide path catalog in that common case; build it only when a
    // day link needs resolution or an area may be an inbox child.
    let needs_live_link_index = day_text.contains("#^");
    let inbox_without_index =
        crate::native::task_complete::successors::inbox_note_index_free_result(
            &relative,
            &source_text,
        );
    let index =
        (needs_live_link_index || inbox_without_index.is_none()).then(|| {
            super::task_complete::staged_note_index(
                &request.bob_dir,
                planner,
                dependency_ctx,
            )
        });
    let live = match (needs_live_link_index, index.as_ref()) {
        (true, Some(index)) => {
            live_link_keys(&day_text, Path::new(&day_relative), index)
        }
        _ => BTreeSet::new(),
    };
    let inbox = match inbox_without_index {
        Some(inbox) => inbox,
        None => is_inbox_note(
            &relative,
            &source_text,
            index.as_ref().expect("possible inbox child builds index"),
        ),
    };

    // Prerequisite proof follows the same first-match rules as
    // classification below: known blocked symbols and project tags
    // short-circuit without proof, while ignored/closed/custom rows never
    // trigger it. Every remaining dependency-bearing row needs proof even
    // when a later scheduled, hidden, or already-planned filter would
    // exclude it; otherwise a hidden row with a closed prerequisite would
    // fold as blocked when the snapshot is skipped.
    let needs_snapshot = tasks.iter().any(|task| {
        if task.depends_on.is_empty()
            || task.block_id.as_deref() == Some("prj")
            || !task.status_recognized
            || !matches!(task.status, ' ' | '*' | '/' | '?')
        {
            return false;
        }
        let raw = crate::native::task_complete::successors::raw_task_line(
            &source_text,
            task.line_index,
        );
        if task.status == '?' || has_prj_is_blocked_tag(&raw) {
            return false;
        }
        true
    });
    let snapshot_state: Option<SnapshotProof> = if needs_snapshot {
        let mut needed_ids = BTreeSet::new();
        for task in &tasks {
            if task.depends_on.is_empty()
                || task.block_id.as_deref() == Some("prj")
                || !task.status_recognized
                || !matches!(task.status, ' ' | '*' | '/' | '?')
            {
                continue;
            }
            let raw = crate::native::task_complete::successors::raw_task_line(
                &source_text,
                task.line_index,
            );
            if task.status == '?' || has_prj_is_blocked_tag(&raw) {
                continue;
            }
            needed_ids.extend(task.depends_on.iter().cloned());
        }
        if let Some(prj) = tasks
            .iter()
            .find(|task| task.block_id.as_deref() == Some("prj"))
        {
            needed_ids.extend(prj.depends_on.iter().cloned());
        }
        Some(prepare_snapshot_proof(
            dependency_ctx,
            planner,
            &request.bob_dir,
            &tasks_settings,
            &needed_ids,
            &relative,
        ))
    } else {
        None
    };

    let mut folded_already = 0usize;
    let mut folded_blocked = 0usize;
    let mut folded_scheduled = 0usize;
    let mut folded_hidden = 0usize;
    let mut offers: Vec<NotePullOffer> = Vec::new();

    for task in &tasks {
        let raw = crate::native::task_complete::successors::raw_task_line(
            &source_text,
            task.line_index,
        );
        if task.block_id.as_deref() == Some("prj") {
            continue;
        }
        if !matches!(task.status, ' ' | '*' | '/' | '?') {
            continue;
        }
        if !task.status_recognized {
            continue;
        }
        if task.status == '?' || has_prj_is_blocked_tag(&raw) {
            folded_blocked += 1;
            continue;
        }
        if !task.depends_on.is_empty() {
            let blocked = match snapshot_state.as_ref() {
                None => true,
                Some(proof) => proof.is_blocked(&relative, task),
            };
            if blocked {
                folded_blocked += 1;
                continue;
            }
        }
        if task.scheduled.is_some_and(|date| date > today) {
            folded_scheduled += 1;
            continue;
        }
        if has_hide_tag(&raw) {
            folded_hidden += 1;
            continue;
        }
        if let Some(block_id) = task.block_id.as_deref()
            && live.contains(&(relative.clone(), block_id.to_string()))
        {
            folded_already += 1;
            continue;
        }
        if !matches!(task.status, ' ' | '*' | '/') {
            continue;
        }
        offers.push(NotePullOffer {
            line_index: task.line_index,
            status: task.status,
            block_id: task.block_id.clone(),
            depends_on_empty: task.depends_on.is_empty(),
            text: clean_description(
                &task.description,
                &note_settings.global_filter,
                task.block_id.as_deref(),
            ),
            raw_status_line: raw.to_string(),
        });
    }

    if offers.is_empty() {
        // Selected repeated-pull ownership: a genuinely empty source
        // refuses, but a later pull whose every otherwise-offerable row
        // belongs to earlier offers prunes instead of retroactively
        // refusing the valid batch. Only already-planned folds can carry
        // ownership (they need a block id for the live link); blocked,
        // scheduled, and hidden folds never do. An empty made solely by
        // an earlier explicit link that was not an offer keeps its
        // refusal because that link owns no offer key.
        if folded_already > 0 {
            let mut owned = 0usize;
            let mut total_already = 0usize;
            for task in &tasks {
                if task.block_id.as_deref() == Some("prj") {
                    continue;
                }
                if !matches!(task.status, ' ' | '*' | '/' | '?') {
                    continue;
                }
                if !task.status_recognized {
                    continue;
                }
                let raw =
                    crate::native::task_complete::successors::raw_task_line(
                        &source_text,
                        task.line_index,
                    );
                if task.status == '?' || has_prj_is_blocked_tag(&raw) {
                    continue;
                }
                if !task.depends_on.is_empty() {
                    let blocked = match snapshot_state.as_ref() {
                        None => true,
                        Some(proof) => proof.is_blocked(&relative, task),
                    };
                    if blocked {
                        continue;
                    }
                }
                if task.scheduled.is_some_and(|date| date > today) {
                    continue;
                }
                if has_hide_tag(&raw) {
                    continue;
                }
                let Some(block_id) = task.block_id.as_deref() else {
                    continue;
                };
                if !live.contains(&(relative.clone(), block_id.to_string())) {
                    continue;
                }
                total_already += 1;
                let key = stable_task_key(
                    &relative,
                    Some(block_id),
                    &clean_description(
                        &task.description,
                        &note_settings.global_filter,
                        Some(block_id),
                    ),
                    &note_settings.global_filter,
                    1,
                );
                if earlier_keys.contains(&key)
                    || earlier_block_ids
                        .contains(&(relative.clone(), block_id.to_string()))
                {
                    owned += 1;
                }
            }
            if total_already > 0 && owned == total_already {
                let folded = PullInFoldedJson {
                    hidden: folded_hidden,
                    already_planned: folded_already,
                    sub_projects: 0,
                    blocked: folded_blocked,
                    scheduled: folded_scheduled,
                    already_started: 0,
                    waits_for: 0,
                };
                let kind_label = capture_kind_label(&parsed.kind);
                return Ok(PlannedCaptureItem {
                    result: CaptureItemResult {
                        ok: true,
                        dry_run: request.dry_run,
                        routed: true,
                        route: Some(route.clone()),
                        route_label: route.clone(),
                        relative_target: note.clone(),
                        target: absolute.display().to_string(),
                        text: String::new(),
                        task_line: String::new(),
                        task_theme: None,
                        type_relocation: None,
                        kind: kind_label,
                        fanout: None,
                        created: super::plan::date_string(today),
                        scheduled: None,
                        priority: None,
                        priority_label: None,
                        placement: Placement::Created,
                        sub_bullets: Vec::new(),
                        notes: Vec::new(),
                        drawers: Vec::new(),
                        clip: None,
                        schedule_log: None,
                        block_id: None,
                        day_file: None,
                        block_link: None,
                        pomodoro_link_placement: None,
                        parent_line: None,
                        parent_text: None,
                        drawer: None,
                        parent_status_symbol: None,
                        parent_status_name: None,
                        toggle_direction: None,
                        previous_task_line: None,
                        status_symbol: None,
                        status_name: None,
                        previous_status_symbol: None,
                        previous_status_name: None,
                        pomodoro_name: None,
                        creates_pomodoro: None,
                        pomodoro_already_linked: None,
                        removed_pomodoro_links: None,
                        removed_scheduled: None,
                        pomodoro_selector_unused: None,
                        toggle_behavior: None,
                        status_changed: None,
                        pomodoro_link_action: None,
                        pomodoro_link_source: None,
                        pomodoro_link_destination: None,
                        project_note: None,
                        pomodoro_start: None,
                        pomodoro_adjust: None,
                        pomodoro_shift: None,
                        pomodoro_split: None,
                        pomodoro_merge: None,
                        pomodoro_close: None,
                        pomodoro_reset: None,
                        dependency_update: None,
                        consumed_links: Vec::new(),
                        task_complete: None,
                        r#ref: None,
                        toggle_task_description: None,
                        link_guard: None,
                        dropped_dependencies: Vec::new(),
                        blocks_line_updates: Vec::new(),
                        minted_dependent_block_id: None,
                        dependency_updates: Vec::new(),
                        minted_blocker_block_id: None,
                        relation_updates: Vec::new(),
                        relation_cancels: Vec::new(),
                        relation_completes: Vec::new(),
                        relation_merge_output: Default::default(),
                        linked_from_line_updates: Vec::new(),
                        minted_relation_block_ids: Vec::new(),
                        mirror_skipped: Vec::new(),
                        list: None,
                        relation_successors: Vec::new(),
                        relation_held: Vec::new(),
                        relation_check: None,
                        relation_groups: Vec::new(),
                        relation_label_hides: Vec::new(),
                        note_pull: Some(NotePullSummaryJson {
                            note: note.clone(),
                            route: route.clone(),
                            successors: Vec::new(),
                            folded,
                        }),
                    },
                    clip_plan: None,
                    pomodoro_refs: Vec::new(),
                    task_block_refs: Vec::new(),
                    project_move_landings: Vec::new(),
                });
            }
        }
        // Lazy project proof for the refusal reason: classification needed
        // no snapshot, but a `^prj` with prerequisites still needs proven
        // open counts. Build the narrow proof here so the refusal names
        // only truly open prerequisites.
        let lazy_proof;
        let proof_for_reason = if let Some(proof) = snapshot_state.as_ref() {
            Some(proof)
        } else if tasks.iter().any(|task| {
            task.block_id.as_deref() == Some("prj")
                && !task.depends_on.is_empty()
        }) && blocked_project_reason(&tasks, today, &relative, None)
            .is_some()
        {
            let mut needed_ids = BTreeSet::new();
            if let Some(prj) = tasks
                .iter()
                .find(|task| task.block_id.as_deref() == Some("prj"))
            {
                needed_ids.extend(prj.depends_on.iter().cloned());
            }
            lazy_proof = prepare_snapshot_proof(
                dependency_ctx,
                planner,
                &request.bob_dir,
                &tasks_settings,
                &needed_ids,
                &relative,
            );
            Some(&lazy_proof)
        } else {
            None
        };
        return Err(nothing_to_pull_in_error(
            &tasks,
            &note,
            &route,
            today,
            folded_already,
            folded_blocked,
            folded_scheduled,
            folded_hidden,
            &relative,
            proof_for_reason,
        ));
    }

    // Freeze keys from the staged source before any pick.
    // Same-draft creation provenance: compare staged residents against
    // the draft-start preimage with stable occurrence identity (block ID
    // or cleaned description + ordinal). Raw whole-line inequality is
    // insufficient because an earlier item may have edited an old row
    // (mint, status, stamp) without making it new.
    let original_tasks: Vec<task_status_hooks::TaskLine> = original_source_text
        .as_deref()
        .map(|original| {
            task_status_hooks::parse_tasks(original, &tasks_settings)
        })
        .unwrap_or_default();
    let mut original_by_id: BTreeSet<String> = BTreeSet::new();
    let mut original_idless_counts: BTreeMap<String, usize> = BTreeMap::new();
    for task in &original_tasks {
        if task.block_id.as_deref() == Some("prj") {
            continue;
        }
        if let Some(id) = task.block_id.as_deref() {
            original_by_id.insert(id.to_string());
        } else {
            let cleaned = clean_description(
                &task.description,
                &note_settings.global_filter,
                None,
            );
            *original_idless_counts.entry(cleaned).or_insert(0) += 1;
        }
    }
    let mut seen_idless_for_new: BTreeMap<String, usize> = BTreeMap::new();
    let mut ordinal_seen: BTreeMap<String, usize> = BTreeMap::new();
    let mut frozen: Vec<FrozenNotePull> = Vec::new();
    for offer in &offers {
        let digest_index = if offer.block_id.is_some() {
            1
        } else {
            let count = ordinal_seen.entry(offer.text.clone()).or_insert(0);
            *count += 1;
            *count
        };
        let key = stable_task_key(
            &relative,
            offer.block_id.as_deref(),
            &offer.text,
            &note_settings.global_filter,
            digest_index,
        );
        // Earliest-item ownership across every successor class: a later
        // pull omits a row already owned by an earlier item even when it
        // was unselected. Selected predecessors still own their row after
        // minting via the final block-ID identity.
        if earlier_keys.contains(&key) {
            continue;
        }
        if let Some(id) = offer.block_id.as_deref()
            && earlier_block_ids.contains(&(relative.clone(), id.to_string()))
        {
            continue;
        }
        // New rows (created by an earlier item) have no draft-start
        // preimage match: ID rows miss by block ID, ID-less rows miss by
        // cleaned-description ordinal. An empty preimage (newly staged
        // source) makes every row new.
        let is_new = match (
            original_source_text.as_deref(),
            offer.block_id.as_deref(),
        ) {
            (None, _) => false,
            (Some(_), Some(id)) => !original_by_id.contains(id),
            (Some(_), None) => {
                let count =
                    seen_idless_for_new.entry(offer.text.clone()).or_insert(0);
                *count += 1;
                original_idless_counts
                    .get(&offer.text)
                    .copied()
                    .unwrap_or(0)
                    < *count
            }
        };
        let selected = match &request.selection {
            SuccessorSelection::All => true,
            SuccessorSelection::None => false,
            SuccessorSelection::Keys(keys) => keys.contains(&key),
        };
        frozen.push(FrozenNotePull {
            key,
            status: offer.status,
            text: offer.text.clone(),
            block_id: offer.block_id.clone(),
            block_id_needed: offer.block_id.is_none(),
            selected,
            line_index: offer.line_index,
            order_lane:
                crate::native::task_complete::keep_moving::keep_moving_lane(
                    offer.status,
                ),
            raw: offer.raw_status_line.clone(),
            is_new,
            settled_ordinal: None,
        });
    }
    // A later pull whose every row was already owned succeeds empty
    // (pruned) instead of retroactively refusing the valid batch.
    if frozen.is_empty() && !earlier_keys.is_empty() {
        let folded = PullInFoldedJson {
            hidden: folded_hidden,
            already_planned: folded_already,
            sub_projects: 0,
            blocked: folded_blocked,
            scheduled: folded_scheduled,
            already_started: 0,
            waits_for: 0,
        };
        let kind_label = capture_kind_label(&parsed.kind);
        return Ok(PlannedCaptureItem {
            result: CaptureItemResult {
                ok: true,
                dry_run: request.dry_run,
                routed: true,
                route: Some(route.clone()),
                route_label: route.clone(),
                relative_target: note.clone(),
                target: absolute.display().to_string(),
                text: String::new(),
                task_line: String::new(),
                task_theme: None,
                type_relocation: None,
                kind: kind_label,
                fanout: None,
                created: super::plan::date_string(today),
                scheduled: None,
                priority: None,
                priority_label: None,
                placement: Placement::Created,
                sub_bullets: Vec::new(),
                notes: Vec::new(),
                drawers: Vec::new(),
                clip: None,
                schedule_log: None,
                block_id: None,
                day_file: None,
                block_link: None,
                pomodoro_link_placement: None,
                parent_line: None,
                parent_text: None,
                drawer: None,
                parent_status_symbol: None,
                parent_status_name: None,
                toggle_direction: None,
                previous_task_line: None,
                status_symbol: None,
                status_name: None,
                previous_status_symbol: None,
                previous_status_name: None,
                pomodoro_name: None,
                creates_pomodoro: None,
                pomodoro_already_linked: None,
                removed_pomodoro_links: None,
                removed_scheduled: None,
                pomodoro_selector_unused: None,
                toggle_behavior: None,
                status_changed: None,
                pomodoro_link_action: None,
                pomodoro_link_source: None,
                pomodoro_link_destination: None,
                project_note: None,
                pomodoro_start: None,
                pomodoro_adjust: None,
                pomodoro_shift: None,
                pomodoro_split: None,
                pomodoro_merge: None,
                pomodoro_close: None,
                pomodoro_reset: None,
                dependency_update: None,
                consumed_links: Vec::new(),
                task_complete: None,
                r#ref: None,
                toggle_task_description: None,
                link_guard: None,
                dropped_dependencies: Vec::new(),
                blocks_line_updates: Vec::new(),
                minted_dependent_block_id: None,
                dependency_updates: Vec::new(),
                minted_blocker_block_id: None,
                relation_updates: Vec::new(),
                relation_cancels: Vec::new(),
                relation_completes: Vec::new(),
                relation_merge_output: Default::default(),
                linked_from_line_updates: Vec::new(),
                minted_relation_block_ids: Vec::new(),
                mirror_skipped: Vec::new(),
                list: None,
                relation_successors: Vec::new(),
                relation_held: Vec::new(),
                relation_check: None,
                relation_groups: Vec::new(),
                relation_label_hides: Vec::new(),
                note_pull: Some(NotePullSummaryJson {
                    note: note.clone(),
                    route: route.clone(),
                    successors: Vec::new(),
                    folded,
                }),
            },
            clip_plan: None,
            pomodoro_refs: Vec::new(),
            task_block_refs: Vec::new(),
            project_move_landings: Vec::new(),
        });
    }
    // Lane pull order (Pending → Next → Ready), then source line.
    frozen.sort_by(|left, right| {
        left.order_lane
            .cmp(&right.order_lane)
            .then(left.line_index.cmp(&right.line_index))
    });

    // Sequential scratch placement through all offers so a second DOCS row
    // sees the placeholder the first would create. Provisional IDs stand in
    // for ID-less rows; they are never staged or reported as minted.
    let mut scratch_day = day_text.clone();
    let mut scratch_probe: HashSet<String> =
        crate::native::collect_done::block_ids_in_markdown(&scratch_day)
            .into_iter()
            .collect();
    scratch_probe.extend(crate::native::collect_done::block_ids_in_markdown(
        &source_text,
    ));
    let mut scratch_dests: Vec<ScratchDest> = Vec::new();
    for offer in &frozen {
        let block_id = offer.block_id.clone().unwrap_or_else(|| {
            let used: HashSet<&str> =
                scratch_probe.iter().map(String::as_str).collect();
            let minted = crate::native::capture_block_ids::mint_block_id(
                &offer.text,
                &used,
            );
            scratch_probe.insert(minted.clone());
            minted
        });
        let block_link =
            format!("[[{}#^{block_id}]]", note.trim_end_matches(".md"));
        let placement_type = task_theme_context
            .placement_for_task(planner, &route, &offer.raw, warnings);
        let typed_pair = placement_type
            .as_ref()
            .map(|placement| (placement.as_typed(), placement.placed_type()));
        let (typed, _placed_type) = match typed_pair.as_ref() {
            Some((typed, placed_type)) => {
                (Some(typed), Some(placed_type.clone()))
            }
            None => (None, None),
        };
        let insertion = super::pomodoro_insert::insert_pomodoro_block_link(
            &scratch_day,
            &block_link,
            None,
            false,
            typed,
        )?;
        let creates = insertion.creates_pomodoro;
        let used_typed = typed.is_some();
        scratch_day = insertion.updated.clone();
        scratch_dests.push(ScratchDest {
            entry_name: insertion.destination.name.clone().unwrap_or_default(),
            entry_role: if creates {
                "created"
            } else if used_typed {
                "typed"
            } else {
                "implicit"
            },
            fallback: !creates && !used_typed,
            creates,
        });
    }

    // Actual placement processes only picks; an unpicked row cannot create
    // a placeholder.
    let mut actual_day = day_text.clone();
    let mut actual_by_key: BTreeMap<String, ActualDest> = BTreeMap::new();
    for offer in frozen.iter().filter(|offer| offer.selected) {
        let block_id = match offer.block_id.clone() {
            Some(id) => id,
            None => super::blocks_mirror::mint_staged_block_id(
                dependency_ctx,
                planner,
                &absolute,
                &relative,
                &offer.raw,
            )?,
        };
        let block_link =
            format!("[[{}#^{block_id}]]", note.trim_end_matches(".md"));
        let placement_type = task_theme_context
            .placement_for_task(planner, &route, &offer.raw, warnings);
        let typed_pair = placement_type
            .as_ref()
            .map(|placement| (placement.as_typed(), placement.placed_type()));
        let (typed, placed_type) = match typed_pair.as_ref() {
            Some((typed, placed_type)) => {
                (Some(typed), Some(placed_type.clone()))
            }
            None => (None, None),
        };
        let mut insertion = super::pomodoro_insert::insert_pomodoro_block_link(
            &actual_day,
            &block_link,
            None,
            false,
            typed,
        )?;
        if let Some(placed_type) = placed_type {
            insertion.destination.placed_type = Some(placed_type);
        }
        let creates = insertion.creates_pomodoro;
        let used_typed = typed.is_some();
        actual_day = insertion.updated.clone();
        actual_by_key.insert(
            offer.key.clone(),
            ActualDest {
                updated: actual_day.clone(),
                insertion,
                block_id,
                creates,
                used_typed,
            },
        );
    }

    // Apply source edits for picks: status, mint, fresh stamp. Schedules,
    // `[id::]`, and `[dependsOn::]` are untouched. Rows created by an
    // earlier item of the same draft skip the fresh stamp because
    // creation never stamps.
    let mut edited_lines: Vec<String> =
        source_text.split('\n').map(str::to_string).collect();
    // Track final block IDs for reporting.
    let mut final_ids: BTreeMap<String, String> = BTreeMap::new();
    for offer in frozen.iter().filter(|offer| offer.selected) {
        let actual = actual_by_key.get(&offer.key).expect("placed pick");
        final_ids.insert(offer.key.clone(), actual.block_id.clone());
        let Some(line) = edited_lines.get_mut(offer.line_index) else {
            continue;
        };
        let trimmed = line.strip_suffix('\r').unwrap_or(line).to_string();
        let new_status = match offer.status {
            ' ' => '*',
            other => other,
        };
        let mut updated =
            set_task_line_status(&trimmed, new_status).unwrap_or(trimmed);
        if offer.block_id.is_none() {
            updated.push_str(" ^");
            updated.push_str(&actual.block_id);
        }
        if !offer.is_new {
            updated = stamp_fresh(&updated, today).line;
        }
        if line.ends_with('\r') {
            updated.push('\r');
        }
        *line = updated;
    }
    if frozen.iter().any(|offer| offer.selected) {
        planner.stage(&absolute, edited_lines.join("\n"))?;
        planner.stage(&day_file, actual_day.clone())?;
    }
    // Settled duplicate ordinals for ID-less rows among the final staged
    // source: picks above minted IDs, so pre-pick ordinals no longer
    // address the surviving ID-less population. Line numbers shift when
    // later items insert context, so the net pass resolves by cleaned
    // description plus this ordinal.
    {
        let final_source = planner
            .current_contents(&absolute)?
            .unwrap_or_else(|| source_text.clone());
        let final_tasks =
            task_status_hooks::parse_tasks(&final_source, &tasks_settings);
        // Map each final ID-less task's line to its ordinal among
        // equal-description ID-less rows.
        let mut final_ordinal_by_line: BTreeMap<usize, usize> = BTreeMap::new();
        let mut seen_final: BTreeMap<String, usize> = BTreeMap::new();
        for task in &final_tasks {
            if task.block_id.is_some() {
                continue;
            }
            let cleaned = clean_description(
                &task.description,
                &note_settings.global_filter,
                None,
            );
            let count = seen_final.entry(cleaned).or_insert(0);
            *count += 1;
            final_ordinal_by_line.insert(task.line_index, *count);
        }
        for offer in frozen.iter_mut() {
            if offer.block_id.is_some() {
                offer.settled_ordinal = None;
                continue;
            }
            // Selected ID-less rows minted IDs: they resolve by block ID.
            if offer.selected {
                offer.settled_ordinal = None;
                continue;
            }
            // Unselected ID-less rows survive ID-less at the same staged
            // line (this item edits in place, never inserts). Fall back
            // to the pre-pick ordinal when the line moved.
            if let Some(ordinal) =
                final_ordinal_by_line.get(&offer.line_index).copied()
            {
                offer.settled_ordinal = Some(ordinal);
            } else {
                // Line shifted (later items insert): fall back to counting
                // among final ID-less same-text rows by order. Use the
                // pre-pick digest ordinal as a last resort.
                let mut count = 0usize;
                for task in &final_tasks {
                    if task.block_id.is_some() {
                        continue;
                    }
                    let cleaned = clean_description(
                        &task.description,
                        &note_settings.global_filter,
                        None,
                    );
                    if cleaned == offer.text {
                        count += 1;
                    }
                }
                if count > 0 {
                    // Clamp to surviving population; the net pass falls
                    // back to key ordinal when this still misses.
                    offer.settled_ordinal = Some(count.min(
                        ordinal_seen.get(&offer.text).copied().unwrap_or(1),
                    ));
                }
            }
        }
    }

    // Touched source rows report task blocks with the note-pull role so
    // the batch tracker renders each picked task once, in first-touch
    // order, in its final state.
    let task_block_refs: Vec<super::task_blocks::TaskBlockRef> = frozen
        .iter()
        .filter(|offer| offer.selected)
        .filter_map(|offer| {
            let block_id = final_ids
                .get(&offer.key)
                .cloned()
                .or_else(|| offer.block_id.clone())?;
            Some(super::task_blocks::TaskBlockRef {
                target: absolute.clone(),
                relative_target: note.clone(),
                route: route.clone(),
                line: offer.line_index,
                block_id: Some(block_id),
                role: super::task_blocks::TaskBlockRole::NotePull,
            })
        })
        .collect();

    let day_file_display = day_relative.clone();
    let mut successors: Vec<TaskCompleteUnblockedJson> = Vec::new();
    for (position, offer) in frozen.iter().enumerate() {
        let block_id = final_ids
            .get(&offer.key)
            .cloned()
            .or_else(|| offer.block_id.clone())
            .unwrap_or_default();
        let new_status = match offer.status {
            ' ' => '*',
            other => other,
        };
        let (link, not_linked, destination, _order_entry) = if offer.selected {
            let actual = actual_by_key.get(&offer.key).expect("placed pick");
            let dest = &actual.insertion.destination;
            let entry_name = dest.name.clone().unwrap_or_default();
            let (entry_role, fallback) = if actual.creates {
                ("created", false)
            } else if actual.used_typed {
                ("typed", false)
            } else {
                ("implicit", true)
            };
            let stem = note.trim_end_matches(".md");
            let block_link = format!("[[{stem}#^{block_id}]]");
            let line_no = block_link_line(&actual.updated, &block_link);
            (
                Some(TaskCompleteSuccessorLinkJson {
                    day_file: day_file_display.clone(),
                    entry_name: entry_name.clone(),
                    entry_line: dest.line,
                    entry_created: actual.insertion.creates_pomodoro,
                    next_up: dest.role == Some("next_up"),
                    line: line_no,
                    block_link,
                    block_id_created: offer.block_id_needed,
                }),
                None,
                TaskCompleteOfferDestinationJson {
                    day_file: day_file_display.clone(),
                    entry_name,
                    entry_role,
                    fallback,
                },
                dest.line,
            )
        } else {
            let scratch = &scratch_dests[position];
            (
                None,
                Some("not_selected".to_string()),
                TaskCompleteOfferDestinationJson {
                    day_file: day_file_display.clone(),
                    entry_name: scratch.entry_name.clone(),
                    entry_role: scratch.entry_role,
                    fallback: scratch.fallback,
                },
                0,
            )
        };
        // Draft item order precedes lane, path, and line within the
        // note-pull rank; the final `then(item)` tie-break alone is
        // insufficient when route names sort reverse-lexically (NP20).
        let order_key: crate::native::task_complete::OfferOrderKey = (
            OfferGroup::NotePull as u8,
            (item_index, 0),
            offer.order_lane,
            relative.clone(),
            offer.line_index,
        );
        successors.push(TaskCompleteUnblockedJson {
            note_path: note.clone(),
            block_id: block_id.clone(),
            line: offer.line_index + 1,
            text: offer.text.clone(),
            previous_status_symbol: offer.status,
            previous_status_name: super::task_complete::status_name_for(
                &request.bob_dir,
                offer.status,
            ),
            status_symbol: if offer.selected {
                new_status
            } else {
                offer.status
            },
            status_name: super::task_complete::status_name_for(
                &request.bob_dir,
                if offer.selected {
                    new_status
                } else {
                    offer.status
                },
            ),
            inbox,
            unblocked_by: Vec::new(),
            link,
            not_linked,
            trigger: NOTE_PULL_TRIGGER,
            hide_removed: false,
            offer: Some(TaskCompleteOfferJson {
                key: offer.key.clone(),
                n: position + 1,
                group: "note_pull",
                selected: offer.selected,
                block_id_needed: offer.block_id_needed,
                project: None,
                after: None,
                moved_from: None,
                merged_from: None,
                destination,
                order_key,
                settled_ordinal: offer.settled_ordinal,
            }),
            relation: None,
        });
    }

    let folded = PullInFoldedJson {
        hidden: folded_hidden,
        already_planned: folded_already,
        sub_projects: 0,
        blocked: folded_blocked,
        scheduled: folded_scheduled,
        already_started: 0,
        waits_for: 0,
    };

    let kind_label = capture_kind_label(&parsed.kind);
    Ok(PlannedCaptureItem {
        result: CaptureItemResult {
            ok: true,
            dry_run: request.dry_run,
            routed: true,
            route: Some(route.clone()),
            route_label: route.clone(),
            relative_target: note.clone(),
            target: absolute.display().to_string(),
            text: String::new(),
            task_line: String::new(),
            task_theme: None,
            type_relocation: None,
            kind: kind_label,
            fanout: None,
            created: super::plan::date_string(today),
            scheduled: None,
            priority: None,
            priority_label: None,
            placement: Placement::Created,
            sub_bullets: Vec::new(),
            notes: Vec::new(),
            drawers: Vec::new(),
            clip: None,
            schedule_log: None,
            block_id: None,
            day_file: None,
            block_link: None,
            pomodoro_link_placement: None,
            parent_line: None,
            parent_text: None,
            drawer: None,
            parent_status_symbol: None,
            parent_status_name: None,
            toggle_direction: None,
            previous_task_line: None,
            status_symbol: None,
            status_name: None,
            previous_status_symbol: None,
            previous_status_name: None,
            pomodoro_name: None,
            creates_pomodoro: None,
            pomodoro_already_linked: None,
            removed_pomodoro_links: None,
            removed_scheduled: None,
            pomodoro_selector_unused: None,
            toggle_behavior: None,
            status_changed: None,
            pomodoro_link_action: None,
            pomodoro_link_source: None,
            pomodoro_link_destination: None,
            project_note: None,
            pomodoro_start: None,
            pomodoro_adjust: None,
            pomodoro_shift: None,
            pomodoro_split: None,
            pomodoro_merge: None,
            pomodoro_close: None,
            pomodoro_reset: None,
            dependency_update: None,
            consumed_links: Vec::new(),
            task_complete: None,
            r#ref: None,
            toggle_task_description: None,
            link_guard: None,
            dropped_dependencies: Vec::new(),
            blocks_line_updates: Vec::new(),
            minted_dependent_block_id: None,
            dependency_updates: Vec::new(),
            minted_blocker_block_id: None,
            relation_updates: Vec::new(),
            relation_cancels: Vec::new(),
            relation_completes: Vec::new(),
            relation_merge_output: Default::default(),
            linked_from_line_updates: Vec::new(),
            minted_relation_block_ids: Vec::new(),
            mirror_skipped: Vec::new(),
            list: None,
            relation_successors: Vec::new(),
            relation_held: Vec::new(),
            relation_check: None,
            relation_groups: Vec::new(),
            relation_label_hides: Vec::new(),
            note_pull: Some(NotePullSummaryJson {
                note: note.clone(),
                route: route.clone(),
                successors,
                folded,
            }),
        },
        clip_plan: None,
        pomodoro_refs: Vec::new(),
        task_block_refs,
        project_move_landings: Vec::new(),
    })
}

struct NotePullOffer {
    line_index: usize,
    status: char,
    block_id: Option<String>,
    #[allow(dead_code)]
    depends_on_empty: bool,
    text: String,
    #[allow(dead_code)]
    raw_status_line: String,
}

struct FrozenNotePull {
    key: String,
    status: char,
    text: String,
    block_id: Option<String>,
    block_id_needed: bool,
    selected: bool,
    line_index: usize,
    order_lane: u8,
    raw: String,
    /// True when the row was created by an earlier item of the same draft
    /// (absent from the draft-start preimage). Such picks skip the fresh
    /// stamp because creation never stamps.
    is_new: bool,
    /// Settled duplicate ordinal among still-ID-less rows after picks
    /// mint, for the batch net pass to resolve ID-less rows without
    /// relying on shifting line numbers.
    settled_ordinal: Option<usize>,
}

struct ScratchDest {
    entry_name: String,
    entry_role: &'static str,
    fallback: bool,
    #[allow(dead_code)]
    creates: bool,
}

struct ActualDest {
    updated: String,
    insertion: super::pomodoro_insert::PomodoroBlockLinkInsertion,
    block_id: String,
    creates: bool,
    used_typed: bool,
}

#[allow(dead_code)]
fn offer_text_line(offer: &FrozenNotePull) -> String {
    if let Some(id) = offer.block_id.as_deref() {
        format!("- [{}] {} ^{id}", offer.status, offer.text)
    } else {
        format!("- [{}] {}", offer.status, offer.text)
    }
}

fn block_link_line(contents: &str, block_link: &str) -> usize {
    contents
        .lines()
        .position(|line| line.contains(block_link))
        .map(|index| index + 1)
        .unwrap_or(0)
}

/// Vault-wide prerequisite proof for dependency-bearing candidates,
/// honoring the KM31 gate: only built when needed, overlaid with staged
/// text, with unavailable snapshots folding candidates as blocked.
struct SnapshotProof {
    files: Vec<FileScan>,
    states: BTreeMap<(usize, usize), task_status_hooks::TaskDependencyState>,
}

impl SnapshotProof {
    fn is_blocked(
        &self,
        relative: &Path,
        task: &task_status_hooks::TaskLine,
    ) -> bool {
        let Some(source) = self
            .files
            .iter()
            .position(|file| file.relative_path == relative)
        else {
            return true;
        };
        let Some(state) = self.states.get(&(source, task.line_index)) else {
            return true;
        };
        !crate::native::project_dependencies::blocking_open_dependency_ids(
            &self.files[source],
            task,
            &state.open_dependency_ids,
        )
        .is_empty()
    }

    fn proven_open_prj_ids(
        &self,
        relative: &Path,
        prj: &task_status_hooks::TaskLine,
    ) -> Option<Vec<String>> {
        let source = self
            .files
            .iter()
            .position(|file| file.relative_path == relative)?;
        let state = self.states.get(&(source, prj.line_index))?;
        Some(
            crate::native::project_dependencies::blocking_open_dependency_ids(
                &self.files[source],
                prj,
                &state.open_dependency_ids,
            ),
        )
    }
}

fn prepare_snapshot_proof(
    dependency_ctx: &DependencyContext,
    planner: &CaptureBatchPlanner,
    bob_dir: &Path,
    tasks_settings: &task_status_hooks::TasksSettings,
    needed_ids: &BTreeSet<String>,
    source_relative: &Path,
) -> SnapshotProof {
    let staged = super::task_complete::staged_overlay_map(bob_dir, planner);
    let snapshot = dependency_ctx.dependents_snapshot();
    if !snapshot.snapshot_complete(&staged) {
        return SnapshotProof {
            files: Vec::new(),
            states: BTreeMap::new(),
        };
    }
    // Narrow the vault-wide proof to the source note plus notes that can
    // name one of the needed prerequisite ids. Any note holding an open
    // task with one of those ids contains the id substring (its `[id::]`
    // identity), so substring retention keeps every note that can affect
    // the verdict while skipping unrelated dependency-relevant notes.
    // Staged text supersedes disk via the shared overlay rules.
    let overlaid = snapshot.overlaid_matching(&staged, |relative, text| {
        if relative == source_relative {
            return true;
        }
        needed_ids.iter().any(|id| text.contains(id.as_str()))
    });
    let files: Vec<FileScan> = overlaid
        .into_iter()
        .map(|(relative_path, contents)| {
            let tasks =
                task_status_hooks::parse_tasks(&contents, tasks_settings);
            FileScan {
                path: bob_dir.join(&relative_path),
                relative_path,
                contents,
                tasks,
            }
        })
        .collect();
    let states =
        task_status_hooks::task_dependency_states(&files, &BTreeSet::new());
    SnapshotProof { files, states }
}

fn pull_note_missing_error(route: &str, note: &str) -> CaptureError {
    CaptureError::usage_code(
        format!(
            "@{route} pulls ready tasks from {note}, which does not exist: \
             add `- <task>` bullets below it to start a list there, or run \
             'bob capture-targets' to list routable notes"
        ),
        "pull_note_missing",
    )
}

#[allow(clippy::too_many_arguments)]
fn nothing_to_pull_in_error(
    tasks: &[task_status_hooks::TaskLine],
    note: &str,
    route: &str,
    today: NaiveDate,
    already: usize,
    blocked: usize,
    scheduled: usize,
    hidden: usize,
    relative: &Path,
    proof: Option<&SnapshotProof>,
) -> CaptureError {
    let mut extra = serde_json::Map::new();
    extra.insert(
        "note_pull".to_string(),
        serde_json::to_value(&NotePullSummaryJson {
            note: note.to_string(),
            route: route.to_string(),
            successors: Vec::new(),
            folded: PullInFoldedJson {
                hidden,
                already_planned: already,
                sub_projects: 0,
                blocked,
                scheduled,
                already_started: 0,
                waits_for: 0,
            },
        })
        .expect("serialize note_pull"),
    );
    let message = if blocked > 0
        && let Some(reason) =
            blocked_project_reason(tasks, today, relative, proof)
    {
        format!(
            "nothing to pull in from {note}: the project is blocked ({reason})"
        )
    } else if already + blocked + scheduled + hidden > 0 {
        let mut parts: Vec<String> = Vec::new();
        if already > 0 {
            parts.push(format!("{already} already planned"));
        }
        if blocked > 0 {
            parts.push(format!("{blocked} blocked"));
        }
        if scheduled > 0 {
            parts.push(format!("{scheduled} scheduled later"));
        }
        if hidden > 0 {
            parts.push(format!("{hidden} hidden"));
        }
        format!("nothing to pull in from {note}: {}", parts.join(" · "))
    } else {
        format!("nothing to pull in from {note}: it has no open tasks")
    };
    let mut error = CaptureError::usage_code(message, "nothing_to_pull_in");
    error.extra = Some(extra);
    error
}

fn blocked_project_reason(
    tasks: &[task_status_hooks::TaskLine],
    today: NaiveDate,
    relative: &Path,
    proof: Option<&SnapshotProof>,
) -> Option<String> {
    let prj = tasks
        .iter()
        .find(|task| task.block_id.as_deref() == Some("prj"))?;
    if prj.scheduled.is_some_and(|date| date > today) {
        let date = prj
            .scheduled
            .map(|date| date.format("%Y-%m-%d").to_string())
            .unwrap_or_default();
        return Some(format!("is scheduled for {date}"));
    }
    if !prj.depends_on.is_empty() {
        // Prove which prerequisites remain open using the same
        // completeness rule as classification: unavailable proof stays
        // blocked, and staged overlays are already applied. Never report
        // a closed prerequisite as waiting.
        let open_count = proof
            .and_then(|proof| proof.proven_open_prj_ids(relative, prj))
            .map(|ids| ids.len());
        let count = match open_count {
            Some(count) if count > 0 => count,
            Some(_) => {
                // Proven closed: fall through to the `?` wording when the
                // project line itself is blocked, otherwise no project
                // reason.
                if prj.status == '?' {
                    return Some("its ^prj is blocked".to_string());
                }
                return None;
            }
            None => prj.depends_on.len(),
        };
        let task_word = if count == 1 { "task" } else { "tasks" };
        return Some(format!("its ^prj waits on {count} {task_word}"));
    }
    if prj.status == '?' {
        return Some("its ^prj is blocked".to_string());
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native::capture_language::{CaptureKind, ParsedCaptureText};
    use crate::native::env::TestEnvGuard;
    use crate::native::task_complete::SuccessorSelection;
    use std::fs;

    const TODAY_YMD: (i32, u32, u32) = (2026, 9, 20);

    fn today() -> NaiveDate {
        NaiveDate::from_ymd_opt(TODAY_YMD.0, TODAY_YMD.1, TODAY_YMD.2)
            .expect("valid test date")
    }

    struct Harness {
        _temp: tempfile::TempDir,
        _guard: TestEnvGuard,
        request: CaptureRequest,
        planner: CaptureBatchPlanner,
        dependency_ctx: DependencyContext,
        task_theme_context: CaptureTaskThemeContext,
        warnings: Vec<CaptureWarning>,
    }

    fn vault_harness(
        files: &[(&str, &str)],
        day: Option<&str>,
        selection: SuccessorSelection,
    ) -> Harness {
        let temp = tempfile::tempdir().expect("create note-pull vault");
        let vault = temp.path().join("vault");
        fs::create_dir_all(&vault).expect("create vault");
        for (name, contents) in files {
            let path = vault.join(name);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).expect("create parent");
            }
            fs::write(&path, contents).expect("write fixture note");
        }
        let day_file = vault.join("day.md");
        if let Some(day) = day {
            fs::write(&day_file, day).expect("write day file");
        }
        // Tasks status knowledge mirrors the capture offer harness:
        // open lanes plus the blocked marker status.
        let tasks_settings =
            vault.join(".obsidian/plugins/obsidian-tasks-plugin");
        fs::create_dir_all(&tasks_settings).expect("create Tasks settings");
        fs::write(
            tasks_settings.join("data.json"),
            r##"{
          "globalFilter": "#task",
          "taskFormat": "dataview",
          "statusSettings": {
            "coreStatuses": [
              {"symbol":" ","name":"Ready","nextStatusSymbol":"x","availableAsCommand":true,"type":"TODO"},
              {"symbol":"x","name":"Done","nextStatusSymbol":" ","availableAsCommand":true,"type":"DONE"}
            ],
            "customStatuses": [
              {"symbol":"/","name":"In Progress","nextStatusSymbol":"x","availableAsCommand":true,"type":"IN_PROGRESS"},
              {"symbol":"*","name":"Next","nextStatusSymbol":"x","availableAsCommand":true,"type":"ON_HOLD"},
              {"symbol":"?","name":"Blocked","nextStatusSymbol":" ","availableAsCommand":true,"type":"ON_HOLD"},
              {"symbol":"-","name":"Canceled","nextStatusSymbol":" ","availableAsCommand":true,"type":"CANCELLED"}
            ]
          }
        }"##,
        )
        .expect("write Tasks settings");
        // Isolate plan config so theme placement never reads the host's
        // real config: a missing file loads `PlanConfig::default()`.
        let missing_config = temp.path().join("missing-bob-config.toml");
        let guard = TestEnvGuard::set(&[
            ("BOB_NOW", Some(std::ffi::OsStr::new("2026-09-20 14:31:07"))),
            ("BOB_DAY_FILE", Some(day_file.as_os_str())),
            ("BOB_CONFIG_FILE", Some(missing_config.as_os_str())),
            ("TZ", Some(std::ffi::OsStr::new("UTC0"))),
        ]);
        let request = CaptureRequest {
            bob_dir: vault,
            dry_run: true,
            forced_clip: None,
            forced_destination_flags: Vec::new(),
            forced_route: None,
            forced_section: None,
            forced_sub_bullet_target: None,
            forced_drawer: None,
            inbox_routes: Vec::new(),
            no_clip: false,
            no_ref: false,
            pull_in: None,
            refuse_override: false,
            require_inbox_route_choice: false,
            require_pull_in_choice: false,
            template_origin: None,
            raw_text: "@sase".to_string(),
            selection,
        };
        let today_date = today();
        Harness {
            planner: CaptureBatchPlanner::default(),
            dependency_ctx: DependencyContext::new(
                &request.bob_dir,
                today_date,
            ),
            task_theme_context: CaptureTaskThemeContext::new(
                &request.bob_dir,
                today_date,
                None,
            ),
            warnings: Vec::new(),
            request,
            _temp: temp,
            _guard: guard,
        }
    }

    fn parsed_pull() -> ParsedCaptureText {
        ParsedCaptureText {
            body: String::new(),
            clip: None,
            route: Some("sase".to_string()),
            kind: CaptureKind::NotePull {
                route: "sase".to_string(),
            },
            scheduled_offset: None,
            priority_level: None,
            sub_bullets: Vec::new(),
            dependencies: Vec::new(),
            blocked_tasks: Vec::new(),
            relations: Vec::new(),
            dependency_target: None,
            type_marker: None,
        }
    }

    fn plan(harness: &mut Harness) -> Result<PlannedCaptureItem, CaptureError> {
        let parsed = parsed_pull();
        let empty_keys = std::collections::BTreeSet::new();
        let empty_ids = std::collections::BTreeSet::new();
        plan_note_pull_item(
            &harness.request,
            parsed,
            today(),
            &mut harness.planner,
            &mut harness.dependency_ctx,
            &mut harness.task_theme_context,
            &mut harness.warnings,
            0,
            &empty_keys,
            &empty_ids,
        )
    }

    fn staged(harness: &Harness, name: &str) -> Option<String> {
        harness
            .planner
            .peek_text(&harness.request.bob_dir.join(name))
    }

    fn summary(planned: &PlannedCaptureItem) -> &NotePullSummaryJson {
        planned
            .result
            .note_pull
            .as_ref()
            .expect("note_pull payload")
    }

    const FIX_DAY: &str = "## Pomodoros\n- [ ] () \u{2014} FIX\n";

    const PBA: &str = concat!(
        "- [/] #task Ship the badge renderer [id:: p] ^ship-renderer\n",
        "- [*] #task Wire badges into the README [id:: b] ^readme-badges\n",
        "- [ ] #task Write badge docs [id:: a]\n",
    );

    // NP1: one `note_pull` group orders Pending, Next, Ready; rows carry
    // `trigger: note_pull` with empty `unblocked_by`; nothing is written.
    #[test]
    fn np1_no_pick_reports_lane_order_and_writes_nothing() {
        let mut harness = vault_harness(
            &[("sase.md", PBA)],
            Some(FIX_DAY),
            SuccessorSelection::None,
        );
        let planned = plan(&mut harness).expect("NP1 plans");
        assert!(
            harness.dependency_ctx.snapshot_read_counts().is_empty(),
            "dependency-free pulls must not read the dependents snapshot"
        );
        assert_eq!(
            harness.dependency_ctx.catalog_walk_count(),
            0,
            "a pull with no day links and no inbox-parent metadata must not walk the vault"
        );
        assert!(planned.result.ok);
        assert_eq!(planned.result.kind, "note_pull");
        let pull = summary(&planned);
        assert_eq!(pull.note, "sase.md");
        assert_eq!(pull.route, "sase");
        assert_eq!(pull.successors.len(), 3);
        let texts: Vec<&str> = pull
            .successors
            .iter()
            .map(|row| row.text.as_str())
            .collect();
        assert_eq!(
            texts,
            vec![
                "Ship the badge renderer",
                "Wire badges into the README",
                "Write badge docs",
            ]
        );
        for row in &pull.successors {
            assert_eq!(row.trigger, NOTE_PULL_TRIGGER);
            assert!(row.unblocked_by.is_empty());
            assert!(!row.inbox);
            assert_eq!(row.not_linked.as_deref(), Some("not_selected"));
            assert!(row.link.is_none());
            let offer = row.offer.as_ref().expect("offer");
            assert_eq!(offer.group, "note_pull");
            assert!(offer.project.is_none());
            assert!(!offer.selected);
        }
        assert_eq!(
            pull.successors
                .iter()
                .map(|row| row.offer.as_ref().expect("offer").n)
                .collect::<Vec<_>>(),
            vec![1, 2, 3]
        );
        // Write-free preview: staged bytes still equal the fixture inputs.
        assert_eq!(staged(&harness, "sase.md"), Some(PBA.to_string()));
        assert_eq!(staged(&harness, "day.md"), Some(FIX_DAY.to_string()));
    }

    #[test]
    fn dependency_free_pull_without_day_links_does_not_walk_note_catalog() {
        let harness = vault_harness(
            &[("sase.md", PBA)],
            Some(FIX_DAY),
            SuccessorSelection::None,
        );
        let batch = crate::native::capture::plan::plan_capture_batch(
            &harness.request,
            None,
        )
        .expect("dependency-free note pull plans");
        assert_eq!(
            batch.catalog_walks, 0,
            "the note-pull net pass must skip the catalog when the day has no task links"
        );
        assert!(batch.snapshot_read_counts.is_empty());
    }

    // NP2: `-p all` links every offer; Ready becomes Next while Pending
    // and Next keep their symbols; missing IDs are minted; every pick is
    // stamped fresh and lands at a typed-or-implicit destination.
    #[test]
    fn np2_pick_all_links_statuses_mints_and_stamps() {
        let mut harness = vault_harness(
            &[("sase.md", PBA)],
            Some(FIX_DAY),
            SuccessorSelection::All,
        );
        let planned = plan(&mut harness).expect("NP2 plans");
        let pull = summary(&planned);
        assert_eq!(pull.successors.len(), 3);
        let symbols: Vec<char> = pull
            .successors
            .iter()
            .map(|row| row.status_symbol)
            .collect();
        assert_eq!(symbols, vec!['/', '*', '*']);
        for row in &pull.successors {
            assert!(row.link.is_some(), "picked row links");
            assert_eq!(row.not_linked, None);
            assert!(row.offer.as_ref().expect("offer").selected);
        }
        // The ID-less Ready row minted an ID and reports it.
        let ready = &pull.successors[2];
        assert!(!ready.block_id.is_empty());
        assert!(ready.link.as_ref().expect("link").block_id_created);
        assert!(
            !pull.successors[0]
                .link
                .as_ref()
                .expect("link")
                .block_id_created
        );
        // Source bytes: statuses flipped, IDs minted, fresh stamped.
        let staged_source = staged(&harness, "sase.md").expect("source staged");
        assert!(staged_source.contains("- [*] #task Write badge docs"));
        assert!(staged_source.contains("[fresh:: 2026-09-20]"));
        assert!(staged_source.contains(&format!("^{}", ready.block_id)));
        // Day bytes: all three links landed under FIX.
        let staged_day = staged(&harness, "day.md").expect("day staged");
        assert!(staged_day.contains("[[sase#^ship-renderer]]"));
        assert!(staged_day.contains("[[sase#^readme-badges]]"));
        assert!(staged_day.contains(&format!("[[sase#^{}]]", ready.block_id)));
    }

    // NP3: a live Task Link under today's open Pomodoros folds the row as
    // already planned.
    #[test]
    fn np3_live_link_folds_already_planned() {
        let day = format!("{FIX_DAY}  - [[sase#^readme-badges]]\n");
        let mut harness = vault_harness(
            &[("sase.md", PBA)],
            Some(&day),
            SuccessorSelection::None,
        );
        let planned = plan(&mut harness).expect("NP3 plans");
        let pull = summary(&planned);
        assert_eq!(pull.successors.len(), 2);
        assert_eq!(pull.folded.already_planned, 1);
        assert!(pull
            .successors
            .iter()
            .all(|row| row.text != "Wire badges into the README"));
    }

    // NP4: a struck link, or a link under a closed Pomodoro, is not live,
    // so the row stays offered.
    #[test]
    fn np4_struck_or_closed_links_stay_offered() {
        for day in [
            format!("{FIX_DAY}  - ~~[[sase#^readme-badges]]~~\n"),
            "## Pomodoros\n- [x] () \u{2014} FIX\n  - [[sase#^readme-badges]]\n- [ ] () \u{2014} WORK\n"
                .to_string(),
        ] {
            let mut harness = vault_harness(
                &[("sase.md", PBA)],
                Some(&day),
                SuccessorSelection::None,
            );
            let planned = plan(&mut harness).expect("NP4 plans");
            assert_eq!(summary(&planned).successors.len(), 3, "{day}");
        }
    }

    // NP5: fold precedence — blocked `?`, an open own prerequisite, and
    // `#prj_is_blocked` fold as blocked; future-scheduled folds as
    // scheduled; `#hide` folds as hidden; a scheduled date of today is
    // eligible.
    #[test]
    fn np5_fold_precedence_and_today_scheduled_offered() {
        let source = concat!(
            "- [?] #task Blocked row ^q\n",
            "- [ ] #task Needs proof [dependsOn:: open1] [id:: np] ^np\n",
            "- [ ] #task Opener [id:: open1] ^open1\n",
            "- [ ] #task Tagged #prj_is_blocked ^pb\n",
            "- [ ] #task Tomorrow [scheduled:: 2026-09-21] ^tm\n",
            "- [ ] #task Hidden #hide ^hi\n",
            "- [ ] #task Today [scheduled:: 2026-09-20] ^td\n",
            "- [ ] #task Plain ^pl\n",
        );
        let mut harness = vault_harness(
            &[("sase.md", source)],
            Some(FIX_DAY),
            SuccessorSelection::None,
        );
        let planned = plan(&mut harness).expect("NP5 plans");
        let pull = summary(&planned);
        assert_eq!(pull.folded.blocked, 3);
        assert_eq!(pull.folded.scheduled, 1);
        assert_eq!(pull.folded.hidden, 1);
        let texts: Vec<&str> = pull
            .successors
            .iter()
            .map(|row| row.text.as_str())
            .collect();
        assert!(texts.contains(&"Opener"));
        assert!(texts.contains(&"Today"));
        assert!(texts.contains(&"Plain"));
        assert!(!texts.contains(&"Needs proof"));
    }

    // NP6: the source note's own `^prj` is never offered or counted.
    #[test]
    fn np6_own_prj_never_offered_or_counted() {
        let source =
            concat!("- [ ] #task #prj Sase ^prj\n", "- [ ] #task Plain ^pl\n",);
        let mut harness = vault_harness(
            &[("sase.md", source)],
            Some(FIX_DAY),
            SuccessorSelection::None,
        );
        let planned = plan(&mut harness).expect("NP6 plans");
        let pull = summary(&planned);
        assert_eq!(pull.successors.len(), 1);
        assert_eq!(pull.successors[0].text, "Plain");
        assert_eq!(pull.folded.blocked, 0);
        assert_eq!(pull.folded.already_planned, 0);
        assert_eq!(pull.folded.scheduled, 0);
        assert_eq!(pull.folded.hidden, 0);
    }

    // NP7: only the named note's residents are offered; tasks living in
    // other files never appear.
    #[test]
    fn np7_only_source_residents_offered() {
        let mut harness = vault_harness(
            &[
                ("sase.md", "- [ ] #task Home row ^home\n"),
                ("other.md", "- [ ] #task Away row ^away\n"),
            ],
            Some(FIX_DAY),
            SuccessorSelection::None,
        );
        let planned = plan(&mut harness).expect("NP7 plans");
        let pull = summary(&planned);
        assert_eq!(pull.successors.len(), 1);
        assert_eq!(pull.successors[0].note_path, "sase.md");
    }

    // NP8: done, cancelled, and custom-status residents are ignored and
    // uncounted, so an all-closed note refuses with the no-open-tasks
    // copy.
    #[test]
    fn np8_closed_statuses_ignored_uncounted() {
        let source = concat!(
            "- [x] #task Done [id:: closed] [dependsOn:: missing] ^d\n",
            "- [-] #task Gone ^c\n",
            "- [>] #task Custom ^u\n",
        );
        let mut harness = vault_harness(
            &[("sase.md", source)],
            Some(FIX_DAY),
            SuccessorSelection::None,
        );
        let Err(error) = plan(&mut harness) else {
            panic!("NP8 refuses unexpectedly planned");
        };
        assert_eq!(error.code.as_deref(), Some("nothing_to_pull_in"));
        assert_eq!(
            error.message,
            "nothing to pull in from sase.md: it has no open tasks"
        );
        assert_eq!(staged(&harness, "sase.md"), Some(source.to_string()));
        assert!(
            harness.dependency_ctx.snapshot_read_counts().is_empty(),
            "closed dependency rows must not trigger a vault-wide snapshot"
        );
    }

    // NP9: `#ref` tasks, nested subtasks, and recurring tasks due today
    // are offered like any resident.
    #[test]
    fn np9_ref_nested_recurring_offered() {
        let source = concat!(
            "- [ ] #task #ref Read the paper [id:: rf] ^rf\n",
            "- [ ] #task Parent [id:: nest] ^nest\n",
            "  - [ ] #task Nested [id:: nested] ^nested\n",
            "- [ ] #task Water [repeat:: weekly] [id:: rec] ^rec\n",
        );
        let mut harness = vault_harness(
            &[("sase.md", source)],
            Some(FIX_DAY),
            SuccessorSelection::None,
        );
        let planned = plan(&mut harness).expect("NP9 plans");
        let texts: Vec<&str> = summary(&planned)
            .successors
            .iter()
            .map(|row| row.text.as_str())
            .collect();
        assert!(texts.contains(&"#ref Read the paper"), "{texts:?}");
        assert!(texts.contains(&"Nested"), "{texts:?}");
        assert!(texts.contains(&"Water"), "{texts:?}");
    }

    // NP10: every resident folded, or no open tasks at all, refuses with
    // `nothing_to_pull_in` and writes nothing.
    #[test]
    fn np10_empty_refusals_carry_fold_and_write_nothing() {
        let mut harness = vault_harness(
            &[(
                "sase.md",
                "- [?] #task Blocked ^q\n- [ ] #task Hidden #hide ^hi\n",
            )],
            Some(FIX_DAY),
            SuccessorSelection::All,
        );
        let Err(error) = plan(&mut harness) else {
            panic!("NP10 folded refuses unexpectedly planned");
        };
        assert_eq!(error.code.as_deref(), Some("nothing_to_pull_in"));
        assert_eq!(
            error.message,
            "nothing to pull in from sase.md: 1 blocked · 1 hidden"
        );
        let payload = error
            .extra
            .as_ref()
            .and_then(|extra| extra.get("note_pull"))
            .expect("note_pull payload");
        assert_eq!(payload["note"], "sase.md");
        assert_eq!(payload["route"], "sase");
        assert_eq!(payload["folded"]["blocked"], 1);
        assert_eq!(payload["folded"]["hidden"], 1);
        assert_eq!(
            staged(&harness, "sase.md"),
            Some(
                "- [?] #task Blocked ^q\n- [ ] #task Hidden #hide ^hi\n"
                    .to_string()
            )
        );
        assert_eq!(staged(&harness, "day.md"), Some(FIX_DAY.to_string()));
    }

    // NP11: a blocked project names its `^prj` reason in the empty-pull
    // message instead of the plain fold line.
    #[test]
    fn np11_blocked_project_names_prj_reason() {
        let mut harness = vault_harness(
            &[
                (
                    "sase.md",
                    concat!(
                        "- [?] #task #prj Sase [dependsOn:: open1] ^prj\n",
                        "- [ ] #task Resident #prj_is_blocked ^r\n",
                    ),
                ),
                ("other.md", "- [ ] #task Opener [id:: open1] ^open1\n"),
            ],
            Some(FIX_DAY),
            SuccessorSelection::All,
        );
        let Err(error) = plan(&mut harness) else {
            panic!("NP11 refuses unexpectedly planned");
        };
        assert_eq!(error.code.as_deref(), Some("nothing_to_pull_in"));
        assert_eq!(
            error.message,
            "nothing to pull in from sase.md: the project is blocked \
             (its ^prj waits on 1 task)"
        );
    }

    #[test]
    fn np11_scheduled_project_names_date_reason() {
        let mut harness = vault_harness(
            &[(
                "sase.md",
                concat!(
                    "- [?] #task #prj Sase [scheduled:: 2026-09-21] ^prj\n",
                    "- [ ] #task Resident #prj_is_blocked ^r\n",
                ),
            )],
            Some(FIX_DAY),
            SuccessorSelection::All,
        );
        let Err(error) = plan(&mut harness) else {
            panic!("NP11 scheduled refuses unexpectedly planned");
        };
        assert_eq!(error.code.as_deref(), Some("nothing_to_pull_in"));
        assert_eq!(
            error.message,
            "nothing to pull in from sase.md: the project is blocked \
             (is scheduled for 2026-09-21)"
        );
    }

    // Fold gating: a hidden, scheduled, or already-planned row with a
    // closed prerequisite folds under its later filter, not as blocked.
    // A source holding only such a row refuses with that fold, and adding
    // an unrelated eligible dependency row must not change the first
    // row's fold.
    #[test]
    fn fold_gating_closed_prereq_keeps_later_filter() {
        let closed = "- [x] #task Done [id:: closed1] ^closed1\n";
        for (name, source, day, expect_hidden, expect_scheduled, expect_already) in [
            (
                "hidden",
                "- [ ] #task Hidden [dependsOn:: closed1] #hide ^hi\n".to_string(),
                FIX_DAY.to_string(),
                1usize,
                0usize,
                0usize,
            ),
            (
                "scheduled",
                "- [ ] #task Later [dependsOn:: closed1] [scheduled:: 2026-09-21] ^lt\n"
                    .to_string(),
                FIX_DAY.to_string(),
                0usize,
                1usize,
                0usize,
            ),
            (
                "already",
                "- [ ] #task Live [dependsOn:: closed1] ^live\n".to_string(),
                format!("{FIX_DAY}  - [[sase#^live]]\n"),
                0usize,
                0usize,
                1usize,
            ),
        ] {
            let mut harness = vault_harness(
                &[("sase.md", &source), ("other.md", closed)],
                Some(&day),
                SuccessorSelection::None,
            );
            let Err(error) = plan(&mut harness) else {
                panic!("{name} with only a filtered row must refuse");
            };
            assert_eq!(
                error.code.as_deref(),
                Some("nothing_to_pull_in"),
                "{name}"
            );
            let payload = error
                .extra
                .as_ref()
                .and_then(|extra| extra.get("note_pull"))
                .expect("refusal carries note_pull");
            let folded_count = |key: &str| -> u64 {
                payload
                    .get("folded")
                    .and_then(|folded| folded.get(key))
                    .and_then(|value| value.as_u64())
                    .unwrap_or(0)
            };
            assert_eq!(folded_count("hidden"), expect_hidden as u64, "{name} hidden");
            assert_eq!(
                folded_count("scheduled"),
                expect_scheduled as u64,
                "{name} scheduled"
            );
            assert_eq!(
                folded_count("already_planned"),
                expect_already as u64,
                "{name} already"
            );
            assert_eq!(folded_count("blocked"), 0, "{name} blocked");
            // Same source plus an unrelated eligible row keeps the first
            // row's fold while offering the new row.
            let mut extended = source.clone();
            extended.push_str("- [ ] #task Plain ^pl\n");
            let mut harness = vault_harness(
                &[("sase.md", &extended), ("other.md", closed)],
                Some(&day),
                SuccessorSelection::None,
            );
            let planned =
                plan(&mut harness).expect("extended source plans");
            let pull = summary(&planned);
            assert_eq!(pull.folded.hidden, expect_hidden, "{name} extended");
            assert_eq!(
                pull.folded.scheduled, expect_scheduled,
                "{name} extended"
            );
            assert_eq!(
                pull.folded.already_planned, expect_already,
                "{name} extended"
            );
            assert_eq!(pull.folded.blocked, 0, "{name} extended blocked");
            assert!(
                pull.successors.iter().any(|row| row.text == "Plain"),
                "{name} extended offers plain"
            );
        }
    }

    // Blocked-project reason counts only truly open prerequisites: one
    // closed and one open prerequisite reports one task, and a staged
    // close of the open prerequisite removes the project reason.
    #[test]
    fn blocked_project_counts_only_open_prerequisites() {
        let mut harness = vault_harness(
            &[
                (
                    "sase.md",
                    concat!(
                        "- [?] #task #prj Sase [dependsOn:: open1, closed1] ^prj\n",
                        "- [ ] #task Resident #prj_is_blocked ^r\n",
                    ),
                ),
                ("other.md", "- [ ] #task Opener [id:: open1] ^open1\n"),
                ("closed.md", "- [x] #task Done [id:: closed1] ^closed1\n"),
            ],
            Some(FIX_DAY),
            SuccessorSelection::All,
        );
        let Err(error) = plan(&mut harness) else {
            panic!("blocked project refuses");
        };
        assert_eq!(
            error.message,
            "nothing to pull in from sase.md: the project is blocked \
             (its ^prj waits on 1 task)"
        );
        // Staging a close of the open prerequisite proves the project
        // unblocked for the reason: the `?` line still blocks, so the
        // refusal keeps the generic blocked wording without a count.
        let mut harness = vault_harness(
            &[
                (
                    "sase.md",
                    concat!(
                        "- [?] #task #prj Sase [dependsOn:: open1, closed1] ^prj\n",
                        "- [ ] #task Resident #prj_is_blocked ^r\n",
                    ),
                ),
                ("other.md", "- [x] #task Opener [id:: open1] ^open1\n"),
                ("closed.md", "- [x] #task Done [id:: closed1] ^closed1\n"),
            ],
            Some(FIX_DAY),
            SuccessorSelection::All,
        );
        let Err(error) = plan(&mut harness) else {
            panic!("staged-closed project refuses");
        };
        assert_eq!(
            error.message,
            "nothing to pull in from sase.md: the project is blocked \
             (its ^prj is blocked)"
        );
    }

    // NP12: a missing source note refuses `pull_note_missing` and writes
    // nothing.
    #[test]
    fn np12_missing_source_refuses_pull_note_missing() {
        let mut harness =
            vault_harness(&[], Some(FIX_DAY), SuccessorSelection::None);
        let Err(error) = plan(&mut harness) else {
            panic!("NP12 refuses unexpectedly planned");
        };
        assert_eq!(error.code.as_deref(), Some("pull_note_missing"));
        assert_eq!(
            error.message,
            "@sase pulls ready tasks from sase.md, which does not exist: \
             add `- <task>` bullets below it to start a list there, or run \
             'bob capture-targets' to list routable notes"
        );
        assert_eq!(staged(&harness, "sase.md"), None);
        assert_eq!(staged(&harness, "day.md"), None);
    }

    // NP15: `-p none` succeeds, reports every offer unselected, mints no
    // IDs, and writes nothing.
    #[test]
    fn np15_none_selection_writes_nothing_and_mints_nothing() {
        let source =
            concat!("- [*] #task Has id ^b\n", "- [ ] #task No id yet\n",);
        let mut harness = vault_harness(
            &[("sase.md", source)],
            Some(FIX_DAY),
            SuccessorSelection::None,
        );
        let planned = plan(&mut harness).expect("NP15 plans");
        let pull = summary(&planned);
        assert_eq!(pull.successors.len(), 2);
        assert!(pull.successors.iter().all(|row| row.link.is_none()
            && row.not_linked.as_deref() == Some("not_selected")));
        let idless = pull
            .successors
            .iter()
            .find(|row| row.text == "No id yet")
            .expect("id-less offer");
        assert_eq!(idless.block_id, "");
        assert_eq!(staged(&harness, "sase.md"), Some(source.to_string()));
        assert_eq!(staged(&harness, "day.md"), Some(FIX_DAY.to_string()));
    }

    // NP21: destinations use the normal typed rules without a predecessor;
    // without theme config a typeless pick lands implicit with fallback.
    // A placeholder is written only when a picked link lands there: with
    // no picks the day is untouched, and a subset pick links only the
    // picked row even when an unselected same-shape row precedes it.
    #[test]
    fn np21_subset_pick_links_only_picked_row() {
        let source = concat!(
            "- [ ] #task First typeless ^first\n",
            "- [ ] #task Second typeless ^second\n",
        );
        let mut harness = vault_harness(
            &[("sase.md", source)],
            Some(FIX_DAY),
            SuccessorSelection::Keys(
                ["sase.md#^second".to_string()].into_iter().collect(),
            ),
        );
        let planned = plan(&mut harness).expect("NP21 plans");
        let pull = summary(&planned);
        assert_eq!(pull.successors.len(), 2);
        assert!(!pull.successors[0].offer.as_ref().expect("offer").selected);
        assert!(pull.successors[1].offer.as_ref().expect("offer").selected);
        assert!(pull.successors[1].link.is_some());
        assert!(
            pull.successors[1]
                .offer
                .as_ref()
                .expect("offer")
                .destination
                .fallback
        );
        let staged_day = staged(&harness, "day.md").expect("day staged");
        assert!(staged_day.contains("[[sase#^second]]"));
        assert!(!staged_day.contains("[[sase#^first]]"));
        let staged_source = staged(&harness, "sase.md").expect("source staged");
        assert!(staged_source.contains("- [*] #task Second typeless"));
        assert!(staged_source.contains("- [ ] #task First typeless"));
    }

    // NP22: when prerequisite proof is unavailable, a dependency-bearing
    // candidate folds as blocked instead of being offered.
    #[test]
    fn np22_unavailable_proof_folds_candidate_blocked() {
        let proof = SnapshotProof {
            files: Vec::new(),
            states: BTreeMap::new(),
        };
        let task = crate::native::task_status_hooks::TaskLine {
            line_index: 0,
            status: ' ',
            status_byte_offset: 0,
            block_id: Some("np".to_string()),
            task_id: Some("np".to_string()),
            depends_on: vec!["open1".to_string()],
            scheduled: None,
            status_type: crate::native::task_status_hooks::TaskStatusType::Todo,
            status_recognized: true,
            description: "Needs proof".to_string(),
        };
        assert!(proof.is_blocked(Path::new("sase.md"), &task));
    }

    #[test]
    fn staged_closed_prerequisite_is_visible_to_note_pull_snapshot() {
        let mut harness = vault_harness(
            &[(
                "sase.md",
                "- [ ] #task Dependent [dependsOn:: pre] ^dependent\n",
            )],
            Some(FIX_DAY),
            SuccessorSelection::None,
        );
        let staged_prerequisite = harness.request.bob_dir.join("pre.md");
        harness
            .planner
            .stage(
                &staged_prerequisite,
                "- [x] #task Prerequisite [id:: pre] ^pre\n".to_string(),
            )
            .expect("stage closed prerequisite note");

        let planned = plan(&mut harness).expect("staged-overlay pull plans");
        let pull = summary(&planned);
        assert_eq!(pull.folded.blocked, 0);
        assert_eq!(pull.successors.len(), 1);
        assert_eq!(pull.successors[0].text, "Dependent");
        assert!(
            !harness.dependency_ctx.snapshot_read_counts().is_empty(),
            "the dependency-bearing pull must consult its snapshot"
        );
    }

    // NP23: no daily note, or a daily note without an eligible Pomodoro
    // section, returns the usual error and writes nothing.
    #[test]
    fn np23_missing_day_uses_existing_error() {
        let mut harness =
            vault_harness(&[("sase.md", PBA)], None, SuccessorSelection::All);
        let Err(error) = plan(&mut harness) else {
            panic!("NP23 missing day unexpectedly planned");
        };
        assert!(error.message.contains("Bob daily note does not exist"));
        assert_eq!(staged(&harness, "sase.md"), Some(PBA.to_string()));
    }

    #[test]
    fn np23_day_without_pomodoros_uses_existing_error() {
        let mut harness = vault_harness(
            &[("sase.md", PBA)],
            Some("# Day\nno pomodoros here\n"),
            SuccessorSelection::All,
        );
        let Err(error) = plan(&mut harness) else {
            panic!("NP23 no section unexpectedly planned");
        };
        assert!(!error.message.is_empty());
        assert_eq!(staged(&harness, "sase.md"), Some(PBA.to_string()));
        assert_eq!(
            staged(&harness, "day.md"),
            Some("# Day\nno pomodoros here\n".to_string())
        );
    }

    // ---- Batch phase (NP13–NP20) ----

    struct BatchVault {
        _temp: tempfile::TempDir,
        _guard: TestEnvGuard,
        request: CaptureRequest,
    }

    fn batch_vault(
        files: &[(&str, &str)],
        day: Option<&str>,
        raw_text: &str,
    ) -> BatchVault {
        let temp = tempfile::tempdir().expect("create batch vault");
        let vault = temp.path().join("vault");
        fs::create_dir_all(&vault).expect("create vault");
        for (name, contents) in files {
            let path = vault.join(name);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).expect("create parent");
            }
            fs::write(&path, contents).expect("write fixture note");
        }
        let day_file = vault.join("day.md");
        if let Some(day) = day {
            fs::write(&day_file, day).expect("write day file");
        }
        let tasks_settings =
            vault.join(".obsidian/plugins/obsidian-tasks-plugin");
        fs::create_dir_all(&tasks_settings).expect("create Tasks settings");
        fs::write(
            tasks_settings.join("data.json"),
            r##"{
          "globalFilter": "#task",
          "taskFormat": "dataview",
          "statusSettings": {
            "coreStatuses": [
              {"symbol":" ","name":"Ready","nextStatusSymbol":"x","availableAsCommand":true,"type":"TODO"},
              {"symbol":"x","name":"Done","nextStatusSymbol":" ","availableAsCommand":true,"type":"DONE"}
            ],
            "customStatuses": [
              {"symbol":"/","name":"In Progress","nextStatusSymbol":"x","availableAsCommand":true,"type":"IN_PROGRESS"},
              {"symbol":"*","name":"Next","nextStatusSymbol":"x","availableAsCommand":true,"type":"ON_HOLD"},
              {"symbol":"?","name":"Blocked","nextStatusSymbol":" ","availableAsCommand":true,"type":"ON_HOLD"},
              {"symbol":"-","name":"Canceled","nextStatusSymbol":" ","availableAsCommand":true,"type":"CANCELLED"}
            ]
          }
        }"##,
        )
        .expect("write Tasks settings");
        let missing_config = temp.path().join("missing-bob-config.toml");
        let guard = TestEnvGuard::set(&[
            ("BOB_NOW", Some(std::ffi::OsStr::new("2026-09-20 14:31:07"))),
            ("BOB_DAY_FILE", Some(day_file.as_os_str())),
            ("BOB_CONFIG_FILE", Some(missing_config.as_os_str())),
            ("TZ", Some(std::ffi::OsStr::new("UTC0"))),
        ]);
        let request = CaptureRequest {
            bob_dir: vault,
            dry_run: true,
            forced_clip: None,
            forced_destination_flags: Vec::new(),
            forced_route: None,
            forced_section: None,
            forced_sub_bullet_target: None,
            forced_drawer: None,
            inbox_routes: Vec::new(),
            no_clip: false,
            no_ref: false,
            pull_in: None,
            refuse_override: false,
            require_inbox_route_choice: false,
            require_pull_in_choice: false,
            template_origin: None,
            raw_text: raw_text.to_string(),
            selection: SuccessorSelection::None,
        };
        BatchVault {
            _temp: temp,
            _guard: guard,
            request,
        }
    }

    fn batch_offers(
        batch: &crate::native::capture::plan::PlannedCaptureBatch,
    ) -> Vec<crate::native::capture::output::TaskCompleteUnblockedJson> {
        batch
            .items
            .iter()
            .flat_map(|item| {
                crate::native::capture::output::all_offer_rows(&item.result)
                    .cloned()
                    .collect::<Vec<_>>()
            })
            .collect()
    }

    // NP20: two pulls in item order keep item precedence even when route
    // names sort reverse-lexically (`sase` first, `dev` second, but
    // `dev.md` < `sase.md` lexically). Numbers continue across the draft.
    #[test]
    fn np20_two_pulls_keep_item_order_and_continue_numbers() {
        let mut vault = batch_vault(
            &[
                ("sase.md", "- [ ] #task Sase row ^sr\n"),
                ("dev.md", "- [ ] #task Dev row ^dr\n"),
            ],
            Some(FIX_DAY),
            "@sase\n\n@dev",
        );
        let (batch, _) = crate::native::capture::pull_in::plan_with_pull_in(
            &mut vault.request,
            None,
        )
        .expect("NP20 plans");
        assert_eq!(batch.items.len(), 2);
        let offers = batch_offers(&batch);
        assert_eq!(offers.len(), 2, "{offers:?}");
        // Item order wins: sase (item 0) before dev (item 1).
        assert_eq!(offers[0].note_path, "sase.md");
        assert_eq!(offers[1].note_path, "dev.md");
        let ns: Vec<usize> = offers
            .iter()
            .filter_map(|row| row.offer.as_ref().map(|offer| offer.n))
            .collect();
        assert_eq!(ns, vec![1, 2], "{offers:?}");
    }

    // NP19: a hand-off recovery owns its row; a later pull omits the
    // duplicate instead of offering it twice.
    #[test]
    fn np19_hand_off_owns_row_later_pull_omits() {
        let mut vault = batch_vault(
            &[(
                "sase.md",
                "- [ ] #task Dependent [dependsOn:: open1] [id:: dep] ^dep\n- [ ] #task Opener [id:: open1] ^open1\n",
            )],
            Some(FIX_DAY),
            "!sase:open1\n\n@sase",
        );
        // Select-all would link the hand-off and the pull; use no picks
        // to inspect ownership without writes.
        let (batch, _) = crate::native::capture::pull_in::plan_with_pull_in(
            &mut vault.request,
            None,
        )
        .expect("NP19 plans");
        let offers = batch_offers(&batch);
        let dep_rows: Vec<
            &crate::native::capture::output::TaskCompleteUnblockedJson,
        > = offers.iter().filter(|row| row.block_id == "dep").collect();
        // D appears once (in hand_off); the pull omits it.
        assert_eq!(dep_rows.len(), 1, "{offers:?}");
        assert_ne!(dep_rows[0].trigger, "note_pull");
    }

    // NP17: an earlier creation makes its task offerable; picking it
    // does not stamp fresh.
    #[test]
    fn np17_same_draft_creation_offered_without_fresh_stamp() {
        let mut vault = batch_vault(
            &[("sase.md", "- [ ] #task Old ^old\n")],
            Some(FIX_DAY),
            "Write docs @sase\n\n@sase",
        );
        vault.request.selection = SuccessorSelection::All;
        let batch = crate::native::capture::plan::plan_capture_batch(
            &vault.request,
            None,
        )
        .expect("NP17 plans");
        assert_eq!(batch.items.len(), 2);
        let pull_item = &batch.items[1];
        let pull = pull_item
            .result
            .note_pull
            .as_ref()
            .expect("second item is note pull");
        // Old plus newly created rows are both offered.
        assert!(
            pull.successors.len() >= 2,
            "expected old + created offers: {}",
            pull.successors.len()
        );
        let staged_source = batch
            .text_files
            .iter()
            .find(|file| file.target.ends_with("sase.md"))
            .map(|file| file.updated_target.clone())
            .unwrap_or_default();
        // The created row was picked but carries no fresh stamp.
        let created_lines: Vec<&str> = staged_source
            .lines()
            .filter(|line| line.contains("Write docs"))
            .collect();
        assert_eq!(created_lines.len(), 1, "{staged_source}");
        assert!(
            !created_lines[0].contains("[fresh::"),
            "created pick must not stamp: {}",
            created_lines[0]
        );
        // The old row was picked and is stamped.
        assert!(
            staged_source.contains("[fresh:: 2026-09-20]"),
            "{staged_source}"
        );
    }

    // NP18: a later explicit link drops the earlier pull's row in the
    // final net pass (SL20).
    #[test]
    fn np18_later_link_drops_earlier_pull_row() {
        let vault = batch_vault(
            &[("sase.md", "- [ ] #task Doc row [id:: docs] ^docs\n")],
            Some(FIX_DAY),
            "@sase\n\n@sase:docs",
        );
        let batch = crate::native::capture::plan::plan_capture_batch(
            &vault.request,
            None,
        )
        .expect("NP18 plans");
        assert_eq!(batch.items.len(), 2);
        let pull_item = &batch.items[0];
        let pull = pull_item
            .result
            .note_pull
            .as_ref()
            .expect("first item is note pull");
        // The later `@sase:docs` link removes A's row from the pull.
        assert!(
            pull.successors.is_empty(),
            "later link should prune the pull row: {:?}",
            pull.successors
        );
    }

    // Duplicate ID-less descriptions keep stable `~k` keys: selecting
    // only the second of two identical rows links just that occurrence.
    #[test]
    fn batch_duplicate_idless_second_row_selects_alone() {
        let source = "- [ ] #task Same words\n- [ ] #task Same words\n";
        let mut vault =
            batch_vault(&[("sase.md", source)], Some(FIX_DAY), "@sase");
        // Probe keys with no picks, then select only the `~2` key.
        let (probe, _) = crate::native::capture::pull_in::plan_with_pull_in(
            &mut vault.request,
            None,
        )
        .expect("probe plans");
        let keys: Vec<String> = batch_offers(&probe)
            .iter()
            .filter_map(|row| row.offer.as_ref().map(|offer| offer.key.clone()))
            .collect();
        assert_eq!(keys.len(), 2, "{keys:?}");
        assert!(
            keys[1].contains('~'),
            "second duplicate needs an ordinal suffix: {keys:?}"
        );
        vault.request.pull_in =
            Some(crate::native::capture::cli::PullInPicks::Tokens {
                numbers: Vec::new(),
                keys: vec![keys[1].clone()],
            });
        let (batch, _) = crate::native::capture::pull_in::plan_with_pull_in(
            &mut vault.request,
            None,
        )
        .expect("key pick plans");
        let offers = batch_offers(&batch);
        assert_eq!(offers.len(), 2);
        let picked: Vec<
            &crate::native::capture::output::TaskCompleteUnblockedJson,
        > = offers
            .iter()
            .filter(|row| {
                row.offer.as_ref().is_some_and(|offer| offer.selected)
            })
            .collect();
        assert_eq!(picked.len(), 1);
        assert_eq!(picked[0].offer.as_ref().expect("offer").key, keys[1]);
        assert!(picked[0].link.is_some());
    }

    // Repeated source pulls: the earliest item owns each key; the later
    // pull succeeds empty (pruned) instead of refusing.
    #[test]
    fn batch_repeated_source_second_pull_pruned_not_refused() {
        let mut vault = batch_vault(
            &[("sase.md", "- [ ] #task Only ^only\n")],
            Some(FIX_DAY),
            "@sase\n\n@sase",
        );
        let (batch, _) = crate::native::capture::pull_in::plan_with_pull_in(
            &mut vault.request,
            None,
        )
        .expect("repeated pulls plan");
        assert_eq!(batch.items.len(), 2);
        let first_count = batch.items[0]
            .result
            .note_pull
            .as_ref()
            .expect("first pull")
            .successors
            .len();
        let second_count = batch.items[1]
            .result
            .note_pull
            .as_ref()
            .expect("second pull")
            .successors
            .len();
        assert_eq!(first_count, 1);
        assert_eq!(second_count, 0);
        assert_eq!(batch_offers(&batch).len(), 1);
    }

    // Selected repeated pulls own via stable keys and post-mint block
    // ids: `-p all`, numeric, and key picks prune the second pull instead
    // of refusing, including ID-less minting and duplicate descriptions.
    // A genuinely empty source made empty by an earlier explicit link
    // still refuses.
    #[test]
    fn batch_repeated_source_selected_ownership_prunes() {
        for (name, source) in [
            ("id", "- [ ] #task Only ^only\n"),
            ("idless", "- [ ] #task Only words\n"),
            (
                "duplicates",
                "- [ ] #task Same words\n- [ ] #task Same words\n",
            ),
        ] {
            // `-p all` links every row in the first pull; the second
            // pull's rows are already planned via the staged day link.
            let mut vault = batch_vault(
                &[("sase.md", source)],
                Some(FIX_DAY),
                "@sase\n\n@sase",
            );
            vault.request.pull_in =
                Some(crate::native::capture::cli::PullInPicks::All);
            let (batch, _) =
                crate::native::capture::pull_in::plan_with_pull_in(
                    &mut vault.request,
                    None,
                )
                .expect("selected repeated pulls plan");
            assert_eq!(batch.items.len(), 2, "{name}");
            let first_picked = batch.items[0]
                .result
                .note_pull
                .as_ref()
                .expect("first pull")
                .successors
                .iter()
                .filter(|row| {
                    row.offer.as_ref().is_some_and(|offer| offer.selected)
                })
                .count();
            let second_count = batch.items[1]
                .result
                .note_pull
                .as_ref()
                .expect("second pull")
                .successors
                .len();
            assert!(first_picked > 0, "{name} first picks");
            assert_eq!(second_count, 0, "{name} second pruned");
            // One offer owner and one link per selected task.
            let links = batch_offers(&batch)
                .iter()
                .filter(|row| row.link.is_some())
                .count();
            assert_eq!(links, first_picked, "{name} links");
        }
        // Numeric probe then key replan keeps stable numbers/keys.
        let mut vault = batch_vault(
            &[("sase.md", "- [ ] #task Only ^only\n")],
            Some(FIX_DAY),
            "@sase\n\n@sase",
        );
        vault.request.pull_in =
            Some(crate::native::capture::cli::PullInPicks::Tokens {
                numbers: vec![1],
                keys: Vec::new(),
            });
        let (batch, _) = crate::native::capture::pull_in::plan_with_pull_in(
            &mut vault.request,
            None,
        )
        .expect("numeric repeated pulls plan");
        assert_eq!(batch.items.len(), 2);
        assert_eq!(batch_offers(&batch).len(), 1);
        // An earlier explicit link that owns no offer key still refuses.
        let mut vault = batch_vault(
            &[("sase.md", "- [ ] #task Only ^only\n")],
            Some(FIX_DAY),
            "@sase:only\n\n@sase",
        );
        vault.request.pull_in =
            Some(crate::native::capture::cli::PullInPicks::All);
        let Err(error) = crate::native::capture::pull_in::plan_with_pull_in(
            &mut vault.request,
            None,
        ) else {
            panic!("explicit-link emptied pull must refuse");
        };
        assert_eq!(error.code.as_deref(), Some("nothing_to_pull_in"));
    }

    // Selected hand-off owns its row; a later pull omits it even when
    // the hand-off was selected.
    #[test]
    fn batch_selected_handoff_owns_row_later_pull_omits() {
        let mut vault = batch_vault(
            &[(
                "sase.md",
                "- [ ] #task Dependent [dependsOn:: open1] [id:: dep] ^dep\n- [ ] #task Opener [id:: open1] ^open1\n",
            )],
            Some(FIX_DAY),
            "!sase:open1\n\n@sase",
        );
        vault.request.pull_in =
            Some(crate::native::capture::cli::PullInPicks::All);
        let (batch, _) = crate::native::capture::pull_in::plan_with_pull_in(
            &mut vault.request,
            None,
        )
        .expect("selected handoff then pull plans");
        let offers = batch_offers(&batch);
        let dep_rows: Vec<_> =
            offers.iter().filter(|row| row.block_id == "dep").collect();
        assert_eq!(dep_rows.len(), 1, "{offers:?}");
        assert_ne!(dep_rows[0].trigger, "note_pull");
    }

    // Rollback: a later pull failure aborts the whole draft; no earlier
    // staged item may commit.
    #[test]
    fn batch_later_missing_note_rolls_back_earlier_creation() {
        let vault = batch_vault(
            &[("sase.md", "- [ ] #task Old ^old\n")],
            Some(FIX_DAY),
            "Write docs @sase\n\n@nosuch",
        );
        let Err(error) = crate::native::capture::plan::plan_capture_batch(
            &vault.request,
            None,
        ) else {
            panic!("missing second source must fail the batch");
        };
        assert_eq!(error.code.as_deref(), Some("pull_note_missing"));
    }

    // NP13: `--require-pull-in-choice` without `-p` refuses
    // `pull_in_choice_required` with `pull_in` and flat offers.
    #[test]
    fn np13_require_choice_without_picks_refuses() {
        let mut vault =
            batch_vault(&[("sase.md", PBA)], Some(FIX_DAY), "@sase");
        vault.request.require_pull_in_choice = true;
        let Err(error) = crate::native::capture::pull_in::plan_with_pull_in(
            &mut vault.request,
            None,
        ) else {
            panic!("NP13 must refuse without picks");
        };
        assert_eq!(error.code.as_deref(), Some("pull_in_choice_required"));
    }

    // NP14: numeric picks, key picks, and the teaching error for
    // out-of-range picks.
    #[test]
    fn np14_numeric_key_and_out_of_range_picks() {
        let mut vault =
            batch_vault(&[("sase.md", PBA)], Some(FIX_DAY), "@sase");
        // Numeric pick `-p 1` selects the Pending row.
        vault.request.pull_in =
            Some(crate::native::capture::cli::PullInPicks::Tokens {
                numbers: vec![1],
                keys: Vec::new(),
            });
        let (batch, _) = crate::native::capture::pull_in::plan_with_pull_in(
            &mut vault.request,
            None,
        )
        .expect("numeric pick plans");
        let picked = batch_offers(&batch)
            .into_iter()
            .filter(|row| {
                row.offer.as_ref().is_some_and(|offer| offer.selected)
            })
            .collect::<Vec<_>>();
        assert_eq!(picked.len(), 1);
        assert_eq!(picked[0].text, "Ship the badge renderer");

        // Key pick selects the same row by key.
        let mut vault =
            batch_vault(&[("sase.md", PBA)], Some(FIX_DAY), "@sase");
        vault.request.pull_in =
            Some(crate::native::capture::cli::PullInPicks::Tokens {
                numbers: Vec::new(),
                keys: vec!["sase.md#^ship-renderer".to_string()],
            });
        let (batch, _) = crate::native::capture::pull_in::plan_with_pull_in(
            &mut vault.request,
            None,
        )
        .expect("key pick plans");
        let picked = batch_offers(&batch)
            .into_iter()
            .filter(|row| {
                row.offer.as_ref().is_some_and(|offer| offer.selected)
            })
            .collect::<Vec<_>>();
        assert_eq!(picked.len(), 1);

        // Out-of-range `-p 9` is the teaching error with no writes.
        let mut vault =
            batch_vault(&[("sase.md", PBA)], Some(FIX_DAY), "@sase");
        vault.request.pull_in =
            Some(crate::native::capture::cli::PullInPicks::Tokens {
                numbers: vec![9],
                keys: Vec::new(),
            });
        let Err(error) = crate::native::capture::pull_in::plan_with_pull_in(
            &mut vault.request,
            None,
        ) else {
            panic!("out-of-range pick must fail");
        };
        assert!(
            error.message.contains("stale, unknown, or out-of-range"),
            "{}",
            error.message
        );
    }

    // NP16: a Ready inbox pick is an inbox-route candidate moved in the
    // same transaction; a picked Next inbox task is not a candidate.
    #[test]
    fn np16_inbox_ready_pick_routes_next_pick_does_not() {
        let inbox_files: &[(&str, &str)] = &[
            ("inbox.md", "# Inbox\n"),
            (
                "mac_inbox.md",
                "---\ntype: [[area]]\nparent: [[inbox]]\n---\n## Tasks\n- [ ] #task Route me ^route-task\n- [*] #task Already Next ^already-next\n",
            ),
            ("health.md", "---\ntype: [[area]]\n---\n## Tasks\n"),
        ];
        // Pick the Ready row only: it becomes Next and is a candidate.
        let BatchVault {
            _temp: _temp1,
            _guard: _guard1,
            mut request,
        } = batch_vault(inbox_files, Some(FIX_DAY), "@mac_inbox");
        request.selection = SuccessorSelection::Keys(
            ["mac_inbox.md#^route-task".to_string()]
                .into_iter()
                .collect(),
        );
        let batch =
            crate::native::capture::plan::plan_capture_batch(&request, None)
                .expect("NP16 Ready pick plans");
        assert_eq!(batch.inbox_route.candidate_count, 1);
        assert_eq!(
            batch.inbox_route.candidates[0].key,
            "mac_inbox.md#^route-task"
        );

        // Routing it moves the task in the same batch.
        let BatchVault {
            _temp: _temp2,
            _guard: _guard2,
            mut request,
        } = batch_vault(inbox_files, Some(FIX_DAY), "@mac_inbox");
        request.selection = SuccessorSelection::Keys(
            ["mac_inbox.md#^route-task".to_string()]
                .into_iter()
                .collect(),
        );
        request.inbox_routes =
            vec![crate::native::capture::cli::InboxRoutePick::Answer {
                selector: "1".to_string(),
                destination: Some("health".to_string()),
            }];
        let batch =
            crate::native::capture::plan::plan_capture_batch(&request, None)
                .expect("NP16 routed pick plans");
        assert_eq!(batch.inbox_route.routed_count, 1);

        // Picking only the already-Next row is no candidate.
        let BatchVault {
            _temp: _temp3,
            _guard: _guard3,
            mut request,
        } = batch_vault(inbox_files, Some(FIX_DAY), "@mac_inbox");
        request.selection = SuccessorSelection::Keys(
            ["mac_inbox.md#^already-next".to_string()]
                .into_iter()
                .collect(),
        );
        let batch =
            crate::native::capture::plan::plan_capture_batch(&request, None)
                .expect("NP16 Next pick plans");
        assert_eq!(batch.inbox_route.candidate_count, 0);
    }
}
