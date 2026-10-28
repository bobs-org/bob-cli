//! Capture `~` / `~~` complete-label commits: compose the shared Done
//! path with staged relation writes (`docs/task-relations.md` §18).

use std::path::{Path, PathBuf};

use super::*;
use crate::native::{
    config::relations::CloseEffect,
    task_complete::verdicts::{complete_verdict, CompleteSourceVerdict},
};

/// Nested completions produced by one item's complete-label commits.
#[derive(Debug, Default)]
pub(super) struct RelationCompleteReports {
    pub(super) completes: Vec<RelationCompleteJson>,
    pub(super) pomodoro_refs: Vec<PomodoroBlockRef>,
    pub(super) task_block_refs: Vec<TaskBlockRef>,
    pub(super) day_file: Option<String>,
}

/// Plan the complete-label commits for one item's already-staged
/// relation writes. Collects Added/Relabeled edges whose label is
/// configured, refuses the new-task, recurring, and conflict cases with
/// coded usage errors, and stages each completed source through
/// [`complete_existing_task`]. Already-closed sources keep the link
/// only. With no complete labels configured this is a no-op, so older
/// output stays byte-identical.
#[allow(clippy::too_many_arguments)]
pub(super) fn plan_relation_completes(
    ctx: &mut DependencyContext,
    planner: &mut CaptureBatchPlanner,
    request: &CaptureRequest,
    parsed: &ParsedCaptureText,
    updates: &mut [RelationUpdateJson],
    existing_owner: bool,
    warnings: &mut Vec<CaptureWarning>,
) -> Result<RelationCompleteReports, CaptureError> {
    let configured: Vec<String> = ctx
        .complete_labels()
        .iter()
        .map(|label| label.key.clone())
        .collect();
    if configured.is_empty() {
        return Ok(RelationCompleteReports::default());
    }
    let edges =
        collect_closing_edges(updates, &configured, CloseEffect::Complete);
    if edges.is_empty() {
        return Ok(RelationCompleteReports::default());
    }
    refuse_complete_conflicts(
        ctx,
        planner,
        &request.bob_dir,
        parsed,
        updates,
        &edges,
    )?;
    let groups = group_closing_edges(updates, edges);
    let mut reports = RelationCompleteReports::default();
    for group in &groups {
        if let Some(complete) = plan_source_complete(
            ctx,
            planner,
            request,
            parsed,
            updates,
            group,
            existing_owner,
        )? {
            if complete.effect == RelationCompleteEffect::Completed {
                reports.pomodoro_refs.extend(
                    complete.task_complete_refs.pomodoro_refs.iter().cloned(),
                );
                reports.task_block_refs.extend(
                    complete.task_complete_refs.task_block_refs.iter().cloned(),
                );
                if reports.day_file.is_none() {
                    reports.day_file =
                        complete.task_complete_refs.day_file.clone();
                }
            }
            reports.completes.push(complete.row);
        }
    }
    for (group, complete) in groups.iter().zip(reports.completes.iter()) {
        if complete.effect == RelationCompleteEffect::AlreadyClosed {
            continue;
        }
        let source_text = &complete.source.text;
        let target_text = &complete.target.text;
        for label in &complete.labels {
            let target_text = group_target_for_label(updates, group, label)
                .unwrap_or(target_text);
            warnings.push(CaptureWarning::Structured(
                StructuredCaptureWarning {
                    code: "relation_completed_source",
                    label: Some(label.clone()),
                    count: None,
                    message: if request.dry_run {
                        format!(
                            "✓ would complete “{source_text}” ({label} “{target_text}”)"
                        )
                    } else {
                        format!(
                            "✓ completed “{source_text}” ({label} “{target_text}”)"
                        )
                    },
                },
            ));
        }
    }
    Ok(reports)
}

/// Tracking refs from one completed source, held beside the JSON row
/// until the parent item merges them.
#[derive(Debug, Clone, Default)]
struct CompleteTracking {
    pomodoro_refs: Vec<PomodoroBlockRef>,
    task_block_refs: Vec<TaskBlockRef>,
    day_file: Option<String>,
}

struct PlannedSourceComplete {
    row: RelationCompleteJson,
    effect: RelationCompleteEffect,
    task_complete_refs: CompleteTracking,
}

/// Refuse an item that also changes the complete source through another
/// marker (`docs/task-relations.md` §18 CP5). Capture is all-or-nothing,
/// so the refusal writes nothing.
fn refuse_complete_conflicts(
    ctx: &DependencyContext,
    planner: &mut CaptureBatchPlanner,
    bob_dir: &Path,
    parsed: &ParsedCaptureText,
    updates: &[RelationUpdateJson],
    edges: &[ClosingEdge],
) -> Result<(), CaptureError> {
    for edge in edges {
        let update = &updates[edge.update_index];
        let typed = &parsed.relations[edge.typed_index];
        let markers =
            item_action_markers(ctx, planner, bob_dir, parsed, update);
        if markers.is_empty() {
            continue;
        }
        return Err(CaptureError::usage_code(
            format!(
                "cannot relate {}: “{}” would be completed by “{}”, but this capture also uses {}; capture the complete-label link alone",
                typed.raw,
                update.source.text,
                edge.label,
                markers.join(" and "),
            ),
            "relation_complete_conflicts_with_item_action",
        ));
    }
    Ok(())
}

