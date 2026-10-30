//! Pomodoro link capture planning, including link-with-start.
use super::*;

#[allow(clippy::too_many_arguments)]
pub(super) fn plan_capture_with_pomodoro_link(
    planner: &mut CaptureBatchPlanner,
    bob_dir: &Path,
    target: &Path,
    route: &str,
    block_id: &str,
    pomodoro_name: Option<&str>,
    pomodoro_name_is_literal: bool,
    start: Option<&PomodoroStartSpec>,
    capture_block: &str,
    now: chrono::NaiveDateTime,
    task_theme_context: &mut CaptureTaskThemeContext,
    warnings: &mut Vec<CaptureWarning>,
) -> Result<CaptureWritePlan, CaptureError> {
    let target_existed = planner.currently_exists(target)?;
    let original_target = planner.current_contents(target)?.unwrap_or_default();
    reject_duplicate_block_id(&original_target, block_id, target)?;

    let (updated_target, placement) = if target_existed {
        insert_task_line(&original_target, capture_block)
    } else {
        validate_target_parent(target)?;
        (format!("{capture_block}\n"), Placement::Created)
    };

    let day_file = pomodoro::day_file_for(bob_dir);
    if !day_file.is_file() {
        return Err(CaptureError::io(format!(
            "Bob daily note does not exist: {}",
            day_file.display()
        ))
        .with_typed(CaptureFailureKind::NoDailyNote));
    }
    if paths_refer_to_same_file(target, &day_file) {
        return Err(CaptureError::io(
            "routed note and Bob daily note must be different files",
        ));
    }

    let original_day = planner.read_existing(&day_file)?;
    let block_link = format!("[[{route}#^{block_id}]]");
    if original_day.contains(&block_link) {
        return Err(CaptureError::io(format!(
            "Pomodoro ledger already contains {block_link}"
        )));
    }
    let day_file_label = day_file.display().to_string();
    // A bare new-task capture places by the new task's effective type
    // (D3, from the staged task line); a named one keeps its deliberate
    // destination and never types.
    let placement_type = if pomodoro_name.is_none() {
        let task_line = capture_block.lines().next().unwrap_or(capture_block);
        task_theme_context
            .placement_for_task(planner, route, task_line, warnings)
    } else {
        None
    };
    let placed_type = placement_type.as_ref().map(|placement| {
        let typed = placement.as_typed();
        (typed, placement.placed_type())
    });
    let typed = placed_type.as_ref().map(|(typed, _)| typed);
    if let Some(spec) = start {
        let (updated_day, pomodoro_link_placement, summary, start_before) =
            plan_pomodoro_start(
                &original_day,
                &block_link,
                pomodoro_name,
                pomodoro_name_is_literal,
                spec,
                now,
                typed,
            )?;
        planner.stage(target, updated_target)?;
        planner.stage(&day_file, updated_day)?;
        // The start moves (or creates) the entry and appends the Task
        // Link beneath it, so the entry carries both roles.
        let pomodoro_refs = PomodoroBlockRef::started_and_linked(
            start_before,
            summary.pomodoro_line.saturating_sub(1),
        )
        .into_iter()
        .collect();
        let destination = {
            let mut destination = destination_json(
                &capture_task_toggle::PomodoroEndpoint {
                    line: summary.pomodoro_line,
                    name: summary.pomodoro_name.clone(),
                    time_range: Some(summary.time_range.clone()),
                },
                summary.created_pomodoro,
                pomodoro_name,
            );
            if let Some((_, placed)) = placed_type.as_ref() {
                destination.placed_type = Some(placed.clone());
            }
            destination
        };
        return Ok(CaptureWritePlan {
            placement,
            pomodoro: Some(PlannedPomodoroEdit {
                details: PomodoroCaptureDetails {
                    block_id: block_id.to_string(),
                    day_file: day_file_label,
                    block_link,
                    pomodoro_link_placement,
                    pomodoro_name: summary.pomodoro_name.clone(),
                    creates_pomodoro: summary.created_pomodoro,
                    pomodoro_link_destination: Some(destination),
                },
                start: Some(summary),
            }),
            sub_bullet: None,
            pomodoro_note: None,
            toggle: None,
            pomodoro_link: None,
            pomodoro_refs,
        });
    }
    let insertion = insert_pomodoro_block_link(
        &original_day,
        &block_link,
        pomodoro_name,
        pomodoro_name_is_literal,
        typed,
    )?;
    planner.stage(target, updated_target)?;
    planner.stage(&day_file, insertion.updated)?;
    // The destination endpoint is post-state (scanned from the updated
    // note), so its headline needs only resolution — or `Created` — for
    // `before`. A fresh task link never moves a source.
    let pomodoro_refs = vec![if insertion.creates_pomodoro {
        PomodoroBlockRef::created(
            PomodoroBlockRole::Linked,
            insertion.destination.line.saturating_sub(1),
        )
    } else {
        PomodoroBlockRef::resolved(
            PomodoroBlockRole::Linked,
            insertion.destination.line.saturating_sub(1),
        )
    }];
    let mut destination = insertion.destination;
    if let Some((_, placed)) = placed_type.as_ref() {
        destination.placed_type = Some(placed.clone());
    }
    let placement = insertion.placement;
    let destination_name = destination.name.clone();
    let creates_pomodoro = insertion.creates_pomodoro;

    Ok(CaptureWritePlan {
        placement,
        pomodoro: Some(PlannedPomodoroEdit {
            details: PomodoroCaptureDetails {
                block_id: block_id.to_string(),
                day_file: day_file_label,
                block_link,
                pomodoro_link_placement: placement,
                pomodoro_name: destination_name,
                creates_pomodoro,
                pomodoro_link_destination: Some(destination),
            },
            start: None,
        }),
        sub_bullet: None,
        pomodoro_note: None,
        toggle: None,
        pomodoro_link: None,
        pomodoro_refs,
    })
}

pub(super) fn endpoint_json(
    endpoint: &capture_task_toggle::PomodoroEndpoint,
) -> PomodoroLinkEndpoint {
    PomodoroLinkEndpoint {
        line: endpoint.line,
        name: endpoint.name.clone(),
        time_range: endpoint.time_range.clone(),
        role: None,
        placed_type: None,
    }
}

