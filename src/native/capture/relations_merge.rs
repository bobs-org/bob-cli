//! Capture `~` / `~~` merge-label commits through the shared task-merge
//! engine (`docs/task-relations.md` §21).

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use super::*;
use crate::native::{
    capture_language::{CaptureKind, RelationDirection},
    collect_done::link_repair_markdown_files,
    config::relations::CloseEffect,
    task_complete::is_recurring_task_line,
    task_locator::TaskLocator,
    task_merge::{
        plan_task_merge, DailyNoteReadState, MergeContainer, MergeContext,
        MergeGesture, MergeOffer, MergeOutcome, MergeRecord,
        MergeRelationPolicy, MergeRequest,
    },
    task_move::VaultSnapshot,
    vault_links::NoteIndex,
};

#[derive(Default)]
pub(super) struct RelationMergeReports {
    pub(super) records: Vec<MergeRecord>,
    pub(super) offer_rows: Vec<TaskCompleteUnblockedJson>,
}

/// Run configured merge-label effects against the same staged vault
/// snapshot as relation writes and earlier capture items.
#[allow(clippy::too_many_arguments)]
pub(super) fn plan_relation_merges(
    ctx: &mut DependencyContext,
    planner: &mut CaptureBatchPlanner,
    request: &CaptureRequest,
    parsed: &ParsedCaptureText,
    updates: &mut [RelationUpdateJson],
    existing_owner: bool,
    edges: Vec<super::relations_close::ClosingEdge>,
    warnings: &mut Vec<CaptureWarning>,
) -> Result<RelationMergeReports, CaptureError> {
    if edges.is_empty() {
        return Ok(RelationMergeReports::default());
    }
    let groups = super::relations_close::group_closing_edges(updates, edges);
    let mut snapshot = load_staged_snapshot(planner, &request.bob_dir)?;
    let settings = ctx.settings().clone();
    let note_index = NoteIndex::from_paths(snapshot.notes.keys().cloned());
    let drawers = capture_drawers::load_drawer_catalog(&request.bob_dir)
        .unwrap_or_default();
    let daily_path_abs = pomodoro::day_file_for(&request.bob_dir);
    let daily_path = daily_path_abs
        .strip_prefix(&request.bob_dir)
        .map_err(|error| {
            CaptureError::io(format!(
                "daily note {} is outside the vault: {error}",
                daily_path_abs.display()
            ))
        })?
        .to_path_buf();
    let relation_policy = MergeRelationPolicy {
        linked_from_lines: ctx.linked_from_lines_enabled(),
        linked_from_mentions: ctx.linked_from_mentions_enabled(),
    };
    let mut reports = RelationMergeReports::default();

    for group in groups {
        let first = &updates[group[0].update_index];
        if first.direction == RelationDirection::To && !existing_owner {
            return Err(CaptureError::usage_code(
                format!(
                    "“{}” is new and cannot be merged as the source; use ~~ to merge an existing task into it",
                    first.source.text
                ),
                "relation_merges_new_task",
            ));
        }
        let markers = merge_conflict_markers(
            ctx,
            planner,
            &request.bob_dir,
            parsed,
            first,
        );
        if !markers.is_empty() {
            return Err(CaptureError::usage_code(
                format!(
                    "cannot merge “{}” while this capture also uses {}; capture the merge-label link alone",
                    first.source.text,
                    markers.join(" and ")
                ),
                "relation_merge_conflicts_with_item_action",
            ));
        }

        let target_identity = (&first.target.path, &first.target.block_id);
        if group.iter().any(|edge| {
            let update = &updates[edge.update_index];
            update.target.path != *target_identity.0
                || update.target.block_id != *target_identity.1
        }) {
            return Err(CaptureError::usage_code(
                format!(
                    "“{}” has merge labels to more than one target in this capture",
                    first.source.text
                ),
                "relation_close_effects_conflict",
            ));
        }

        let source_locator = endpoint_locator(&first.source)?;
        let target_locator = endpoint_locator(&first.target)?;
        if source_is_recurring(
            &snapshot,
            &first.source.path,
            &first.source.block_id,
            &settings,
        ) {
            return Err(CaptureError::usage_code(
                format!(
                    "^{} is recurring and cannot be merged by a relation label",
                    source_locator.block_id
                ),
                "relation_merges_recurring_task",
            ));
        }
        let label = group[0].label.clone();
        let merge_request = MergeRequest {
            sources: vec![source_locator],
            target: MergeContainer::Task(target_locator),
            label,
            gesture: MergeGesture::Label,
            selection: request.selection.clone(),
        };
        let day_read = snapshot
            .notes
            .get(&daily_path)
            .cloned()
            .map(DailyNoteReadState::Read)
            .unwrap_or(DailyNoteReadState::Missing);
        let merge_context = MergeContext {
            today: ctx.today,
            daily_path: daily_path.clone(),
            daily_read: day_read,
            task_settings: &settings,
            note_index: &note_index,
            drawer_catalog: &drawers,
            relation_policy,
            successor_labels: ctx.successor_labels(),
        };
        let before = snapshot.clone();
        let mut plan =
            plan_task_merge(&snapshot, &merge_request, &merge_context)
                .map_err(|error| {
                    CaptureError::usage_code(error.message, error.code)
                })?;
        let edge_was_written = group.iter().any(|edge| {
            matches!(
                updates[edge.update_index].action,
                RelationAction::Added | RelationAction::Relabeled
            )
        });
        if edge_was_written {
            for record in &mut plan.records {
                if record.outcome == MergeOutcome::Unchanged
                    && record.previous_status.is_some_and(|status| {
                        !matches!(status, ' ' | '*' | '/' | '?')
                    })
                {
                    record.outcome = MergeOutcome::AlreadyClosed;
                    let target_title = record
                        .target
                        .title
                        .as_deref()
                        .unwrap_or("the other task");
                    record.message = format!(
                        "Recorded merge relation to “{target_title}”; source was already closed"
                    );
                }
            }
        }
        for warning in &plan.warnings {
            warnings.push(CaptureWarning::Text(warning.clone()));
        }
        stage_merge_plan(planner, &request.bob_dir, &before, &plan)?;
        snapshot.notes.extend(plan.postimages.clone());
        for record in &plan.records {
            let effect = match record.outcome {
                MergeOutcome::Merged => RelationMergeEffect::Merged,
                MergeOutcome::Unchanged | MergeOutcome::AlreadyClosed => {
                    RelationMergeEffect::AlreadyClosed
                }
            };
            for edge in &group {
                updates[edge.update_index].merge_effect = Some(effect);
            }
            let source_key = super::relations_close::source_registry_key(
                &updates[group[0].update_index],
            );
            ctx.record_close_intent(
                source_key.0,
                source_key.1,
                CloseEffect::Merge,
            );
            reports.offer_rows.extend(offer_rows_for_record(
                record,
                &before,
                &snapshot,
                &settings,
                &daily_path,
            ));
        }
        reports.records.extend(plan.records);
    }
    Ok(reports)
}

