//! Capture `~` / `~~` cancel-label commits: stage the source cancel,
//! today's prune, and recovery in the capture batch with dry-run parity
//! (`docs/task-relations.md` §17).

use std::path::{Path, PathBuf};

use super::*;
use crate::native::{
    config::relations::CloseEffect,
    task_cancel::{
        plan_task_cancel, verdicts::CancelSourceVerdict, CancelLogOutcome,
        DailyReadState, TaskCancelError, TaskCancelInput,
    },
    task_relations::parse as relation_parse,
    task_status_hooks,
};

/// Plan the cancel-label commits for one item's already-staged relation
/// writes. Collects Added/Relabeled edges whose label is configured,
/// refuses the new-task, recurring, and conflict cases with coded usage
/// errors, and stages each cancelled source (LINKS, status, log),
/// today's prune, and recovery through the `CaptureBatchPlanner`
/// overlays. Already-closed sources keep the link only. With no cancel
/// labels configured this is a no-op, so older output stays
/// byte-identical.
#[allow(clippy::too_many_arguments)]
pub(super) fn plan_relation_cancels(
    ctx: &mut DependencyContext,
    planner: &mut CaptureBatchPlanner,
    bob_dir: &Path,
    parsed: &ParsedCaptureText,
    updates: &mut [RelationUpdateJson],
    existing_owner: bool,
    dry_run: bool,
    warnings: &mut Vec<CaptureWarning>,
) -> Result<Vec<RelationCancelJson>, CaptureError> {
    let configured: Vec<String> = ctx
        .cancel_labels()
        .iter()
        .map(|label| label.key.clone())
        .collect();
    if configured.is_empty() {
        return Ok(Vec::new());
    }
    let edges =
        collect_closing_edges(updates, &configured, CloseEffect::Cancel);
    if edges.is_empty() {
        return Ok(Vec::new());
    }
    refuse_cancel_conflicts(ctx, planner, bob_dir, parsed, updates, &edges)?;
    let groups = group_closing_edges(updates, edges);
    let mut cancels = Vec::new();
    for group in &groups {
        if let Some(cancel) = plan_source_cancel(
            ctx,
            planner,
            bob_dir,
            updates,
            group,
            existing_owner,
            dry_run,
        )? {
            cancels.push(cancel);
        }
    }
    // One structured warning per triggering label on a real cancel, so
    // machines can match the human ⊘ row. The human renderer prints the
    // ⊘ row once and never repeats these warning messages. Each label
    // resolves within its own source group so mixed-direction items with
    // the same label keep their own targets.
    for (group, cancel) in groups.iter().zip(cancels.iter()) {
        if cancel.effect == RelationCancelEffect::AlreadyClosed {
            continue;
        }
        let source_text = &cancel.source.text;
        let target_text = &cancel.target.text;
        for label in &cancel.labels {
            let target_text = group_target_for_label(updates, group, label)
                .unwrap_or(target_text);
            warnings.push(CaptureWarning::Structured(
                StructuredCaptureWarning {
                    code: "relation_cancelled_source",
                    label: Some(label.clone()),
                    count: None,
                    message: if dry_run {
                        format!(
                            "⊘ would cancel “{source_text}” ({label} “{target_text}”)"
                        )
                    } else {
                        format!(
                            "⊘ cancelled “{source_text}” ({label} “{target_text}”)"
                        )
                    },
                },
            ));
        }
    }
    Ok(cancels)
}

/// Refuse an item that also changes the cancel source through another
/// marker (`docs/task-relations.md` §17 CL5): `&` / `*` on an owner
/// source (`~`), or a `*` naming a picked source (`~~`). Capture is
/// all-or-nothing, so the refusal writes nothing.
fn refuse_cancel_conflicts(
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
                "cannot relate {}: “{}” would be cancelled by “{}”, but this capture also uses {}; capture the cancel-label link alone",
                typed.raw,
                update.source.text,
                edge.label,
                markers.join(" and "),
            ),
            "relation_cancel_conflicts_with_item_action",
        ));
    }
    Ok(())
}