/// Destination variant of [`endpoint_json`]: the same post-image
/// endpoint plus the plan-budget `role` (`current`/`next_up`/
/// `named`/`created`). Sources keep `role` unset.
pub(super) fn destination_json(
    endpoint: &capture_task_toggle::PomodoroEndpoint,
    creates_pomodoro: bool,
    selector: Option<&str>,
) -> PomodoroLinkEndpoint {
    PomodoroLinkEndpoint {
        line: endpoint.line,
        name: endpoint.name.clone(),
        time_range: endpoint.time_range.clone(),
        role: Some(destination_role(
            creates_pomodoro,
            selector,
            endpoint.time_range.as_deref(),
        )),
        placed_type: None,
    }
}

pub(super) fn reject_pomodoro_link_conflicts(
    parsed: &ParsedCaptureText,
    request: &CaptureRequest,
) -> Result<(), CaptureError> {
    if !request.forced_destination_flags.is_empty() {
        return Err(CaptureError::usage(format!(
            "Pomodoro link capture cannot be combined with {}",
            request.forced_destination_flags.join(", ")
        )));
    }
    if request.forced_clip.is_some() {
        return Err(CaptureError::usage(
            "Pomodoro link capture cannot be combined with --clip",
        ));
    }
    if request.forced_sub_bullet_target.is_some()
        || request.forced_task_section.is_some()
    {
        return Err(CaptureError::usage(
            "Pomodoro link capture cannot be combined with --route, --section, --task, --task-ref, --task-section, or --clip; capture the link alone",
        ));
    }
    if parsed.clip.is_some() {
        return Err(CaptureError::usage(
            "Pomodoro link capture cannot be combined with % clipboard markers",
        ));
    }
    if parsed.scheduled_offset.is_some() {
        return Err(CaptureError::usage(
            "Pomodoro link capture cannot be combined with s:<N>",
        ));
    }
    if parsed.priority_level.is_some() {
        return Err(CaptureError::usage(
            "Pomodoro link capture cannot be combined with $<N>",
        ));
    }
    if !parsed.sub_bullets.is_empty() {
        return Err(CaptureError::usage(
            "Pomodoro link capture cannot be combined with authored child bullets",
        ));
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(super) fn plan_pomodoro_link_capture(
    planner: &mut CaptureBatchPlanner,
    bob_dir: &Path,
    target: &Path,
    route: &str,
    block_id: &str,
    ledger_target: capture_task_toggle::PomodoroLinkTarget<'_>,
    start: Option<&PomodoroStartSpec>,
    now: chrono::NaiveDateTime,
    today: NaiveDate,
    warnings: &mut Vec<CaptureWarning>,
    ctx: &mut DependencyContext,
    mut task_theme_context: Option<&mut CaptureTaskThemeContext>,
) -> Result<(CaptureWritePlan, Option<LinkGuardJson>), CaptureError> {
    let contents = planner.read_existing(target)?;
    let settings = note_tasks::read_settings(bob_dir);
    let scan = note_tasks::scan(&contents, &settings);
    let task = match scan.by_block_id(block_id) {
        BlockIdLookup::Found(task) => task,
        BlockIdLookup::NotATask {
            line_index,
            excerpt,
        } => {
            return Err(CaptureError::io(format!(
                "^{block_id} in {route}.md is not a task (line {}: {excerpt})",
                line_index + 1
            )));
        }
        BlockIdLookup::Duplicate(count) => {
            return Err(CaptureError::io(format!(
                "block ID ^{block_id} appears {count} times in {route}.md; make it unique before capturing"
            )));
        }
        BlockIdLookup::Missing => {
            let choices = format!(
                "run 'bob capture-tasks -r {route}' to list task block IDs"
            );
            let hint = format!(
                "to create a new Pomodoro-linked task, add text: `@{route}:{block_id} <text>`"
            );
            let message = match scan.suggest_block_id(block_id) {
                Some(suggestion) => format!(
                    "no task with block ID ^{block_id} in {route}.md; did you mean ^{suggestion}? ({choices}; {hint})"
                ),
                None => format!(
                    "no task with block ID ^{block_id} in {route}.md ({choices}; {hint})"
                ),
            };
            return Err(CaptureError::io(message));
        }
    };
    let task_line_index = task.line_index;
    let previous_status_symbol = task.status_symbol;
    let previous_status_name = task.status_name.clone();
    let task_description = task.description.clone();
    let previous_task_line =
        line_text_at(&contents, task_line_index)?.to_string();

    if !crate::native::capture_link_tasks::is_linkable_status(
        previous_status_symbol,
    ) {
        return Err(CaptureError::io(format!(
            "task ^{block_id} is {previous_status_name}; only Ready, Blocked, Next, and In Progress tasks can be linked to a Pomodoro"
        )));
    }

    // Resolve the §13.5 guard. An eligible local dependency drop is
    // staged before the ordinary task-link write; any later failure
    // aborts the whole capture batch.
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
                let dependent_desc = format!("pomodoro link ^{block_id}");
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
                let removed_pomodoro_links = consumption.removed_links;
                append_consumption_warning(warnings, removed_pomodoro_links);
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
                        placement: Placement::Unchanged,
                        pomodoro: None,
                        sub_bullet: None,
                        pomodoro_note: None,
                        toggle: None,
                        pomodoro_link: Some(PomodoroLinkCaptureDetails {
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
                            pomodoro_link_action: "not_linked",
                            pomodoro_link_source: None,
                            pomodoro_link_destination: None,
                            removed_scheduled: None,
                            schedule_log: None,
                            status_changed: false,
                            pomodoro_start: None,
                            dropped_dependencies: Vec::new(),
                            task_block_ref: None,
                            blocks_updates: Vec::new(),
                        }),
                        pomodoro_refs: consumption.pomodoro_refs,
                    },
                    Some(link_guard),
                ));
            }
        };

    let current_contents = current_note_text(planner, target)?;
    let current_scan = note_tasks::scan(&current_contents, &settings);
    let current_task = match current_scan.by_block_id(block_id) {
        BlockIdLookup::Found(task) => task,
        _ => {
            return Err(CaptureError::io(
                "pomodoro link capture invariant failed: task disappeared after dependency drop",
            ));
        }
    };

    let task_plan =
        capture_task_toggle::plan_task_link(
            &current_contents,
            current_task.line_index,
            today,
        )
            .ok_or_else(|| {
                CaptureError::io(
                    "pomodoro link capture invariant failed: task line could not be updated",
                )
            })?;
    let task_line =
        line_text_at(&task_plan.content, current_task.line_index)?.to_string();
    let updated_scan = note_tasks::scan(&task_plan.content, &settings);
    let updated_task = match updated_scan.by_block_id(block_id) {
        BlockIdLookup::Found(task) => task,
        _ => {
            return Err(CaptureError::io(
                "pomodoro link capture invariant failed: linked task disappeared",
            ));
        }
    };
    let status_changed = previous_status_symbol != updated_task.status_symbol;
    // No `[dependsOn::` text warning here: the link guard above is
    // authoritative (see `plan_task_toggle_capture`). Typed-placement
    // suggestion warnings still report below.
    if task_plan.content != current_contents {
        planner.stage(target, task_plan.content.clone())?;
    }

    let day_file = pomodoro::day_file_for(bob_dir);
    if !day_file.is_file() && !planner.currently_exists(&day_file)? {
        return Err(CaptureError::io(format!(
            "Bob daily note does not exist: {}",
            day_file.display()
        ))
        .with_typed(CaptureFailureKind::NoDailyNote));
    }
    if paths_refer_to_same_file(target, &day_file) {
        return Err(CaptureError::io(
            "routed note and Bob daily note must be different files",
        ));
    }
    let day_contents = planner.read_existing(&day_file)?;
    let day_file_label = day_file.display().to_string();
    let block_link = format!("[[{route}#^{block_id}]]");

    let pomodoro_name = ledger_target.named_selector();
    let pomodoro_name_is_literal = matches!(
        ledger_target,
        capture_task_toggle::PomodoroLinkTarget::LiteralName(_)
    );
    // A bare link places by the task's effective type (D7). Named targets
    // keep their deliberate destination; the caller applies D6 retyping.
    // `bob ref today` uses this same context and placement path.
    let is_bare = matches!(
        ledger_target,
        capture_task_toggle::PomodoroLinkTarget::Implicit
    );
    let placement_type = if is_bare {
        match task_theme_context.as_mut() {
            Some(context) => {
                context.placement_for_task(planner, route, &task_line, warnings)
            }
            None => None,
        }
    } else {
        None
    };
    let placed_type = placement_type.as_ref().map(|placement| {
        let typed = placement.as_typed();
        (typed, placement.placed_type())
    });
    let typed = placed_type.as_ref().map(|(typed, _)| typed);
    if let Some(spec) = start {
        let (mut plan, guard) = plan_pomodoro_link_with_start(
            planner,
            &day_file,
            &day_file_label,
            &block_link,
            route,
            block_id,
            pomodoro_name,
            pomodoro_name_is_literal,
            spec,
            now,
            &day_contents,
            previous_task_line,
            task_line,
            task_description,
            previous_status_symbol,
            previous_status_name,
            updated_task.status_symbol,
            updated_task.status_name.clone(),
            status_changed,
            task_plan.removed_scheduled,
            task_plan.schedule_log,
            typed,
            placed_type.as_ref().map(|(_, placed)| placed.clone()),
        )
        .map(|plan| (plan, None))?;
        if let Some(details) = plan.pomodoro_link.as_mut() {
            details.dropped_dependencies = dropped_dependencies;
            details.task_block_ref = task_block_ref;
            details.blocks_updates = blocks_updates;
        }
        return Ok((plan, guard));
    }

    let relocation = capture_task_toggle::plan_pomodoro_link_ledger(
        &day_contents,
        &block_link,
        ledger_target,
        typed,
    )
    .map_err(|error| {
        relocation_plan_error(error, &block_link, route, block_id)
    })?;
    if relocation.has_changes {
        planner.stage(&day_file, relocation.content.clone())?;
    }
    let action = match relocation.action {
        capture_task_toggle::PomodoroLinkLedgerAction::Linked => "linked",
        capture_task_toggle::PomodoroLinkLedgerAction::Moved => "moved",
        capture_task_toggle::PomodoroLinkLedgerAction::AlreadyCurrent => {
            "already_current"
        }
    };
    let placement = relocation.placement.map(link_placement_to_placement);
    // The destination endpoint is post-state; a moved source keeps its
    // pre-state headline and resolves through the item's line map.
    // Mention-only destinations (`AlreadyCurrent`, an already-linked
    // entry) still push their ref so the block shows with every line
    // unchanged.
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
    if relocation.action == capture_task_toggle::PomodoroLinkLedgerAction::Moved
        && let Some(source) = relocation.source.as_ref()
    {
        pomodoro_refs.push(PomodoroBlockRef::unlinked_before(source.line - 1));
    }
    Ok((
        CaptureWritePlan {
            placement: Placement::Linked,
            pomodoro: None,
            sub_bullet: None,
            pomodoro_note: None,
            toggle: None,
            pomodoro_link: Some(PomodoroLinkCaptureDetails {
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
                pomodoro_link_placement: placement,
                pomodoro_name: relocation.destination.name.clone(),
                creates_pomodoro: relocation.creates_pomodoro,
                pomodoro_link_action: action,
                pomodoro_link_source: relocation
                    .source
                    .as_ref()
                    .map(endpoint_json),
                pomodoro_link_destination: Some({
                    let mut destination = destination_json(
                        &relocation.destination,
                        relocation.creates_pomodoro,
                        pomodoro_name,
                    );
                    if let Some((_, placed)) = placed_type.as_ref() {
                        destination.placed_type = Some(placed.clone());
                    }
                    destination
                }),
                removed_scheduled: task_plan.removed_scheduled,
                schedule_log: task_plan.schedule_log,
                status_changed,
                pomodoro_start: None,
                dropped_dependencies,
                task_block_ref,
                blocks_updates,
            }),
            pomodoro_refs,
        },
        None,
    ))
}