fn endpoint_locator(
    endpoint: &RelationEndpointJson,
) -> Result<TaskLocator, CaptureError> {
    let note = endpoint.path.strip_suffix(".md").unwrap_or(&endpoint.path);
    TaskLocator::parse(&format!("{note}:{}", endpoint.block_id)).ok_or_else(
        || {
            CaptureError::usage_code(
                format!("{} is not a mergeable task locator", endpoint.path),
                "task_not_found",
            )
        },
    )
}

fn source_is_recurring(
    snapshot: &VaultSnapshot,
    path: &str,
    block_id: &str,
    settings: &note_tasks::NoteTaskSettings,
) -> bool {
    let path = PathBuf::from(path);
    let Some(contents) = snapshot.notes.get(&path) else {
        return false;
    };
    let scan = note_tasks::scan(contents, settings);
    let Some(task) = (match scan.by_block_id(block_id) {
        note_tasks::BlockIdLookup::Found(task) => Some(task),
        note_tasks::BlockIdLookup::Missing
        | note_tasks::BlockIdLookup::Duplicate(_)
        | note_tasks::BlockIdLookup::NotATask { .. } => None,
    }) else {
        return false;
    };
    contents
        .lines()
        .nth(task.line_index)
        .is_some_and(is_recurring_task_line)
}

pub(super) fn load_staged_snapshot(
    planner: &mut CaptureBatchPlanner,
    bob_dir: &Path,
) -> Result<VaultSnapshot, CaptureError> {
    let files = link_repair_markdown_files(bob_dir).map_err(|error| {
        CaptureError::io(format!("scan vault for merge planning: {error}"))
    })?;
    planner.guard_merge_scan(files.clone())?;
    let overlays = planner.staged_snapshot();
    let mut notes = BTreeMap::new();
    for path in files {
        let relative = path.strip_prefix(bob_dir).map_err(|error| {
            CaptureError::io(format!(
                "{} is outside the vault: {error}",
                path.display()
            ))
        })?;
        let contents = match overlays.get(&path) {
            Some(Some(contents)) => contents.clone(),
            Some(None) => continue,
            None => std::fs::read_to_string(&path).map_err(|error| {
                CaptureError::io(format!("read {}: {error}", path.display()))
            })?,
        };
        planner.guard_merge_read(&path, Some(contents.clone()));
        notes.insert(relative.to_path_buf(), contents);
    }
    for (path, contents) in overlays {
        let Some(contents) = contents else { continue };
        let relative = path.strip_prefix(bob_dir).map_err(|error| {
            CaptureError::io(format!(
                "{} is outside the vault: {error}",
                path.display()
            ))
        })?;
        planner.guard_merge_read(&path, Some(contents.clone()));
        notes.insert(relative.to_path_buf(), contents);
    }
    Ok(VaultSnapshot { notes })
}

