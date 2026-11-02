//! Shared closing-edge collection for capture cancel and complete
//! labels (`docs/task-relations.md` §§17–18): Added/Relabeled edges,
//! ordered grouping by resolved source identity, and source-action
//! conflict analysis. Cancel and complete still run their own writers.

use std::collections::HashSet;
use std::path::Path;

use super::*;
use crate::native::{
    capture_active_tasks,
    config::relations::CloseEffect,
    relation_labels::relation_label_key,
    task_complete::relation_successors::copy::{self, ReleaseState},
    task_dependencies,
};

/// One closing-label edge that names a configured cancel or complete
/// label on an Added or Relabeled relation, in typed order.
pub(super) struct ClosingEdge {
    pub(super) update_index: usize,
    pub(super) typed_index: usize,
    pub(super) label: String,
    pub(super) effect: CloseEffect,
}

/// Nested completions, tracking refs, and day-file effects produced by
/// one item's closing-label commits.
#[derive(Debug, Default)]
pub(super) struct RelationCloseReports {
    pub(super) cancels: Vec<RelationCancelJson>,
    pub(super) completes: Vec<RelationCompleteJson>,
    pub(super) pomodoro_refs: Vec<PomodoroBlockRef>,
    pub(super) task_block_refs: Vec<TaskBlockRef>,
    pub(super) day_file: Option<String>,
}

/// Collect Added/Relabeled edges whose label is in the effective list
/// for `effect`. Unchanged edges, unconfigured keys, and labels dropped
/// by the both-lists rule never appear.
pub(super) fn collect_closing_edges(
    updates: &[RelationUpdateJson],
    configured_keys: &[String],
    effect: CloseEffect,
) -> Vec<ClosingEdge> {
    let mut edges = Vec::new();
    for (update_index, update) in updates.iter().enumerate() {
        if !matches!(
            update.action,
            RelationAction::Added | RelationAction::Relabeled
        ) {
            continue;
        }
        let Some(label) = update.label.clone() else {
            continue;
        };
        if !configured_keys
            .iter()
            .any(|key| *key == relation_label_key(&label))
        {
            continue;
        }
        edges.push(ClosingEdge {
            update_index,
            typed_index: update_index,
            label,
            effect,
        });
    }
    edges
}

/// Group closing edges by resolved source identity, preserving authored
/// order of first appearance and of edges inside each group.
pub(super) fn group_closing_edges(
    updates: &[RelationUpdateJson],
    edges: Vec<ClosingEdge>,
) -> Vec<Vec<ClosingEdge>> {
    let mut groups: Vec<Vec<ClosingEdge>> = Vec::new();
    for edge in edges {
        let key = source_identity(&updates[edge.update_index]);
        match groups.iter_mut().find(|group| {
            source_identity(&updates[group[0].update_index]) == key
        }) {
            Some(group) => group.push(edge),
            None => groups.push(vec![edge]),
        }
    }
    groups
}

/// One closing-label source, by staged identity: same note and block
/// ID when the source has one, falling back to the staged line only for
/// an actually idless source. Mirror staging can shift the line of a
/// stable block within one item, so the line must not split one source.
pub(super) fn source_identity(update: &RelationUpdateJson) -> (String, String) {
    source_registry_key(update)
}

/// The target text behind one triggering label within its own source
/// group: the first group edge carrying that label names it. Scoping to
/// the group keeps mixed-direction items with the same label on both
/// edges from attributing one source's warning to the other's target.
pub(super) fn group_target_for_label<'a>(
    updates: &'a [RelationUpdateJson],
    group: &[ClosingEdge],
    label: &str,
) -> Option<&'a str> {
    group
        .iter()
        .find(|edge| updates[edge.update_index].label.as_deref() == Some(label))
        .map(|edge| updates[edge.update_index].target.text.as_str())
}

/// Draft-wide identity for opposite-effect tracking: resolved note path
/// and block id, falling back to the source line when no id exists.
pub(super) fn source_registry_key(
    update: &RelationUpdateJson,
) -> (String, String) {
    let block = if update.source.block_id.is_empty() {
        format!("line:{}", update.source.line)
    } else {
        update.source.block_id.clone()
    };
    (update.source.path.clone(), block)
}

/// Markers on this item that also act on the relation source
/// (`docs/task-relations.md` §17 CL5 / §18 CP5): `&` / `*` on an owner
/// source (`~`), or a `*` naming a picked source (`~~`). An action on
/// a `~~` new target is not an action on the picked source. `@note+id`
/// existing-owner syntax is the relation itself, never a conflict.
pub(super) fn item_action_markers(
    ctx: &DependencyContext,
    planner: &mut CaptureBatchPlanner,
    bob_dir: &Path,
    parsed: &ParsedCaptureText,
    update: &RelationUpdateJson,
) -> Vec<&'static str> {
    match update.direction {
        RelationDirection::To => {
            let mut markers = Vec::new();
            if !parsed.dependencies.is_empty() {
                markers.push("&");
            }
            if !parsed.blocked_tasks.is_empty() {
                markers.push("*");
            }
            markers
        }
        RelationDirection::From => {
            let names_source = parsed.blocked_tasks.iter().any(|star| {
                star.block_id == update.source.block_id
                    && !update.source.block_id.is_empty()
                    && resolve_prerequisite_note(
                        ctx, planner, bob_dir, &star.note,
                    )
                    .is_ok_and(|relative| {
                        display_relative(&relative) == update.source.path
                    })
            });
            if names_source {
                vec!["*"]
            } else {
                Vec::new()
            }
        }
    }
}