#[allow(clippy::too_many_arguments)]
pub(super) fn plan_existing_task_pomodoro_link_item(
    planner: &mut CaptureBatchPlanner,
    bob_dir: &Path,
    dry_run: bool,
    target: &Path,
    route: &str,
    block_id: &str,
    ledger_target: capture_task_toggle::PomodoroLinkTarget<'_>,
    start: Option<&PomodoroStartSpec>,
    now: chrono::NaiveDateTime,
    today: NaiveDate,
    warnings: &mut Vec<CaptureWarning>,
    ctx: &mut DependencyContext,
    mut task_theme_context: Option<&mut CaptureTaskThemeContext>,
    skip_named_retype: bool,
) -> Result<PlannedCaptureItem, CaptureError> {
    let (mut note_plan, link_guard) = plan_pomodoro_link_capture(
        planner,
        bob_dir,
        target,
        route,
        block_id,
        ledger_target,
        start,
        now,
        today,
        warnings,
        ctx,
        task_theme_context.as_deref_mut(),
    )?;
    let mut task_theme = None;
    if !skip_named_retype
        && link_guard.is_none()
        && !matches!(
            ledger_target,
            capture_task_toggle::PomodoroLinkTarget::Implicit
        )
        && let Some(context) = task_theme_context
        && let Some(link) = note_plan.pomodoro_link.as_mut()
    {
        let applied = context.apply_existing_task(
            planner,
            target,
            route,
            block_id,
            &link.task_line,
            Some(link.pomodoro_name.as_deref().unwrap_or_default()),
            None,
            warnings,
        )?;
        if let Some(applied) = applied {
            link.task_line = applied.task_line;
            task_theme = Some(applied.summary);
        }
    }
    let mut planned = planned_item_from_pomodoro_link_plan(
        note_plan, link_guard, route, target, dry_run, today,
    )?;
    planned.result.task_theme = task_theme;
    Ok(planned)
}