/// Stage one source's cancel (or record its link-only outcome) and mark
/// every triggering update with its `cancel_effect`. Returns `None`
/// only when there is nothing to report, which cannot happen for a
/// non-empty group.
#[allow(clippy::too_many_arguments)]
fn plan_source_cancel(
    ctx: &mut DependencyContext,
    planner: &mut CaptureBatchPlanner,
    bob_dir: &Path,
    updates: &mut [RelationUpdateJson],
    group: &[ClosingEdge],
    existing_owner: bool,
    dry_run: bool,
) -> Result<Option<RelationCancelJson>, CaptureError> {
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
                "relation cancel invariant failed: source task is missing from staged text",
            )
        })?;
        (line_index, task.description.clone())
    } else {
        let task = lookup_staged_task(&scan, &display, &first_source.block_id)
            .map_err(|error| {
                CaptureError::io(format!(
                    "relation cancel invariant failed: {}",
                    error.message
                ))
            })?;
        (task.line_index, task.description.clone())
    };
    let source_is_new = group.iter().all(|edge| {
        updates[edge.update_index].direction == RelationDirection::To
    }) && !existing_owner;
    let source_block_id = if first_source.block_id.is_empty() {
        None
    } else {
        Some(first_source.block_id.clone())
    };
    let reasons = cancel_reasons(
        ctx,
        planner,
        bob_dir,
        &source_relative,
        line_index,
        updates,
        group,
    )?;
    let day_absolute = pomodoro::day_file_for(bob_dir);
    let day_relative = day_absolute
        .strip_prefix(bob_dir)
        .map(Path::to_path_buf)
        .unwrap_or_else(|_| day_absolute.clone());
    let daily_state = daily_read_state(planner, &day_absolute);
    let note_index =
        super::dependencies::staged_note_index(ctx, planner, bob_dir);
    let tasks_settings =
        crate::native::task_status_hooks::read_tasks_settings(bob_dir);
    let snapshots = staged_snapshot_for_recovery(bob_dir, planner, ctx);
    let plan = plan_task_cancel(TaskCancelInput {
        source_path: source_relative.clone(),
        source_block_id: source_block_id.clone(),
        source_line_index: line_index,
        source_text: staged,
        source_is_new,
        ordered_reasons: reasons,
        today: ctx.today,
        note_index: &note_index,
        tasks_settings: &tasks_settings,
        note_settings: ctx.settings(),
        daily_path: day_relative,
        daily_state,
        dependency_snapshots: snapshots,
    });
    let plan = match plan {
        Ok(plan) => plan,
        Err(TaskCancelError::Refused(CancelSourceVerdict::NewTask)) => {
            return Err(CaptureError::usage_code(
                format!(
                    "“{}” is new — it would be born cancelled · to retire “{}”, use ~~",
                    first_source.text, first_target.text,
                ),
                "relation_cancels_new_task",
            ));
        }
        Err(TaskCancelError::Refused(CancelSourceVerdict::Recurring)) => {
            return Err(CaptureError::usage_code(
                format!(
                    "“{}” repeats — cancel it with Obsidian Tasks first",
                    first_source.text,
                ),
                "relation_cancels_recurring_task",
            ));
        }
        Err(TaskCancelError::Refused(_)) => {
            return Err(CaptureError::io(
                "relation cancel invariant failed: refusal escaped as a plan",
            ));
        }
        Err(TaskCancelError::DailyRead(error)) => {
            return Err(CaptureError::io(error));
        }
    };
    let (effect, previous_status, is_project) = match plan.verdict {
        CancelSourceVerdict::Cancels {
            previous_status,
            project,
        } => {
            let registry = source_registry_key(&updates[group[0].update_index]);
            ctx.record_close_intent(
                registry.0,
                registry.1,
                CloseEffect::Cancel,
            );
            (RelationCancelEffect::Cancelled, previous_status, project)
        }
        CancelSourceVerdict::AlreadyClosed { status } => {
            (RelationCancelEffect::AlreadyClosed, status, false)
        }
        CancelSourceVerdict::Recurring | CancelSourceVerdict::NewTask => {
            return Err(CaptureError::io(
                "relation cancel invariant failed: refusal escaped as a plan",
            ));
        }
    };
    for edge in group {
        updates[edge.update_index].cancel_effect = Some(effect);
    }
    for (path, text) in &plan.postimages {
        planner.stage(&bob_dir.join(path), text.clone())?;
    }
    let final_staged = current_note_text(planner, &source_absolute)?;
    let final_line = locate_final_line(
        &final_staged,
        ctx,
        &display,
        source_block_id.as_deref(),
        first_source.line,
    );
    let previous_status_name = status_name_for(bob_dir, previous_status);
    let labels: Vec<String> = group
        .iter()
        .map(|edge| {
            updates[edge.update_index].label.clone().unwrap_or_default()
        })
        .collect();
    let action = updates[group[0].update_index].action;
    let target = updates[group[0].update_index].target.clone();
    if effect == RelationCancelEffect::Cancelled {
        let project_open_tasks = if is_project {
            Some(count_open_tasks(&final_staged, ctx, final_line))
        } else {
            None
        };
        let recovered = plan
            .recovered
            .iter()
            .map(|task| RelationCancelRecoveredJson {
                path: display_relative(&task.path),
                line: task.line,
                block_id: task.block_id.clone().unwrap_or_default(),
                text: clean_task_text(
                    &task.text,
                    task.block_id.as_deref(),
                    ctx,
                ),
            })
            .collect::<Vec<_>>();
        let cancel_log = match plan.cancel_log {
            CancelLogOutcome::Prepended => "prepended",
            CancelLogOutcome::None | CancelLogOutcome::NotPresent => "none",
        };
        let cancel = RelationCancelJson {
            effect: RelationCancelEffect::Cancelled,
            source: RelationCancelSourceJson {
                path: display.clone(),
                line: final_line,
                block_id: source_block_id.clone().unwrap_or_default(),
                text: description.clone(),
            },
            target: RelationCancelTargetJson {
                path: target.path.clone(),
                block_id: target.block_id.clone(),
                text: target.text.clone(),
            },
            labels,
            action,
            previous_status,
            previous_status_name: previous_status_name.clone(),
            project_open_tasks,
            cancel_log,
            pomodoro_links_removed: plan.removed_pomodoro_links,
            recovered: recovered.clone(),
            message: cancel_message(
                dry_run,
                &description,
                &previous_status_name,
                plan.removed_pomodoro_links,
                &recovered,
                project_open_tasks,
            ),
        };
        return Ok(Some(cancel));
    }
    Ok(Some(RelationCancelJson {
        effect: RelationCancelEffect::AlreadyClosed,
        source: RelationCancelSourceJson {
            path: display,
            line: final_line,
            block_id: source_block_id.unwrap_or_default(),
            text: description,
        },
        target: RelationCancelTargetJson {
            path: target.path.clone(),
            block_id: target.block_id.clone(),
            text: target.text.clone(),
        },
        labels,
        action,
        previous_status,
        previous_status_name: previous_status_name.clone(),
        project_open_tasks: None,
        cancel_log: "none",
        pomodoro_links_removed: 0,
        recovered: Vec::new(),
        message: already_closed_message(previous_status),
    }))
}