/// The 1-based source line after staging, located by block id so a
/// composed write still reports where S sits.
pub(super) fn locate_final_line(
    staged: &str,
    ctx: &DependencyContext,
    display: &str,
    block_id: Option<&str>,
    fallback: usize,
) -> usize {
    let scan = note_tasks::scan(staged, ctx.settings());
    if let Some(block_id) = block_id
        && !block_id.is_empty()
        && let Ok(task) = lookup_staged_task(&scan, display, block_id)
    {
        return task.line_index + 1;
    }
    fallback
}

/// Open tasks left behind by a closed `^prj`: every open-status task
/// in the project note except the closed source line itself.
pub(super) fn count_open_tasks(
    staged: &str,
    ctx: &DependencyContext,
    source_line: usize,
) -> usize {
    note_tasks::scan(staged, ctx.settings())
        .tasks()
        .iter()
        .filter(|task| {
            task.line_index + 1 != source_line
                && matches!(task.status_symbol, ' ' | '*' | '/' | '?')
        })
        .count()
}

/// One item's staged `~` / `~~` relation writes together with the
/// closing-label effects they trigger. The ordinary item path and the
/// multi-note path both stage relations through this one sequence, so a
/// configured complete or cancel label never works for one and silently
/// stays open for the other.
pub(super) struct ItemRelationReports {
    pub(super) writes: PlannedRelationReports,
    pub(super) close: RelationCloseReports,
}

/// Stage the item's relation writes, then its cancel- and complete-label
/// commits against the same overlays. `writes.updates` carries the
/// per-edge `cancel_effect` / `complete_effect` marks on return.
pub(super) fn plan_item_relations(
    ctx: &mut DependencyContext,
    planner: &mut CaptureBatchPlanner,
    request: &CaptureRequest,
    parsed: &ParsedCaptureText,
    owner: &RelationOwner,
    existing_owner: bool,
    now: chrono::NaiveDateTime,
    warnings: &mut Vec<CaptureWarning>,
) -> Result<ItemRelationReports, CaptureError> {
    let mut writes = plan_relation_writes(
        ctx,
        planner,
        &request.bob_dir,
        owner,
        &parsed.relations,
        warnings,
        existing_owner,
    )?;
    let close = plan_relation_close_effects(
        ctx,
        planner,
        request,
        parsed,
        &mut writes.updates,
        existing_owner,
        now,
        warnings,
    )?;
    Ok(ItemRelationReports { writes, close })
}

/// Plan cancel-label and complete-label commits for one item's already
/// staged relation writes. Prefights opposite effects on the same
/// source (this item and earlier items in the draft) before either
/// writer runs, so a later staged closed status cannot mask a
/// cancel/complete contradiction as `already_closed`.
#[allow(clippy::too_many_arguments)]
pub(super) fn plan_relation_close_effects(
    ctx: &mut DependencyContext,
    planner: &mut CaptureBatchPlanner,
    request: &CaptureRequest,
    parsed: &ParsedCaptureText,
    updates: &mut [RelationUpdateJson],
    existing_owner: bool,
    now: chrono::NaiveDateTime,
    warnings: &mut Vec<CaptureWarning>,
) -> Result<RelationCloseReports, CaptureError> {
    // Load both lists once so overlap and complete-label warnings share
    // the established once-per-batch path with cancel diagnostics.
    let _ = ctx.complete_labels();
    let cancel_keys: Vec<String> = ctx
        .cancel_labels()
        .iter()
        .map(|label| label.key.clone())
        .collect();
    let complete_keys: Vec<String> = ctx
        .complete_labels()
        .iter()
        .map(|label| label.key.clone())
        .collect();
    let cancel_edges =
        collect_closing_edges(updates, &cancel_keys, CloseEffect::Cancel);
    let complete_edges =
        collect_closing_edges(updates, &complete_keys, CloseEffect::Complete);
    refuse_opposite_close_effects(
        ctx,
        updates,
        &cancel_edges,
        &complete_edges,
    )?;
    let cancels = plan_relation_cancels(
        ctx,
        planner,
        &request.bob_dir,
        parsed,
        updates,
        existing_owner,
        request.dry_run,
        warnings,
    )?;
    let complete = plan_relation_completes(
        ctx,
        planner,
        request,
        parsed,
        updates,
        existing_owner,
        now,
        warnings,
    )?;
    plan_relation_successor_indications(
        ctx,
        planner,
        &request.bob_dir,
        updates,
        warnings,
    );
    Ok(RelationCloseReports {
        cancels,
        completes: complete.completes,
        pomodoro_refs: complete.pomodoro_refs,
        task_block_refs: complete.task_block_refs,
        day_file: complete.day_file,
    })
}