fn planned_item_from_pomodoro_link_plan(
    mut note_plan: CaptureWritePlan,
    link_guard: Option<LinkGuardJson>,
    route: &str,
    target: &Path,
    dry_run: bool,
    today: NaiveDate,
) -> Result<PlannedCaptureItem, CaptureError> {
    let pomodoro_refs = std::mem::take(&mut note_plan.pomodoro_refs);
    let link = note_plan.pomodoro_link.as_ref().ok_or_else(|| {
        CaptureError::io(
            "pomodoro link capture invariant failed: missing link details",
        )
    })?;
    let link_consumed = link_guard.is_some();
    let relative_target = relative_target(Some(route));
    Ok(PlannedCaptureItem {
        result: CaptureItemResult {
            task_theme: None,
            type_relocation: None,
            ok: true,
            dry_run,
            routed: true,
            route: Some(route.to_string()),
            route_label: route_label(route),
            relative_target: relative_target.to_string_lossy().into_owned(),
            target: target.display().to_string(),
            text: String::new(),
            task_line: link.task_line.clone(),
            kind: if link_consumed {
                "link_guard"
            } else {
                "pomodoro_link"
            },
            created: date_string(today),
            scheduled: None,
            priority: None,
            priority_label: None,
            placement: note_plan.placement,
            sub_bullets: Vec::new(),
            clip: None,
            schedule_log: link.schedule_log.clone(),
            block_id: Some(link.block_id.clone()),
            day_file: if link_consumed {
                link_guard.as_ref().and_then(|guard| guard.day_file.clone())
            } else {
                Some(link.day_file.clone())
            },
            block_link: (!link_consumed).then(|| link.block_link.clone()),
            pomodoro_link_placement: link.pomodoro_link_placement,
            parent_line: None,
            parent_text: None,
            parent_section: None,
            parent_status_symbol: None,
            parent_status_name: None,
            toggle_direction: None,
            previous_task_line: Some(link.previous_task_line.clone()),
            status_symbol: Some(link.status_symbol),
            status_name: Some(link.status_name.clone()),
            previous_status_symbol: Some(link.previous_status_symbol),
            previous_status_name: Some(link.previous_status_name.clone()),
            pomodoro_name: link.pomodoro_name.clone(),
            creates_pomodoro: (!link_consumed).then_some(link.creates_pomodoro),
            pomodoro_already_linked: None,
            removed_pomodoro_links: link_guard
                .as_ref()
                .and_then(|guard| guard.removed_pomodoro_links),
            removed_scheduled: link.removed_scheduled.clone(),
            pomodoro_selector_unused: None,
            toggle_behavior: None,
            status_changed: Some(link.status_changed),
            pomodoro_link_action: (!link_consumed)
                .then_some(link.pomodoro_link_action),
            pomodoro_link_source: link.pomodoro_link_source.clone(),
            pomodoro_link_destination: link.pomodoro_link_destination.clone(),
            project_note: None,
            pomodoro_start: link.pomodoro_start.clone(),
            pomodoro_adjust: None,
            pomodoro_shift: None,
            pomodoro_close: None,
            pomodoro_reset: None,
            dependency_update: None,
            consumed_links: Vec::new(),
            task_complete: None,
            toggle_task_description: Some(link.task_description.clone()),
            r#ref: None,
            link_guard,
            dropped_dependencies: link.dropped_dependencies.clone(),
            blocks_line_updates: link.blocks_updates.clone(),
            minted_dependent_block_id: None,
            dependency_updates: Vec::new(),
            minted_blocker_block_id: None,
            relation_updates: Vec::new(),
            relation_cancels: Vec::new(),
            relation_completes: Vec::new(),
            linked_from_line_updates: Vec::new(),
            minted_relation_block_ids: Vec::new(),
            mirror_skipped: Vec::new(),
            relation_label_hides: Vec::new(),
        },
        clip_plan: None,
        pomodoro_refs,
        task_block_refs: link.task_block_ref.clone().into_iter().collect(),
    })
}