/// Stage one source's completion (or record its link-only outcome) and
/// mark every triggering update with its `complete_effect`.
#[allow(clippy::too_many_arguments)]
fn plan_source_complete(
    ctx: &mut DependencyContext,
    planner: &mut CaptureBatchPlanner,
    request: &CaptureRequest,
    parsed: &ParsedCaptureText,
    updates: &mut [RelationUpdateJson],
    group: &[ClosingEdge],
    existing_owner: bool,
) -> Result<Option<PlannedSourceComplete>, CaptureError> {
    let bob_dir = &request.bob_dir;
    let first_source = updates[group[0].update_index].source.clone();
    let first_target = updates[group[0].update_index].target.clone();
    let source_relative = PathBuf::from(&first_source.path);
    let source_absolute = absolute_note(bob_dir, &source_relative);
    let staged = current_note_text(planner, &source_absolute)?;
    let display = display_relative(&source_relative);
    let scan = note_tasks::scan(&staged, ctx.settings());
    let (line_index, description) = if first_source.block_id.is_empty() {
        let line_index = first_source.line.saturating_sub(1);
        let task = scan.task_at(line_index).ok_or_else(|| {
            CaptureError::io(
                "relation complete invariant failed: source task is missing from staged text",
            )
        })?;
        (line_index, task.description.clone())
    } else {
        let task = lookup_staged_task(&scan, &display, &first_source.block_id)
            .map_err(|error| {
                CaptureError::io(format!(
                    "relation complete invariant failed: {}",
                    error.message
                ))
            })?;
        (task.line_index, task.description.clone())
    };
    let source_line = staged
        .lines()
        .nth(line_index)
        .unwrap_or_default()
        .to_string();
    let source_is_new = group.iter().all(|edge| {
        updates[edge.update_index].direction == RelationDirection::To
    }) && !existing_owner;
    let source_block_id = if first_source.block_id.is_empty() {
        None
    } else {
        Some(first_source.block_id.clone())
    };
    let tasks_settings =
        crate::native::task_status_hooks::read_tasks_settings(bob_dir);
    let verdict =
        complete_verdict(&source_line, &tasks_settings, source_is_new);
    let labels: Vec<String> = group
        .iter()
        .map(|edge| {
            updates[edge.update_index].label.clone().unwrap_or_default()
        })
        .collect();
    let action = updates[group[0].update_index].action;
    let target = updates[group[0].update_index].target.clone();
    let registry = source_registry_key(&updates[group[0].update_index]);
    match verdict {
        CompleteSourceVerdict::NewTask => Err(CaptureError::usage_code(
            format!(
                "“{}” is new — it would be born done · to finish “{}”, use ~~",
                first_source.text, first_target.text,
            ),
            "relation_completes_new_task",
        )),
        CompleteSourceVerdict::Recurring => Err(CaptureError::usage_code(
            format!(
                "“{}” repeats — complete it in Obsidian so Tasks writes the next one",
                first_source.text,
            ),
            "relation_completes_recurring_task",
        )),
        CompleteSourceVerdict::AlreadyClosed { status } => {
            for edge in group {
                updates[edge.update_index].complete_effect =
                    Some(RelationCompleteEffect::AlreadyClosed);
            }
            let previous_status_name = status_name_for(bob_dir, status);
            let final_staged = current_note_text(planner, &source_absolute)?;
            let final_line = locate_final_line(
                &final_staged,
                ctx,
                &display,
                source_block_id.as_deref(),
                first_source.line,
            );
            Ok(Some(PlannedSourceComplete {
                row: RelationCompleteJson {
                    effect: RelationCompleteEffect::AlreadyClosed,
                    source: RelationCompleteSourceJson {
                        path: display,
                        line: final_line,
                        block_id: source_block_id.unwrap_or_default(),
                        text: description,
                    },
                    target: RelationCompleteTargetJson {
                        path: target.path.clone(),
                        block_id: target.block_id.clone(),
                        text: target.text.clone(),
                    },
                    labels,
                    action,
                    previous_status: status,
                    previous_status_name: previous_status_name.clone(),
                    project_open_tasks: None,
                    task_complete: empty_complete_summary(),
                    message: already_closed_complete_message(
                        status,
                        &previous_status_name,
                    ),
                },
                effect: RelationCompleteEffect::AlreadyClosed,
                task_complete_refs: CompleteTracking::default(),
            }))
        }
        CompleteSourceVerdict::Completes {
            previous_status,
            project,
        } => {
            let block_id = source_block_id.clone().ok_or_else(|| {
                CaptureError::io(
                    "relation complete invariant failed: completable source has no block id",
                )
            })?;
            ctx.record_close_intent(
                registry.0,
                registry.1,
                CloseEffect::Complete,
            );
            let raw = parsed.relations[group[0].typed_index].raw.clone();
            let note = note_token(&first_source.path);
            let outcome = complete_existing_task(
                request, &raw, note, &block_id, ctx.today, planner, ctx,
            )?;
            for edge in group {
                updates[edge.update_index].complete_effect =
                    Some(RelationCompleteEffect::Completed);
            }
            let final_staged = current_note_text(planner, &source_absolute)?;
            let final_line = locate_final_line(
                &final_staged,
                ctx,
                &display,
                Some(&block_id),
                first_source.line,
            );
            let previous_status_name =
                status_name_for(bob_dir, previous_status);
            let project_open_tasks = if project {
                Some(count_open_tasks(&final_staged, ctx, final_line))
            } else {
                None
            };
            let struck = outcome
                .summary
                .ledger
                .as_ref()
                .map(|ledger| ledger.struck)
                .unwrap_or(0);
            let message = complete_message(
                request.dry_run,
                &description,
                &previous_status_name,
                struck,
                &outcome.summary.unblocked,
                project_open_tasks,
            );
            let tracking = CompleteTracking {
                pomodoro_refs: outcome.pomodoro_refs.clone(),
                task_block_refs: outcome.task_block_refs.clone(),
                day_file: outcome.top_day_file.clone(),
            };
            Ok(Some(PlannedSourceComplete {
                row: RelationCompleteJson {
                    effect: RelationCompleteEffect::Completed,
                    source: RelationCompleteSourceJson {
                        path: display,
                        line: final_line,
                        block_id: block_id.clone(),
                        text: description,
                    },
                    target: RelationCompleteTargetJson {
                        path: target.path.clone(),
                        block_id: target.block_id.clone(),
                        text: target.text.clone(),
                    },
                    labels,
                    action,
                    previous_status,
                    previous_status_name,
                    project_open_tasks,
                    task_complete: outcome.summary,
                    message,
                },
                effect: RelationCompleteEffect::Completed,
                task_complete_refs: tracking,
            }))
        }
    }
}