/// Attach the configured successor-label consequence to each newly written
/// edge. This read-only indication follows staged task and ledger text and
/// never refuses an otherwise valid capture.
fn plan_relation_successor_indications(
    ctx: &DependencyContext,
    planner: &mut CaptureBatchPlanner,
    bob_dir: &Path,
    updates: &mut [RelationUpdateJson],
    warnings: &mut Vec<CaptureWarning>,
) {
    let configured: Vec<String> = ctx
        .successor_labels()
        .iter()
        .map(|label| label.key.clone())
        .collect();
    if configured.is_empty() {
        return;
    }
    for update in updates.iter_mut() {
        if !matches!(
            update.action,
            RelationAction::Added | RelationAction::Relabeled
        ) {
            continue;
        }
        let Some(label) = update.label.as_deref() else {
            continue;
        };
        if !configured
            .iter()
            .any(|key| *key == relation_label_key(label))
        {
            continue;
        }
        let Some(effect) =
            relation_target_offer_effect(ctx, planner, bob_dir, update)
        else {
            continue;
        };
        update.offer_effect = Some(effect);
        let message = match effect {
            RelationOfferEffect::Offers => continue,
            RelationOfferEffect::AlreadyParked => {
                copy::already_released_warning(
                    label,
                    &update.source.text,
                    &update.target.text,
                )
                .message
            }
            RelationOfferEffect::AlreadyClosed => copy::release_caveat(
                ReleaseState::Closed,
                &update.target.text,
                None,
            ),
        };
        warnings.push(CaptureWarning::Structured(StructuredCaptureWarning {
            code: "relation_successor_already_released",
            label: Some(label.to_string()),
            count: None,
            message,
        }));
    }
}

fn relation_target_offer_effect(
    ctx: &DependencyContext,
    planner: &mut CaptureBatchPlanner,
    bob_dir: &Path,
    update: &RelationUpdateJson,
) -> Option<RelationOfferEffect> {
    let relative = PathBuf::from(&update.target.path);
    if relative.is_absolute()
        || relative
            .components()
            .any(|part| matches!(part, std::path::Component::ParentDir))
    {
        return None;
    }
    let block_id = update.target.block_id.as_str();
    if block_id.is_empty() {
        return None;
    }
    if task_dependencies::is_archive_path(&relative) {
        return Some(RelationOfferEffect::AlreadyClosed);
    }
    let target_contents =
        current_note_text(planner, &bob_dir.join(&relative)).ok()?;
    let scan = note_tasks::scan(&target_contents, ctx.settings());
    let target =
        lookup_staged_task(&scan, &update.target.path, block_id).ok()?;
    if target.status_type.is_terminal() {
        return Some(RelationOfferEffect::AlreadyClosed);
    }
    if target.status_symbol != '/' {
        return Some(RelationOfferEffect::Offers);
    }
    let day_file = pomodoro::day_file_for(bob_dir);
    let day_contents = current_note_text(planner, &day_file).ok()?;
    let ledger = capture_active_tasks::ledger_from_contents(&day_contents);
    let mut route = relative;
    route.set_extension("");
    let route = route.to_string_lossy().replace('\\', "/");
    ledger
        .position(&(route, block_id.to_string()))
        .is_none()
        .then_some(RelationOfferEffect::AlreadyParked)
        .or(Some(RelationOfferEffect::Offers))
}

fn refuse_opposite_close_effects(
    ctx: &DependencyContext,
    updates: &[RelationUpdateJson],
    cancel_edges: &[ClosingEdge],
    complete_edges: &[ClosingEdge],
) -> Result<(), CaptureError> {
    if cancel_edges.is_empty() && complete_edges.is_empty() {
        return Ok(());
    }
    let cancel_keys: HashSet<(String, String)> = cancel_edges
        .iter()
        .map(|edge| source_registry_key(&updates[edge.update_index]))
        .collect();
    let complete_keys: HashSet<(String, String)> = complete_edges
        .iter()
        .map(|edge| source_registry_key(&updates[edge.update_index]))
        .collect();
    for edge in complete_edges.iter().chain(cancel_edges.iter()) {
        let key = source_registry_key(&updates[edge.update_index]);
        let both_this_item =
            cancel_keys.contains(&key) && complete_keys.contains(&key);
        let prior = ctx.close_intent(&key.0, &key.1);
        let opposite = match edge.effect {
            CloseEffect::Complete => {
                both_this_item || prior == Some(CloseEffect::Cancel)
            }
            CloseEffect::Cancel => prior == Some(CloseEffect::Complete),
        };
        if !opposite {
            continue;
        }
        let source = &updates[edge.update_index].source.text;
        return Err(CaptureError::usage_code(
            format!(
                "“{source}” can't be both cancelled and completed in one commit"
            ),
            "relation_close_effects_conflict",
        ));
    }
    Ok(())
}