#[allow(clippy::too_many_arguments)]
pub(super) fn plan_pomodoro_link_with_start(
    planner: &mut CaptureBatchPlanner,
    day_file: &Path,
    day_file_label: &str,
    block_link: &str,
    route: &str,
    block_id: &str,
    pomodoro_name: Option<&str>,
    pomodoro_name_is_literal: bool,
    spec: &PomodoroStartSpec,
    now: chrono::NaiveDateTime,
    day_contents: &str,
    previous_task_line: String,
    task_line: String,
    task_description: String,
    previous_status_symbol: char,
    previous_status_name: String,
    status_symbol: char,
    status_name: String,
    status_changed: bool,
    removed_scheduled: Option<String>,
    schedule_log: Option<capture_schedule_log::ScheduleLog>,
    typed: Option<&capture_task_toggle::TypedImplicit<'_>>,
    placed: Option<super::batch::PlacedType>,
) -> Result<CaptureWritePlan, CaptureError> {
    let (start_text, end_text, duration_minutes, time_range) =
        compute_pomodoro_start_range(now, spec)?;
    let scan = capture_pomodoros::scan(day_contents);
    if !scan.has_section {
        return Err(CaptureError::io(
            "Bob daily note has no Pomodoros section",
        ));
    }
    let timed_open: Vec<_> = scan
        .entries
        .iter()
        .filter(|entry| {
            entry.state == capture_pomodoros::PomodoroState::Open
                && entry.time_range.is_some()
        })
        .collect();
    if !timed_open.is_empty() {
        if timed_open.len() > 1 {
            return Err(CaptureError::io(
                "Bob daily note has multiple open timed Pomodoros",
            ));
        }
        let running = timed_open[0];
        let lines = line_spans(day_contents);
        let line_texts = lines.iter().map(|line| line.text).collect::<Vec<_>>();
        let section = pomodoro::pomodoros_section_range(&line_texts)
            .ok_or_else(|| {
                CaptureError::io("Bob daily note has no Pomodoros section")
            })?;
        let movable = capture_task_toggle::find_movable_task_links(
            &lines, &scan, section, block_link,
        );
        let queued_in_running =
            movable.iter().any(|link| link.owner.line == running.line);
        if queued_in_running {
            let name = running
                .name
                .clone()
                .unwrap_or_else(|| "current".to_string());
            return Err(CaptureError::io(format!(
                "Pomodoro {name} is already running; finish the current Pomodoro first (close it with `x`) or use `+N`/`-N` to adjust it or `++N`/`--N` to shift it"
            )));
        }
        return Err(CaptureError::io(
            "Bob daily note has an active timed Pomodoro; finish the current Pomodoro first (close it with `x`)",
        ));
    }

    let lines = line_spans(day_contents);
    let line_texts = lines.iter().map(|line| line.text).collect::<Vec<_>>();
    let section =
        pomodoro::pomodoros_section_range(&line_texts).ok_or_else(|| {
            CaptureError::io("Bob daily note has no Pomodoros section")
        })?;
    let movable = capture_task_toggle::find_movable_task_links(
        &lines,
        &scan,
        section.clone(),
        block_link,
    );
    let queue = match movable.as_slice() {
        [] => None,
        [_, _, ..] => {
            return Err(CaptureError::io(format!(
                "found more than one movable open Pomodoro Task Link for {block_link}; make the dedicated Task Link unique before capturing"
            )));
        }
        [source] => Some((
            endpoint_json(&capture_task_toggle::endpoint_from_entry(
                source.owner,
            )),
            source.entry_line_index,
            source.line_index,
            source.subtree_end,
        )),
    };

    if queue.is_none() {
        let (updated_day, placement, summary, start_before) =
            plan_pomodoro_start(
                day_contents,
                block_link,
                pomodoro_name,
                pomodoro_name_is_literal,
                spec,
                now,
                typed,
            )?;
        planner.stage(day_file, updated_day)?;
        let pomodoro_refs = PomodoroBlockRef::started_and_linked(
            start_before,
            summary.pomodoro_line.saturating_sub(1),
        )
        .into_iter()
        .collect();
        return Ok(CaptureWritePlan {
            placement: Placement::Linked,
            pomodoro: None,
            sub_bullet: None,
            pomodoro_note: None,
            toggle: None,
            pomodoro_link: Some(PomodoroLinkCaptureDetails {
                previous_task_line,
                task_line,
                task_description,
                previous_status_symbol,
                previous_status_name,
                status_symbol,
                status_name,
                block_id: block_id.to_string(),
                day_file: day_file_label.to_string(),
                block_link: block_link.to_string(),
                pomodoro_link_placement: Some(placement),
                pomodoro_name: summary.pomodoro_name.clone(),
                creates_pomodoro: summary.created_pomodoro,
                pomodoro_link_action: "linked",
                pomodoro_link_source: None,
                pomodoro_link_destination: Some({
                    let mut destination = destination_json(
                        &capture_task_toggle::PomodoroEndpoint {
                            line: summary.pomodoro_line,
                            name: summary.pomodoro_name.clone(),
                            time_range: Some(format!(
                                "{}-{}",
                                summary.start, summary.end
                            )),
                        },
                        summary.created_pomodoro,
                        pomodoro_name,
                    );
                    destination.placed_type = placed;
                    destination
                }),
                removed_scheduled,
                schedule_log,
                status_changed,
                pomodoro_start: Some(summary),
                dropped_dependencies: Vec::new(),
                task_block_ref: None,
                blocks_updates: Vec::new(),
            }),
            pomodoro_refs,
        });
    }

    let (source_endpoint, q_entry_index, q_line_index, q_subtree_end) =
        queue.expect("queue exists");
    if let Some(selector) = pomodoro_name {
        match capture_pomodoros::select_capture_name(
            &scan,
            selector,
            pomodoro_name_is_literal,
        ) {
            capture_pomodoros::NamedSelection::Found(entry) => {
                if !(entry.state == capture_pomodoros::PomodoroState::Open
                    && entry.placeholder
                    && entry.time_range.is_none())
                {
                    return Err(CaptureError::io(format!(
                        "selected Pomodoro `#{}` is not an untimed open placeholder; finish the current Pomodoro first or choose an untimed placeholder",
                        entry.slug
                    )));
                }
                let dest_index = entry.line - 1;
                if q_entry_index == dest_index {
                    let (started_day, moved_index) =
                        start_existing_pomodoro_entry(
                            day_contents,
                            dest_index,
                            &time_range,
                        )?;
                    planner.stage(day_file, started_day.clone())?;
                    // The queued link's own entry started: it carries
                    // both the start and the (mention-only) link ref.
                    let pomodoro_refs = PomodoroBlockRef::started_and_linked(
                        PomodoroBlockBefore::At(dest_index),
                        moved_index,
                    )
                    .into_iter()
                    .collect();
                    let after_scan = capture_pomodoros::scan(&started_day);
                    let dest_entry = after_scan
                        .entries
                        .iter()
                        .find(|candidate| candidate.line == moved_index + 1)
                        .map(capture_task_toggle::endpoint_from_entry)
                        .unwrap_or(capture_task_toggle::PomodoroEndpoint {
                            line: moved_index + 1,
                            name: entry.name.clone(),
                            time_range: None,
                        });
                    let dest_json =
                        destination_json(&dest_entry, false, pomodoro_name);
                    let summary = PomodoroStartSummary {
                        start: start_text.clone(),
                        end: end_text.clone(),
                        duration_minutes,
                        offset_units: spec.offset_units,
                        pomodoro_name: entry.name.clone(),
                        pomodoro_line: dest_json.line,
                        created_pomodoro: false,
                        time_range: time_range.clone(),
                        tasks: None,
                        drop: Vec::new(),
                        dropped: Vec::new(),
                        r#override: None,
                    };
                    return Ok(CaptureWritePlan {
                        placement: Placement::Linked,
                        pomodoro: None,
                        sub_bullet: None,
                        pomodoro_note: None,
                        toggle: None,
                        pomodoro_link: Some(PomodoroLinkCaptureDetails {
                            previous_task_line,
                            task_line,
                            task_description,
                            previous_status_symbol,
                            previous_status_name,
                            status_symbol,
                            status_name,
                            block_id: block_id.to_string(),
                            day_file: day_file_label.to_string(),
                            block_link: block_link.to_string(),
                            pomodoro_link_placement: None,
                            pomodoro_name: entry.name.clone(),
                            creates_pomodoro: false,
                            pomodoro_link_action: "already_current",
                            pomodoro_link_source: Some(PomodoroLinkEndpoint {
                                line: source_endpoint.line,
                                name: source_endpoint.name.clone(),
                                time_range: source_endpoint.time_range.clone(),
                                role: None,
                                placed_type: None,
                            }),
                            pomodoro_link_destination: Some(dest_json),
                            removed_scheduled,
                            schedule_log,
                            status_changed,
                            pomodoro_start: Some(summary),
                            dropped_dependencies: Vec::new(),
                            task_block_ref: None,
                            blocks_updates: Vec::new(),
                        }),
                        pomodoro_refs,
                    });
                }
                let (started_day, moved_dest_index) =
                    start_existing_pomodoro_entry(
                        day_contents,
                        dest_index,
                        &time_range,
                    )?;
                let working_lines = line_spans(&started_day);
                let working_texts = working_lines
                    .iter()
                    .map(|line| line.text)
                    .collect::<Vec<_>>();
                let working_section =
                    pomodoro::pomodoros_section_range(&working_texts)
                        .ok_or_else(|| {
                            CaptureError::io(
                                "Bob daily note has no Pomodoros section",
                            )
                        })?;
                let working_scan = capture_pomodoros::scan(&started_day);
                let movable = capture_task_toggle::find_movable_task_links(
                    &working_lines,
                    &working_scan,
                    working_section,
                    block_link,
                );
                let relocated = match movable.as_slice() {
                    [source] => source,
                    _ => {
                        return Err(CaptureError::io(
                            "pomodoro link capture invariant failed: queued link disappeared",
                        ));
                    }
                };
                let (moved_day, placement) =
                    capture_task_toggle::move_subtree_to_entry(
                        &started_day,
                        relocated.line_index,
                        relocated.subtree_end,
                        moved_dest_index,
                    )
                    .map_err(|error| {
                        relocation_plan_error(
                            error, block_link, route, block_id,
                        )
                    })?;
                planner.stage(day_file, moved_day.clone())?;
                let after_scan = capture_pomodoros::scan(&moved_day);
                let dest_entry = after_scan
                    .entries
                    .iter()
                    .find(|candidate| {
                        candidate.state
                            == capture_pomodoros::PomodoroState::Open
                            && candidate.name == entry.name
                    })
                    .map(capture_task_toggle::endpoint_from_entry)
                    .unwrap_or(capture_task_toggle::PomodoroEndpoint {
                        line: moved_dest_index + 1,
                        name: entry.name.clone(),
                        time_range: Some(format!("{start_text}-{end_text}")),
                    });
                let dest_json =
                    destination_json(&dest_entry, false, pomodoro_name);
                // The named entry started and moved, and the queued link
                // moved into it: the destination carries both roles while
                // the source resolves through the item's line map.
                let mut pomodoro_refs = PomodoroBlockRef::started_and_linked(
                    PomodoroBlockBefore::At(dest_index),
                    dest_json.line.saturating_sub(1),
                )
                .into_iter()
                .collect::<Vec<_>>();
                pomodoro_refs.push(PomodoroBlockRef::unlinked_before(
                    source_endpoint.line.saturating_sub(1),
                ));
                let summary = PomodoroStartSummary {
                    start: start_text,
                    end: end_text,
                    duration_minutes,
                    offset_units: spec.offset_units,
                    pomodoro_name: entry.name.clone(),
                    pomodoro_line: dest_json.line,
                    created_pomodoro: false,
                    time_range: time_range.clone(),
                    tasks: None,
                    drop: Vec::new(),
                    dropped: Vec::new(),
                    r#override: None,
                };
                return Ok(CaptureWritePlan {
                    placement: Placement::Linked,
                    pomodoro: None,
                    sub_bullet: None,
                    pomodoro_note: None,
                    toggle: None,
                    pomodoro_link: Some(PomodoroLinkCaptureDetails {
                        previous_task_line,
                        task_line,
                        task_description,
                        previous_status_symbol,
                        previous_status_name,
                        status_symbol,
                        status_name,
                        block_id: block_id.to_string(),
                        day_file: day_file_label.to_string(),
                        block_link: block_link.to_string(),
                        pomodoro_link_placement: Some(
                            link_placement_to_placement(placement),
                        ),
                        pomodoro_name: entry.name.clone(),
                        creates_pomodoro: false,
                        pomodoro_link_action: "moved",
                        pomodoro_link_source: Some(PomodoroLinkEndpoint {
                            line: source_endpoint.line,
                            name: source_endpoint.name.clone(),
                            time_range: source_endpoint.time_range.clone(),
                            role: None,
                            placed_type: None,
                        }),
                        pomodoro_link_destination: Some(dest_json),
                        removed_scheduled,
                        schedule_log,
                        status_changed,
                        pomodoro_start: Some(summary),
                        dropped_dependencies: Vec::new(),
                        task_block_ref: None,
                        blocks_updates: Vec::new(),
                    }),
                    pomodoro_refs,
                });
            }
            capture_pomodoros::NamedSelection::CompletedOnly(_)
            | capture_pomodoros::NamedSelection::Missing { .. } => {
                let (with_placeholder, created_line, name) =
                    capture_task_toggle::insert_named_placeholder(
                        day_contents,
                        selector,
                        pomodoro_name_is_literal,
                    )
                    .map_err(|error| {
                        relocation_plan_error(
                            error, block_link, route, block_id,
                        )
                    })?;
                let (started_day, _) = replace_placeholder_range(
                    &with_placeholder,
                    created_line,
                    &time_range,
                )?;
                let working_lines = line_spans(&started_day);
                let working_texts = working_lines
                    .iter()
                    .map(|line| line.text)
                    .collect::<Vec<_>>();
                let working_section =
                    pomodoro::pomodoros_section_range(&working_texts)
                        .ok_or_else(|| {
                            CaptureError::io(
                                "Bob daily note has no Pomodoros section",
                            )
                        })?;
                let working_scan = capture_pomodoros::scan(&started_day);
                let movable = capture_task_toggle::find_movable_task_links(
                    &working_lines,
                    &working_scan,
                    working_section,
                    block_link,
                );
                let relocated = match movable.as_slice() {
                    [source] => source,
                    _ => {
                        return Err(CaptureError::io(
                            "pomodoro link capture invariant failed: queued link disappeared",
                        ));
                    }
                };
                let (moved_day, placement) =
                    capture_task_toggle::move_subtree_to_entry(
                        &started_day,
                        relocated.line_index,
                        relocated.subtree_end,
                        created_line,
                    )
                    .map_err(|error| {
                        relocation_plan_error(
                            error, block_link, route, block_id,
                        )
                    })?;
                planner.stage(day_file, moved_day.clone())?;
                let after_scan = capture_pomodoros::scan(&moved_day);
                let dest_entry = after_scan
                    .entries
                    .iter()
                    .find(|candidate| {
                        candidate.state
                            == capture_pomodoros::PomodoroState::Open
                            && candidate.name.as_deref() == Some(name.as_str())
                    })
                    .map(capture_task_toggle::endpoint_from_entry)
                    .unwrap_or(capture_task_toggle::PomodoroEndpoint {
                        line: created_line + 1,
                        name: Some(name.clone()),
                        time_range: Some(format!("{start_text}-{end_text}")),
                    });
                let dest_json =
                    destination_json(&dest_entry, true, pomodoro_name);
                // The named entry was created and started, and the queued
                // link moved into it.
                let mut pomodoro_refs = PomodoroBlockRef::started_and_linked(
                    PomodoroBlockBefore::Created,
                    dest_json.line.saturating_sub(1),
                )
                .into_iter()
                .collect::<Vec<_>>();
                pomodoro_refs.push(PomodoroBlockRef::unlinked_before(
                    source_endpoint.line.saturating_sub(1),
                ));
                let summary = PomodoroStartSummary {
                    start: start_text,
                    end: end_text,
                    duration_minutes,
                    offset_units: spec.offset_units,
                    pomodoro_name: Some(name.clone()),
                    pomodoro_line: dest_json.line,
                    created_pomodoro: true,
                    time_range: time_range.clone(),
                    tasks: None,
                    drop: Vec::new(),
                    dropped: Vec::new(),
                    r#override: None,
                };
                return Ok(CaptureWritePlan {
                    placement: Placement::Linked,
                    pomodoro: None,
                    sub_bullet: None,
                    pomodoro_note: None,
                    toggle: None,
                    pomodoro_link: Some(PomodoroLinkCaptureDetails {
                        previous_task_line,
                        task_line,
                        task_description,
                        previous_status_symbol,
                        previous_status_name,
                        status_symbol,
                        status_name,
                        block_id: block_id.to_string(),
                        day_file: day_file_label.to_string(),
                        block_link: block_link.to_string(),
                        pomodoro_link_placement: Some(
                            link_placement_to_placement(placement),
                        ),
                        pomodoro_name: Some(name),
                        creates_pomodoro: true,
                        pomodoro_link_action: "moved",
                        pomodoro_link_source: Some(PomodoroLinkEndpoint {
                            line: source_endpoint.line,
                            name: source_endpoint.name.clone(),
                            time_range: source_endpoint.time_range.clone(),
                            role: None,
                            placed_type: None,
                        }),
                        pomodoro_link_destination: Some(dest_json),
                        removed_scheduled,
                        schedule_log,
                        status_changed,
                        pomodoro_start: Some(summary),
                        dropped_dependencies: Vec::new(),
                        task_block_ref: None,
                        blocks_updates: Vec::new(),
                    }),
                    pomodoro_refs,
                });
            }
        }
    }

    let q_scan_entry = scan
        .entries
        .iter()
        .find(|entry| entry.line - 1 == q_entry_index)
        .ok_or_else(|| {
            CaptureError::io(
                "pomodoro link capture invariant failed: queued Pomodoro disappeared",
            )
        })?;
    if !(q_scan_entry.placeholder && q_scan_entry.time_range.is_none()) {
        let suggestion = q_scan_entry
            .name
            .as_deref()
            .map(|name| format!("#{name}=<X>"))
            .unwrap_or_else(|| "#<pomodoro>=<X>".to_string());
        return Err(CaptureError::io(format!(
            "selected Pomodoro is not an untimed open placeholder; use `{suggestion}` to start a specific Pomodoro"
        )));
    }
    let (started_day, moved_index) = start_existing_pomodoro_entry(
        day_contents,
        q_entry_index,
        &time_range,
    )?;
    planner.stage(day_file, started_day.clone())?;
    let after_scan = capture_pomodoros::scan(&started_day);
    let dest_entry = after_scan
        .entries
        .iter()
        .find(|candidate| candidate.line - 1 == moved_index)
        .map(capture_task_toggle::endpoint_from_entry)
        .unwrap_or(capture_task_toggle::PomodoroEndpoint {
            line: moved_index + 1,
            name: q_scan_entry.name.clone(),
            time_range: Some(format!("{start_text}-{end_text}")),
        });
    let dest_json = destination_json(&dest_entry, false, pomodoro_name);
    let summary = PomodoroStartSummary {
        start: start_text,
        end: end_text,
        duration_minutes,
        offset_units: spec.offset_units,
        pomodoro_name: q_scan_entry.name.clone(),
        pomodoro_line: dest_json.line,
        created_pomodoro: false,
        time_range: time_range.clone(),
        tasks: None,
        drop: Vec::new(),
        dropped: Vec::new(),
        r#override: None,
    };
    let _ = (q_line_index, q_subtree_end);
    // The queued entry started in place and already holds the link: the
    // destination carries both the start and the (mention-only) link ref.
    let pomodoro_refs = PomodoroBlockRef::started_and_linked(
        PomodoroBlockBefore::At(q_entry_index),
        moved_index,
    )
    .into_iter()
    .collect();
    Ok(CaptureWritePlan {
        placement: Placement::Linked,
        pomodoro: None,
        sub_bullet: None,
        pomodoro_note: None,
        toggle: None,
        pomodoro_link: Some(PomodoroLinkCaptureDetails {
            previous_task_line,
            task_line,
            task_description,
            previous_status_symbol,
            previous_status_name,
            status_symbol,
            status_name,
            block_id: block_id.to_string(),
            day_file: day_file_label.to_string(),
            block_link: block_link.to_string(),
            pomodoro_link_placement: None,
            pomodoro_name: q_scan_entry.name.clone(),
            creates_pomodoro: false,
            pomodoro_link_action: "already_current",
            pomodoro_link_source: Some(PomodoroLinkEndpoint {
                line: source_endpoint.line,
                name: source_endpoint.name.clone(),
                time_range: source_endpoint.time_range.clone(),
                role: None,
                placed_type: None,
            }),
            pomodoro_link_destination: Some(dest_json),
            removed_scheduled,
            schedule_log,
            status_changed,
            pomodoro_start: Some(summary),
            dropped_dependencies: Vec::new(),
            task_block_ref: None,
            blocks_updates: Vec::new(),
        }),
        pomodoro_refs,
    })
}