fn note_token(path: &str) -> &str {
    path.strip_suffix(".md").unwrap_or(path)
}

fn empty_complete_summary() -> TaskCompleteSummaryJson {
    TaskCompleteSummaryJson {
        raw: String::new(),
        note: String::new(),
        note_path: String::new(),
        block_id: String::new(),
        action: "already_done",
        completion_date: None,
        text: String::new(),
        subtasks: Vec::new(),
        subtasks_left_open: Vec::new(),
        ledger: None,
        unblocked: Vec::new(),
        still_blocked: Vec::new(),
        unblocked_check: "checked",
        project_folds: Vec::new(),
    }
}

/// The bob-authored capture row for a staged complete
/// (`docs/task-relations.md` §18 CP6): names S, the status transition,
/// the strike count, and the recovered dependent. Dry runs use
/// would-forms.
fn complete_message(
    dry_run: bool,
    source_text: &str,
    previous_status_name: &str,
    struck: usize,
    unblocked: &[TaskCompleteUnblockedJson],
    project_open_tasks: Option<usize>,
) -> String {
    let mut message = if dry_run {
        format!(
            "✓ would complete “{source_text}” · {previous_status_name} → Done"
        )
    } else {
        format!("✓ completed “{source_text}” · {previous_status_name} → Done")
    };
    if struck > 0 {
        let noun = if struck == 1 {
            "Pomodoro link"
        } else {
            "Pomodoro links"
        };
        if dry_run {
            message.push_str(&format!(" · would strike {struck} {noun}"));
        } else {
            message.push_str(&format!(" · struck {struck} {noun}"));
        }
    }
    match unblocked {
        [] => {}
        [one] => {
            if dry_run {
                message.push_str(&format!(" · would unblock “{}”", one.text));
            } else {
                message.push_str(&format!(" · unblocked “{}”", one.text));
            }
        }
        many => {
            let count = many.len();
            if dry_run {
                message.push_str(&format!(" · would unblock {count} tasks"));
            } else {
                message.push_str(&format!(" · unblocked {count} tasks"));
            }
        }
    }
    if let Some(open_tasks) = project_open_tasks
        && open_tasks > 0
    {
        if open_tasks == 1 {
            message.push_str(" · its 1 open task stays open");
        } else {
            message
                .push_str(&format!(" · its {open_tasks} open tasks stay open"));
        }
    }
    message
}

/// The muted link-only row for an already-closed source
/// (`docs/task-relations.md` §18 CP6).
fn already_closed_complete_message(status: char, status_name: &str) -> String {
    match status {
        'x' | 'X' => "Already Done · link only".to_string(),
        '-' => "Stays Cancelled · link only".to_string(),
        _ => format!("Stays {status_name} · link only"),
    }
}