fn merge_conflict_markers(
    ctx: &DependencyContext,
    planner: &mut CaptureBatchPlanner,
    bob_dir: &Path,
    parsed: &ParsedCaptureText,
    update: &RelationUpdateJson,
) -> Vec<&'static str> {
    let mut markers = super::relations_close::item_action_markers(
        ctx, planner, bob_dir, parsed, update,
    );
    let starts = matches!(
        &parsed.kind,
        CaptureKind::PomodoroStart { .. }
            | CaptureKind::Pomodoro { start: Some(_), .. }
            | CaptureKind::PomodoroLink { start: Some(_), .. }
    );
    if starts {
        markers.push("a Pomodoro start");
    }
    if matches!(
        &parsed.kind,
        CaptureKind::PomodoroLink { .. } | CaptureKind::TaskComplete { .. }
    ) {
        markers.push("a task completion or Pomodoro link");
    }
    markers
}

pub(super) fn offer_rows_for_record(
    record: &MergeRecord,
    before: &VaultSnapshot,
    after: &VaultSnapshot,
    settings: &note_tasks::NoteTaskSettings,
    daily_path: &Path,
) -> Vec<TaskCompleteUnblockedJson> {
    let target_path = &record.target.path;
    let Some(before_text) = before.notes.get(target_path) else {
        return Vec::new();
    };
    let Some(after_text) = after.notes.get(target_path) else {
        return Vec::new();
    };
    let before_scan = note_tasks::scan(before_text, settings);
    let after_scan = note_tasks::scan(after_text, settings);
    let Some(target_id) = record.target.block_id.as_deref() else {
        return Vec::new();
    };
    let note_tasks::BlockIdLookup::Found(previous) =
        before_scan.by_block_id(target_id)
    else {
        return Vec::new();
    };
    let note_tasks::BlockIdLookup::Found(current) =
        after_scan.by_block_id(target_id)
    else {
        return Vec::new();
    };
    record
        .offers
        .iter()
        .map(|offer| {
            merge_offer_row(record, offer, previous, current, daily_path)
        })
        .collect()
}

fn merge_offer_row(
    record: &MergeRecord,
    offer: &MergeOffer,
    previous: &note_tasks::NoteTask,
    current: &note_tasks::NoteTask,
    daily_path: &Path,
) -> TaskCompleteUnblockedJson {
    let target_path = display_relative(&record.target.path);
    let source_path = display_relative(&record.source.path);
    let source_id = record.source.block_id.clone();
    let destination_role = match offer.entry_role.as_str() {
        "slot" => "slot",
        "created" => "created",
        "next_up" => "next_up",
        _ => "current",
    };
    TaskCompleteUnblockedJson {
        note_path: target_path.clone(),
        block_id: record.target.block_id.clone().unwrap_or_default(),
        line: current.line_index + 1,
        text: current.description.clone(),
        previous_status_symbol: previous.status_symbol,
        previous_status_name: previous.status_name.clone(),
        status_symbol: current.status_symbol,
        status_name: current.status_name.clone(),
        inbox: false,
        unblocked_by: vec![TaskCompleteUnblockedCauseJson {
            note_path: source_path.clone(),
            block_id: source_id.clone(),
            text: record.source.title.clone(),
        }],
        link: None,
        not_linked: (!offer.selected).then(|| "not_selected".to_string()),
        trigger: "merge",
        hide_removed: false,
        offer: Some(TaskCompleteOfferJson {
            key: offer.key.clone(),
            n: offer.n,
            group: "hand_off",
            selected: offer.selected,
            block_id_needed: false,
            project: None,
            after: None,
            moved_from: None,
            merged_from: Some(offer.merged_from.clone().unwrap_or(source_path)),
            destination: TaskCompleteOfferDestinationJson {
                day_file: display_relative(daily_path),
                entry_name: offer.entry_name.clone(),
                entry_role: destination_role,
                fallback: false,
            },
            order_key: (
                0,
                (0, 0),
                0,
                record.target.path.clone(),
                current.line_index,
            ),
            settled_ordinal: None,
        }),
        relation: None,
    }
}

/// Preserve engine read dependencies and target/dependents/repairs/day/source order.
pub(super) fn stage_merge_plan(
    planner: &mut CaptureBatchPlanner,
    bob_dir: &Path,
    before: &VaultSnapshot,
    plan: &crate::native::task_merge::MergePlan,
) -> Result<(), CaptureError> {
    for path in &plan.read_paths {
        planner.guard_merge_read(
            &bob_dir.join(path),
            before.notes.get(path).cloned(),
        );
    }
    for path in &plan.write_order {
        planner.stage_snapshot_postimage(
            &bob_dir.join(path),
            before.notes.get(path).cloned(),
            plan.postimages[path].clone(),
        )?;
    }
    for pair in plan.write_order.windows(2) {
        planner.record_move(&bob_dir.join(&pair[1]), &bob_dir.join(&pair[0]));
    }
    Ok(())
}