pub(super) fn relocation_plan_error(
    error: capture_task_toggle::LinkRelocationError,
    block_link: &str,
    route: &str,
    block_id: &str,
) -> CaptureError {
    match error {
        capture_task_toggle::LinkRelocationError::NoPomodorosSection => {
            link_plan_error(capture_task_toggle::LinkPlanError::NoPomodorosSection)
        }
        capture_task_toggle::LinkRelocationError::NoEligibleOpenEntry => {
            link_plan_error(capture_task_toggle::LinkPlanError::NoEligibleOpenEntry)
        }
        capture_task_toggle::LinkRelocationError::MultipleOpenTimedEntries => {
            link_plan_error(
                capture_task_toggle::LinkPlanError::MultipleOpenTimedEntries,
            )
        }
        capture_task_toggle::LinkRelocationError::InvalidPomodoroName => {
            link_plan_error(capture_task_toggle::LinkPlanError::InvalidPomodoroName)
        }
        capture_task_toggle::LinkRelocationError::StrictTypeCap { name, count, cap } => {
            CaptureError::strict_type_cap(&name, count, cap)
        }
        capture_task_toggle::LinkRelocationError::NoMovableLink => CaptureError::io(format!(
            "no movable open Pomodoro Task Link for {block_link}; use @{route}+{block_id}! to run the explicit toggle and add one"
        )).with_typed(CaptureFailureKind::LedgerInvariant),
        capture_task_toggle::LinkRelocationError::MultipleMovableLinks => {
            CaptureError::io(format!(
                "found more than one movable open Pomodoro Task Link for {block_link}; make the dedicated Task Link unique before capturing"
            )).with_typed(CaptureFailureKind::LedgerInvariant)
        }
        capture_task_toggle::LinkRelocationError::StalePomodoro => {
            CaptureError::io(
                "Pomodoro ref no longer matches an open entry; list again and retry",
            )
            .with_typed(CaptureFailureKind::StalePomodoro)
        }
        capture_task_toggle::LinkRelocationError::PomodoroClosed => {
            CaptureError::io("Pomodoro ref names a completed entry")
                .with_typed(CaptureFailureKind::PomodoroClosed)
        }
    }
}

