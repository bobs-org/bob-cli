//! Task-toggle capture planning.
use super::*;
use crate::native::task_complete as engine;

#[allow(clippy::too_many_arguments)]
pub(super) fn plan_task_toggle_capture(
    planner: &mut CaptureBatchPlanner,
    bob_dir: &Path,
    target: &Path,
    route: &str,
    block_id: &str,
    intent: TaskToggleIntent,
    now: chrono::NaiveDateTime,
    today: NaiveDate,
    warnings: &mut Vec<CaptureWarning>,
    ctx: &mut DependencyContext,
    task_theme_context: &mut CaptureTaskThemeContext,
    selection: engine::SuccessorSelection,
) -> Result<(CaptureWritePlan, Option<LinkGuardJson>), CaptureError> {
    let contents = planner.read_existing(target)?;
    let settings = note_tasks::read_settings(bob_dir);
    let scan = note_tasks::scan(&contents, &settings);
    let task = scan
        .lookup_capture_task(route, block_id)
        .map_err(CaptureError::io)?;
    let task_line_index = task.line_index;
    let previous_status_symbol = task.status_symbol;
    let previous_status_name = task.status_name.clone();
    let task_description = task.description.clone();
    let previous_task_line =
        line_text_at(&contents, task_line_index)?.to_string();

    if intent == TaskToggleIntent::EnsureNext {
        return plan_ensure_next_capture(
            planner,
            bob_dir,
            target,
            route,
            block_id,
            &contents,
            task_line_index,
            previous_status_symbol,
            previous_status_name,
            task_description,
            previous_task_line,
            now,
            today,
            warnings,
            &settings,
            ctx,
            task_theme_context,
        );
    }

    // Link-presence toggle: the ledger decides the direction. A matching
    // link under any open Pomodoro unlinks (ledger only, lane kept);
    // otherwise the task links under its typed placement (Ready/Blocked
    // rise to Next, Next/In Progress keep theirs).
    match previous_status_symbol {
        ' ' | '?' | '*' | '/' => {}
        _ => {
            return Err(CaptureError::io(format!(
                "task ^{block_id} is {previous_status_name}; only Ready, Blocked, Next, and In Progress tasks can be toggled"
            )));
        }
    };

    let day_file = pomodoro::day_file_for(bob_dir);
    if !day_file.is_file() {
        return Err(CaptureError::io(format!(
            "Bob daily note does not exist: {}",
            day_file.display()
        )));
    }
    let block_link = format!("[[{route}#^{block_id}]]");
    let day_contents = planner.read_existing(&day_file)?;
    let day_file_label = day_file.display().to_string();

    // "Linked" is exactly `plan_link_removal`'s matcher: a link under an
    // open entry of today's daily note.
    let removal =
        capture_task_toggle::plan_link_removal(&day_contents, &block_link);
    if removal.has_changes {
        let day_relative =
            day_file.strip_prefix(bob_dir).unwrap_or(day_file.as_path());
        planner.stage(&day_file, removal.content.clone())?;
        let relation = plan_unlink_relation_successors(
            planner,
            ctx,
            bob_dir,
            target,
            &day_file,
            day_relative,
            block_id,
            previous_status_symbol,
            &previous_task_line,
            &day_contents,
            &removal.content,
            removal.first_removed_line,
            today,
            selection,
        )?;
        let final_day = planner
            .current_contents(&day_file)?
            .unwrap_or_else(|| removal.content.clone());
        let pomodoro_refs = restamp_changed_timed_entries(
            planner,
            bob_dir,
            &day_file,
            day_relative,
            &day_contents,
            final_day,
            now,
            warnings,
        )?;
        return Ok((
            CaptureWritePlan {
                placement: Placement::Toggled,
                pomodoro: None,
                sub_bullet: None,
                pomodoro_note: None,
                toggle: Some(TaskToggleCaptureDetails {
                    direction: TaskToggleDirection::Unlink,
                    previous_task_line: previous_task_line.clone(),
                    task_line: previous_task_line,
                    task_description,
                    previous_status_symbol,
                    previous_status_name: previous_status_name.clone(),
                    status_symbol: previous_status_symbol,
                    status_name: previous_status_name,
                    block_id: block_id.to_string(),
                    day_file: day_file_label,
                    block_link,
                    pomodoro_link_placement: None,
                    pomodoro_name: None,
                    creates_pomodoro: false,
                    pomodoro_already_linked: false,
                    removed_pomodoro_links: removal.removed_links,
                    removed_scheduled: None,
                    schedule_log: None,
                    pomodoro_selector_unused: false,
                    toggle_behavior: None,
                    status_changed: Some(false),
                    pomodoro_link_action: None,
                    pomodoro_link_source: None,
                    pomodoro_link_destination: None,
                    dropped_dependencies: Vec::new(),
                    task_block_ref: None,
                    blocks_updates: Vec::new(),
                    relation,
                }),
                pomodoro_link: None,
                pomodoro_refs,
            },
            None,
        ));
    }

    // Link guard (§13.5 gesture rule, PDG1–PDG3, PDG6, PDG18–PDG27):
    // a toggle that would link a Blocked target either drops its
    // eligible own prerequisites and links, or consumes instead of
    // linking. The guard replaces the old `[dependsOn::` text warning:
    // its verdict is authoritative for this instant, so a link that
    // proceeds needs no warning.
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
                // PDG1/PDG3: the target stays Blocked, so the task line and
                // the checkbox stays untouched. Its stale live links are
                // consumed by identity before the open prerequisites rise.
                let dependent_desc = format!("link toggle ^{block_id}");
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
                let consumption = consume_live_links(
                    planner,
                    bob_dir,
                    &identities,
                    now,
                    warnings,
                )?;
                append_consumption_warning(warnings, consumption.removed_links);
                let promoted = promote_consume_plan(
                    ctx,
                    planner,
                    bob_dir,
                    *plan,
                    &dependent_desc,
                )?;
                let mut link_guard =
                    LinkGuardJson::not_linked(&reasons, promoted, drop_refused);
                if consumption.removed_links > 0 {
                    link_guard.removed_pomodoro_links =
                        Some(consumption.removed_links);
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
                            direction: TaskToggleDirection::Link,
                            previous_task_line: previous_task_line.clone(),
                            task_line: previous_task_line,
                            task_description,
                            previous_status_symbol,
                            previous_status_name: previous_status_name.clone(),
                            status_symbol: previous_status_symbol,
                            status_name: previous_status_name,
                            block_id: block_id.to_string(),
                            day_file: day_file_label,
                            block_link,
                            pomodoro_link_placement: None,
                            pomodoro_name: None,
                            creates_pomodoro: false,
                            pomodoro_already_linked: false,
                            removed_pomodoro_links: consumption.removed_links,
                            removed_scheduled: None,
                            schedule_log: None,
                            pomodoro_selector_unused: false,
                            toggle_behavior: None,
                            status_changed: Some(false),
                            pomodoro_link_action: None,
                            pomodoro_link_source: None,
                            pomodoro_link_destination: None,
                            dropped_dependencies: Vec::new(),
                            task_block_ref: None,
                            blocks_updates: Vec::new(),
                            relation: ToggleRelationPlan::default(),
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
    let current_scan = note_tasks::scan(&current_contents, &settings);
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
    let updated_scan = note_tasks::scan(&task_plan.content, &settings);
    let updated_task = match updated_scan.by_block_id(block_id) {
        BlockIdLookup::Found(task) => task,
        _ => {
            return Err(CaptureError::io(
                "task toggle capture invariant failed: toggled task disappeared",
            ));
        }
    };
    let status_changed = previous_status_symbol != updated_task.status_symbol;

    // No `[dependsOn::` text warning here: the link guard above is
    // authoritative for this instant. A Blocked target never reaches
    // this path (consumed instead), and a linked target has no open
    // reason left for reconcile to re-block on. Typed-placement
    // suggestion warnings still report below.

    planner.stage(target, task_plan.content)?;

    // Re-read through the staged snapshot: when the route note is the
    // daily note, insertion must build on the just-staged task edit.
    let day_contents = planner.read_existing(&day_file)?;

    // A `!` toggle places by the task's effective type (D7).
    let placement_type = task_theme_context
        .placement_for_task(planner, route, &task_line, warnings);
    let placed_type = placement_type.as_ref().map(|placement| {
        let typed = placement.as_typed();
        (typed, placement.placed_type())
    });
    let typed = placed_type.as_ref().map(|(typed, _)| typed);

    let plan = capture_task_toggle::plan_link_insertion(
        &day_contents,
        &block_link,
        None,
        false,
        typed,
    )
    .map_err(link_plan_error)?;
    let capture_task_toggle::LinkInsertionOutcome::Planned(plan) = plan else {
        return Err(CaptureError::io(
            "task toggle capture invariant failed: implicit placement requested a named Pomodoro",
        ));
    };
    let pomodoro_link_placement =
        plan.placement.map(link_placement_to_placement);
    let resolved_pomodoro_name = plan.pomodoro_name;
    let pomodoro_already_linked = plan.already_linked;
    let removed_pomodoro_links = plan.removed_links;
    let creates_pomodoro = plan.creates_pomodoro;
    let pomodoro_selector_unused = false;
    let pomodoro_link_destination =
        plan.destination.as_ref().and_then(|destination| {
            placed_type
                .as_ref()
                .map(|(_, placed)| PomodoroLinkEndpoint {
                    line: destination.line,
                    name: destination.name.clone(),
                    time_range: destination.time_range.clone(),
                    role: Some(destination_role(
                        plan.creates_pomodoro,
                        None,
                        destination.time_range.as_deref(),
                    )),
                    placed_type: Some(placed.clone()),
                    merged: None,
                })
        });
    let pomodoro_refs = if plan.has_changes {
        let day_relative =
            day_file.strip_prefix(bob_dir).unwrap_or(day_file.as_path());
        restamp_changed_timed_entries(
            planner,
            bob_dir,
            &day_file,
            day_relative,
            &day_contents,
            plan.content,
            now,
            warnings,
        )?
    } else {
        Vec::new()
    };

    Ok((
        CaptureWritePlan {
            placement: Placement::Toggled,
            pomodoro: None,
            sub_bullet: None,
            pomodoro_note: None,
            toggle: Some(TaskToggleCaptureDetails {
                direction: TaskToggleDirection::Link,
                previous_task_line,
                task_line,
                task_description,
                previous_status_symbol,
                previous_status_name,
                status_symbol: updated_task.status_symbol,
                status_name: updated_task.status_name.clone(),
                block_id: block_id.to_string(),
                day_file: day_file_label,
                block_link,
                pomodoro_link_placement,
                pomodoro_name: resolved_pomodoro_name,
                creates_pomodoro,
                pomodoro_already_linked,
                removed_pomodoro_links,
                removed_scheduled: task_plan.removed_scheduled,
                schedule_log: task_plan.schedule_log,
                pomodoro_selector_unused,
                toggle_behavior: None,
                status_changed: Some(status_changed),
                pomodoro_link_action: None,
                pomodoro_link_source: None,
                pomodoro_link_destination,
                dropped_dependencies,
                task_block_ref,
                blocks_updates,
                relation: ToggleRelationPlan::default(),
            }),
            pomodoro_link: None,
            pomodoro_refs,
        },
        None,
    ))
}

/// Raise `(T, parked)` when `@route+id!` unlinks a Pending task
/// (`docs/task-dependencies.md` §12.18 SA31). Next, Ready, and Blocked
/// unlinks stay silent (SA32). Relation-only: C is empty.
#[allow(clippy::too_many_arguments)]
fn plan_unlink_relation_successors(
    planner: &mut CaptureBatchPlanner,
    ctx: &mut DependencyContext,
    bob_dir: &Path,
    target: &Path,
    day_file: &Path,
    day_relative: &Path,
    block_id: &str,
    status_symbol: char,
    task_line: &str,
    pre_day: &str,
    post_day: &str,
    first_removed_line: Option<usize>,
    today: NaiveDate,
    selection: engine::SuccessorSelection,
) -> Result<ToggleRelationPlan, CaptureError> {
    if status_symbol != '/'
        || block_id.is_empty()
        || ctx.successor_labels().is_empty()
        || !ctx.link_unblocked()
    {
        return Ok(ToggleRelationPlan::default());
    }
    let settings = note_tasks::read_settings(bob_dir);
    let relative = target
        .strip_prefix(bob_dir)
        .map(Path::to_path_buf)
        .unwrap_or_else(|_| target.to_path_buf());
    let parked = vec![engine::RelationEvent {
        target: engine::RelationTarget {
            note_path: relative.clone(),
            block_id: block_id.to_string(),
            text: task_complete_display_text(
                task_line,
                &settings.global_filter,
                Some(block_id),
            ),
        },
        kind: engine::RelationEventKind::Parked,
        slot: first_removed_line.map(|line| engine::RelationSlotAnchor {
            note_path: day_relative.to_path_buf(),
            line,
        }),
    }];
    let mut note_texts = Vec::new();
    if let Some(contents) = planner.current_contents(target)? {
        note_texts.push((relative, contents));
    }
    let tasks_settings =
        crate::native::task_status_hooks::read_tasks_settings(bob_dir);
    let index = super::task_complete::staged_note_index(bob_dir, planner, ctx);
    let Some(read) = super::relation_sources::collect_relation_sources(
        ctx,
        planner,
        bob_dir,
        &[],
        &parked,
        &note_texts,
        &tasks_settings,
        &index,
    )?
    else {
        return Ok(ToggleRelationPlan::default());
    };
    super::task_complete::overlay_note_texts(
        &mut note_texts,
        read.source_notes,
    );
    super::task_complete::overlay_note_texts(
        &mut note_texts,
        read.target_notes,
    );
    super::task_complete::overlay_note_texts(
        &mut note_texts,
        read.dependency_notes,
    );
    let snapshot: Vec<(PathBuf, String)> = Vec::new();
    let successor_plan = engine::plan_successors(engine::SuccessorInput {
        bob_dir: Some(bob_dir),
        completed: Vec::new(),
        snapshot,
        note_texts,
        pre_day: Some(pre_day.to_string()),
        post_day: Some(post_day.to_string()),
        closing: None,
        day_relative: day_relative.to_path_buf(),
        today,
        tasks_settings: &tasks_settings,
        note_settings: &settings,
        link_unblocked: ctx.link_unblocked(),
        keep_moving: ctx.keep_moving(),
        snapshot_complete: true,
        relation: Some(read.input),
        selection,
        index: &index,
    });
    for (relative_path, text) in &successor_plan.changed_files {
        planner.stage(&bob_dir.join(relative_path), text.clone())?;
    }
    if let Some(updated_day) = successor_plan.new_day_text.clone() {
        planner.stage(day_file, updated_day)?;
    }
    let day_relative_display = day_relative.to_string_lossy();
    let successors = successor_plan
        .relation_successors
        .iter()
        .map(|row| {
            unblocked_row_json(
                row,
                bob_dir,
                day_relative_display.as_ref(),
                &settings.global_filter,
            )
        })
        .collect::<Vec<_>>();
    let held = successor_plan
        .relation_held
        .iter()
        .map(|row| {
            relation_held_row_json(row, bob_dir, &settings.global_filter)
        })
        .collect::<Vec<_>>();
    let check =
        relation_check_json(successor_plan.relation_check, &successors, &held);
    let mut task_block_refs = Vec::new();
    for dependent in &successor_plan.relation_successors {
        if dependent.link.is_none() {
            continue;
        }
        let absolute_path = bob_dir.join(&dependent.note_path);
        let rel = display_relative(&dependent.note_path);
        let mut stem = dependent.note_path.clone();
        stem.set_extension("");
        super::task_complete::push_relation_successor_ref(
            &mut task_block_refs,
            absolute_path,
            rel,
            display_relative(&stem),
            dependent.line.saturating_sub(1),
            if dependent.block_id.is_empty() {
                None
            } else {
                Some(dependent.block_id.clone())
            },
        );
    }
    Ok(ToggleRelationPlan {
        successors,
        held,
        check,
        groups: successor_plan.relation_groups.clone(),
        task_block_refs,
    })
}
