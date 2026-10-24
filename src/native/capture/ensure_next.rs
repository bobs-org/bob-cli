//! Ensure-next capture planning.
use super::*;

#[allow(clippy::too_many_arguments)]
pub(super) fn plan_ensure_next_capture(
    planner: &mut CaptureBatchPlanner,
    bob_dir: &Path,
    target: &Path,
    route: &str,
    block_id: &str,
    pomodoro_name: Option<&str>,
    pomodoro_name_is_literal: bool,
    contents: &str,
    _task_line_index: usize,
    previous_status_symbol: char,
    previous_status_name: String,
    task_description: String,
    previous_task_line: String,
    today: NaiveDate,
    warnings: &mut Vec<String>,
    settings: &note_tasks::NoteTaskSettings,
    ctx: &mut DependencyContext,
) -> Result<(CaptureWritePlan, Option<LinkGuardJson>), CaptureError> {
    match previous_status_symbol {
        ' ' | '?' | '/' | '*' => {}
        _ => {
            return Err(CaptureError::io(format!(
                "task ^{block_id} is {previous_status_name}; only Ready, Blocked, In Progress, and Next tasks can be ensured Next"
            )));
        }
    }

    // Link semantics: Ready/Blocked rise to Next, while Next and In
    // Progress keep their lane (sticky lanes: no capture path demotes).
    // Link guard (§13.5 gesture rule, PDG1–PDG3, PDG6, PDG18–PDG27): a
    // Blocked target either drops its eligible own prerequisites and
    // relocates, or consumes instead of linking; the guard replaces the
    // old `[dependsOn::` text warning.
    let target_rel = relative_target(Some(route));
    let verdict = assess_link_target(
        ctx,
        planner,
        bob_dir,
        &target_rel,
        block_id,
        today,
    )?;
    let resolution = resolve_link_guard(
        ctx,
        planner,
        bob_dir,
        &target_rel,
        block_id,
        verdict,
        warnings,
    )?;
    let (dropped_dependencies, task_block_ref, blocks_updates) =
        match resolution {
            ResolvedLinkGuard::Proceed {
                dropped,
                task_block_ref,
                blocks_updates,
            } => (dropped, task_block_ref, blocks_updates),
            ResolvedLinkGuard::Consume(plan) => {
                let dependent_desc = format!("ensure next ^{block_id}");
                let reasons = plan.reasons.clone();
                let drop_refused = plan.drop_refused;
                let target_identity_path = target
                    .strip_prefix(bob_dir)
                    .map(Path::to_path_buf)
                    .unwrap_or_else(|_| target_rel.clone());
                let identities = std::collections::BTreeSet::from([(
                    target_identity_path,
                    block_id.to_string(),
                )]);
                let consumption =
                    consume_live_links(planner, bob_dir, &identities)?;
                append_consumption_warning(warnings, consumption.removed_links);
                let removed_pomodoro_links = consumption.removed_links;
                let promoted = promote_consume_plan(
                    ctx,
                    planner,
                    bob_dir,
                    *plan,
                    &dependent_desc,
                )?;
                let mut link_guard =
                    LinkGuardJson::not_linked(&reasons, promoted, drop_refused);
                if removed_pomodoro_links > 0 {
                    link_guard.removed_pomodoro_links =
                        Some(removed_pomodoro_links);
                    link_guard.day_file = Some(
                        pomodoro::day_file_for(bob_dir).display().to_string(),
                    );
                }
                return Ok((
                    CaptureWritePlan {
                        placement: Placement::Toggled,
                        pomodoro: None,
                        sub_bullet: None,
                        pomodoro_note: None,
                        toggle: Some(TaskToggleCaptureDetails {
                            direction: TaskToggleDirection::Next,
                            previous_task_line: previous_task_line.clone(),
                            task_line: previous_task_line,
                            task_description,
                            previous_status_symbol,
                            previous_status_name: previous_status_name.clone(),
                            status_symbol: previous_status_symbol,
                            status_name: previous_status_name,
                            block_id: block_id.to_string(),
                            day_file: pomodoro::day_file_for(bob_dir)
                                .display()
                                .to_string(),
                            block_link: format!("[[{route}#^{block_id}]]"),
                            pomodoro_link_placement: None,
                            pomodoro_name: None,
                            creates_pomodoro: false,
                            pomodoro_already_linked: false,
                            removed_pomodoro_links,
                            removed_scheduled: None,
                            schedule_log: None,
                            pomodoro_selector_unused: false,
                            toggle_behavior: Some("ensure_next"),
                            status_changed: Some(false),
                            pomodoro_link_action: None,
                            pomodoro_link_source: None,
                            pomodoro_link_destination: None,
                            dropped_dependencies: Vec::new(),
                            task_block_ref: None,
                            blocks_updates: Vec::new(),
                        }),
                        pomodoro_link: None,
                        pomodoro_refs: consumption.pomodoro_refs,
                    },
                    Some(link_guard),
                ));
            }
        };
    // Re-read through the staged snapshot: a dependency drop staged
    // above rewrote the task line, so the link write must build on the
    // post-drop text.
    let current_contents = current_note_text(planner, target)?;
    let current_scan = note_tasks::scan(&current_contents, settings);
    let current_task = match current_scan.by_block_id(block_id) {
        BlockIdLookup::Found(task) => task,
        _ => {
            return Err(CaptureError::io(
                "task toggle capture invariant failed: task disappeared after dependency drop",
            ));
        }
    };
    let task_plan = capture_task_toggle::plan_task_link(
        &current_contents,
        current_task.line_index,
        today,
    )
    .ok_or_else(|| {
        CaptureError::io(
            "task toggle capture invariant failed: task line could not be updated",
        )
    })?;
    let task_line =
        line_text_at(&task_plan.content, current_task.line_index)?.to_string();
    let updated_scan = note_tasks::scan(&task_plan.content, settings);
    let updated_task = match updated_scan.by_block_id(block_id) {
        BlockIdLookup::Found(task) => task,
        _ => {
            return Err(CaptureError::io(
                "task toggle capture invariant failed: toggled task disappeared",
            ));
        }
    };
    let status_changed = previous_status_symbol != updated_task.status_symbol;
    // No `[dependsOn::` text warning: the link guard above is
    // authoritative (see `plan_task_toggle_capture`).
    let _ = warnings;

    if task_plan.content != contents {
        planner.stage(target, task_plan.content.clone())?;
    }

    let day_file = pomodoro::day_file_for(bob_dir);
    if !day_file.is_file() {
        return Err(CaptureError::io(format!(
            "Bob daily note does not exist: {}",
            day_file.display()
        )));
    }
    let block_link = format!("[[{route}#^{block_id}]]");
    let day_contents = planner.read_existing(&day_file)?;
    let relocation = capture_task_toggle::plan_link_relocation(
        &day_contents,
        &block_link,
        pomodoro_name,
        pomodoro_name_is_literal,
    )
    .map_err(|error| {
        relocation_plan_error(error, &block_link, route, block_id)
    })?;
    if relocation.has_changes {
        planner.stage(&day_file, relocation.content.clone())?;
    }

    let selector = pomodoro_name;
    let pomodoro_link_action = match relocation.action {
        capture_task_toggle::LinkRelocationAction::Moved => "moved",
        capture_task_toggle::LinkRelocationAction::AlreadyCurrent => {
            "already_current"
        }
    };
    let pomodoro_already_linked = matches!(
        relocation.action,
        capture_task_toggle::LinkRelocationAction::AlreadyCurrent
    );
    let pomodoro_link_placement =
        relocation.placement.map(link_placement_to_placement);
    let pomodoro_name = relocation.destination.name.clone();
    // The destination endpoint is post-state; a moved source keeps its
    // pre-state headline and resolves through the item's line map. An
    // already-current destination still pushes its ref so the block shows
    // with every line unchanged.
    let mut pomodoro_refs = vec![if relocation.creates_pomodoro {
        PomodoroBlockRef::created(
            PomodoroBlockRole::Linked,
            relocation.destination.line.saturating_sub(1),
        )
    } else {
        PomodoroBlockRef::resolved(
            PomodoroBlockRole::Linked,
            relocation.destination.line.saturating_sub(1),
        )
    }];
    if relocation.action == capture_task_toggle::LinkRelocationAction::Moved {
        pomodoro_refs.push(PomodoroBlockRef::unlinked_before(
            relocation.source.line.saturating_sub(1),
        ));
    }

    Ok((
        CaptureWritePlan {
            placement: Placement::Toggled,
            pomodoro: None,
            sub_bullet: None,
            pomodoro_note: None,
            toggle: Some(TaskToggleCaptureDetails {
                direction: TaskToggleDirection::Next,
                previous_task_line,
                task_line,
                task_description,
                previous_status_symbol,
                previous_status_name,
                status_symbol: updated_task.status_symbol,
                status_name: updated_task.status_name.clone(),
                block_id: block_id.to_string(),
                day_file: day_file.display().to_string(),
                block_link,
                pomodoro_link_placement,
                pomodoro_name,
                creates_pomodoro: relocation.creates_pomodoro,
                pomodoro_already_linked,
                removed_pomodoro_links: 0,
                removed_scheduled: task_plan.removed_scheduled,
                schedule_log: task_plan.schedule_log,
                pomodoro_selector_unused: false,
                toggle_behavior: Some("ensure_next"),
                status_changed: Some(status_changed),
                pomodoro_link_action: Some(pomodoro_link_action),
                pomodoro_link_source: Some(endpoint_json(&relocation.source)),
                pomodoro_link_destination: Some(destination_json(
                    &relocation.destination,
                    relocation.creates_pomodoro,
                    selector,
                )),
                dropped_dependencies,
                task_block_ref,
                blocks_updates,
            }),
            pomodoro_link: None,
            pomodoro_refs,
        },
        None,
    ))
}