pub(super) fn line_text_at(
    contents: &str,
    line_index: usize,
) -> Result<&str, CaptureError> {
    line_spans(contents)
        .get(line_index)
        .map(|line| line.text)
        .ok_or_else(|| {
            CaptureError::io(
                "task toggle capture invariant failed: task line index is out of range",
            )
        })
}

pub(super) fn link_plan_error(
    error: capture_task_toggle::LinkPlanError,
) -> CaptureError {
    match error {
        capture_task_toggle::LinkPlanError::NoPomodorosSection => {
            CaptureError::io("Bob daily note has no Pomodoros section")
                .with_typed(CaptureFailureKind::NoPomodorosSection)
        }
        capture_task_toggle::LinkPlanError::NoEligibleOpenEntry => {
            CaptureError::io("Bob daily note has no eligible open Pomodoro")
                .with_typed(CaptureFailureKind::NoOpenPomodoro)
        }
        capture_task_toggle::LinkPlanError::MultipleOpenTimedEntries => {
            CaptureError::io("Bob daily note has multiple open timed Pomodoros")
                .with_typed(CaptureFailureKind::MultipleRunning)
        }
        capture_task_toggle::LinkPlanError::InvalidPomodoroName => {
            CaptureError::usage(capture_pomodoros::POMODORO_NAME_USAGE)
                .with_typed(CaptureFailureKind::InvalidPomodoroName)
        }
        capture_task_toggle::LinkPlanError::StrictTypeCap {
            name,
            count,
            cap,
        } => CaptureError::strict_type_cap(&name, count, cap),
    }
}

/// Map a D7 typed-placement failure to the capture error contract:
/// the passing ledger errors stay byte-identical and a strict-cap refusal
/// carries the `strict_cap` code with the teaching message.
pub(super) fn typed_implicit_error(
    error: capture_task_toggle::TypedImplicitError,
) -> CaptureError {
    use capture_task_toggle::TypedImplicitError as Typed;
    match error {
        Typed::NoPomodorosSection => {
            CaptureError::io("Bob daily note has no Pomodoros section")
        }
        Typed::NoEligibleOpenEntry => {
            CaptureError::io("Bob daily note has no eligible open Pomodoro")
        }
        Typed::MultipleOpenTimedEntries => {
            CaptureError::io("Bob daily note has multiple open timed Pomodoros")
        }
        Typed::StrictTypeCap { name, count, cap } => {
            CaptureError::strict_type_cap(&name, count, cap)
        }
    }
}

pub(super) fn link_placement_to_placement(
    placement: capture_task_toggle::LinkPlacement,
) -> Placement {
    match placement {
        capture_task_toggle::LinkPlacement::Inserted => Placement::Inserted,
        capture_task_toggle::LinkPlacement::Appended => Placement::Appended,
    }
}