/// The fallback Cancel Log reasons for one source, in authored order:
/// `🪢 {label} {link}` with the exact link text the staged LINKS entry
/// uses.
fn cancel_reasons(
    ctx: &DependencyContext,
    planner: &mut CaptureBatchPlanner,
    bob_dir: &Path,
    source_relative: &Path,
    line_index: usize,
    updates: &[RelationUpdateJson],
    group: &[ClosingEdge],
) -> Result<Vec<String>, CaptureError> {
    let source_absolute = absolute_note(bob_dir, source_relative);
    let staged = current_note_text(planner, &source_absolute)?;
    let owned: Vec<String> = staged.lines().map(str::to_string).collect();
    let lines: Vec<&str> = owned.iter().map(String::as_str).collect();
    let fenced = task_status_hooks::fenced_lines(&lines, 0..lines.len());
    let children =
        relation_parse::links_children_of(&lines, &fenced, line_index);
    let index = super::dependencies::staged_note_index(ctx, planner, bob_dir);
    let mut reasons = Vec::with_capacity(group.len());
    for edge in group {
        let update = &updates[edge.update_index];
        let target_relative = PathBuf::from(&update.target.path);
        let (link, _) = existing_links_entry(
            &children,
            &index,
            source_relative,
            &target_relative,
            &update.target.block_id,
        )
        .ok_or_else(|| {
            CaptureError::io(
                "relation cancel invariant failed: staged LINKS entry is missing",
            )
        })?;
        reasons.push(format!("🪢 {} {link}", edge.label));
    }
    Ok(reasons)
}

/// Today's daily note as the cancel engine reads it: staged text wins,
/// an absent note means zero links, and an unreadable note is an error.
fn daily_read_state(
    planner: &mut CaptureBatchPlanner,
    day_absolute: &Path,
) -> DailyReadState {
    if let Some(staged) = planner.peek_text(day_absolute) {
        return DailyReadState::Present(staged);
    }
    match std::fs::read_to_string(day_absolute) {
        Ok(contents) => DailyReadState::Present(contents),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            DailyReadState::Absent
        }
        Err(error) => DailyReadState::Failed(format!(
            "cannot read today's daily note {}: {error}",
            day_absolute.display()
        )),
    }
}

/// Clean display text for a recovered dependent line.
fn clean_task_text(
    line: &str,
    block_id: Option<&str>,
    ctx: &DependencyContext,
) -> String {
    let body = line
        .find("] ")
        .map(|index| &line[index + 2..])
        .unwrap_or(line);
    note_tasks::clean_description(body, &ctx.settings().global_filter, block_id)
}

/// The bob-authored capture row for a staged cancel
/// (`docs/task-relations.md` §17 CL6): names S, the status transition,
/// the prune count, and the recovered dependent. Dry runs use
/// would-forms.
fn cancel_message(
    dry_run: bool,
    source_text: &str,
    previous_status_name: &str,
    pomodoro_links_removed: usize,
    recovered: &[RelationCancelRecoveredJson],
    project_open_tasks: Option<usize>,
) -> String {
    let mut message = if dry_run {
        format!(
            "⊘ would cancel “{source_text}” · {previous_status_name} → Cancelled"
        )
    } else {
        format!(
            "⊘ cancelled “{source_text}” · {previous_status_name} → Cancelled"
        )
    };
    if pomodoro_links_removed > 0 {
        let noun = if pomodoro_links_removed == 1 {
            "Pomodoro link"
        } else {
            "Pomodoro links"
        };
        if dry_run {
            message.push_str(&format!(
                " · would remove {pomodoro_links_removed} {noun}"
            ));
        } else {
            message.push_str(&format!(
                " · removed {pomodoro_links_removed} {noun}"
            ));
        }
    }
    match recovered {
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
/// (`docs/task-relations.md` §17 CL6). Capture prints this as a human
/// line only: `-` is Already Cancelled, every other closed symbol
/// stays Done.
fn already_closed_message(status: char) -> String {
    if status == '-' {
        "Already Cancelled · link only".to_string()
    } else {
        "Stays Done · link only".to_string()
    }
}
