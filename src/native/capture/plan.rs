//! Batch/item planning and capture formatting helpers.
use super::*;

pub(super) struct PlannedCaptureBatch {
    pub(super) items: Vec<PlannedCaptureItem>,
    pub(super) text_files: Vec<StagedTextFile>,
    pub(super) global_destination: Option<GlobalDestinationSummary>,
    pub(super) warnings: Vec<String>,
    pub(super) plan_budget: Option<CapturePlanBudget>,
    pub(super) pomodoro_blocks: Vec<PomodoroBlockJson>,
    pub(super) task_blocks: Vec<TaskBlockJson>,
}

pub(super) struct PlannedCaptureItem {
    pub(super) result: CaptureItemResult,
    pub(super) clip_plan: Option<capture_clip::ClipPlan>,
    /// Pomodoro block touches this item reported. The batch loop feeds
    /// them to the block tracker; this field is never serialized.
    pub(super) pomodoro_refs: Vec<PomodoroBlockRef>,
    /// Parent-task touches this item reported. The batch loop feeds them
    /// to the task tracker; this field is never serialized.
    pub(super) task_block_refs: Vec<TaskBlockRef>,
}

pub(super) fn plan_capture_batch(
    request: &CaptureRequest,
) -> Result<PlannedCaptureBatch, CaptureError> {
    let parse_clip_markers = request.forced_clip.is_none() && !request.no_clip;
    let parsed_draft = parse_capture_draft_with_clip_control(
        &request.raw_text,
        request.forced_route.as_deref(),
        request.forced_section.as_deref(),
        parse_clip_markers,
    )?;
    if parsed_draft.global.is_some()
        && !request.forced_destination_flags.is_empty()
    {
        return Err(CaptureError::usage(competing_destination_error(
            &request.forced_destination_flags,
        )));
    }
    let parsed_items = parsed_draft.items;
    let mut warnings = parsed_draft.warnings;
    let global_destination =
        parsed_draft.global.map(|global| GlobalDestinationSummary {
            mode: global.mode_label(),
            route: global.route,
            block_id: global.block_id,
        });
    let now = bob_env::current_datetime();
    let today = now.date();
    let roll_seed = config::roll_seed();
    let mut planner = CaptureBatchPlanner::default();
    let mut clip_reservations = capture_clip::ClipReservations::default();
    let mut items = Vec::new();
    let day_file = pomodoro::day_file_for(&request.bob_dir);
    let day_relative =
        capture_pomodoros::relative_day_file(&day_file, &request.bob_dir);
    let mut block_tracker = PomodoroBlockTracker::new(day_relative);
    let task_settings = note_tasks::read_settings(&request.bob_dir);
    let mut task_tracker = TaskBlockTracker::new(task_settings);
    let mut dependency_ctx = DependencyContext::new(&request.bob_dir, today);

    for parsed_item in parsed_items {
        let item_number = parsed_item.index + 1;
        let line_start = parsed_item.line_start;
        let pre_day = planner.peek_text(&day_file);
        let pre_tasks = task_tracker.snapshot_pre(&planner);
        let mut planned = plan_capture_item(
            request,
            parsed_item,
            now,
            today,
            roll_seed,
            &mut planner,
            &mut clip_reservations,
            &mut warnings,
            &mut dependency_ctx,
        )
        .map_err(|mut error| {
            error.message = format!(
                "capture item {item_number} starting on line {line_start}: {}",
                error.message
            );
            error
        })?;
        let refs = std::mem::take(&mut planned.pomodoro_refs);
        match planner.loaded_texts(&day_file) {
            Some((original, current)) => {
                let pre = pre_day.unwrap_or(original);
                block_tracker.track_item(Some(&pre), Some(&current), refs);
            }
            None => {
                debug_assert!(
                    refs.is_empty(),
                    "pomodoro block refs without a loaded day file"
                );
            }
        }
        let task_refs = std::mem::take(&mut planned.task_block_refs);
        task_tracker.track_item(&planner, &pre_tasks, task_refs);
        items.push(planned);
    }

    let final_day = planner.peek_text(&day_file);
    let pomodoro_blocks = block_tracker.finish(final_day.as_deref());
    let task_blocks = task_tracker.finish(&planner);
    Ok(PlannedCaptureBatch {
        items,
        text_files: planner.into_staged_files(),
        global_destination,
        warnings,
        plan_budget: None,
        pomodoro_blocks,
        task_blocks,
    })
}

pub(super) fn plan_capture_item(
    request: &CaptureRequest,
    parsed_item: ParsedCaptureItem,
    now: chrono::NaiveDateTime,
    today: NaiveDate,
    roll_seed: u64,
    planner: &mut CaptureBatchPlanner,
    clip_reservations: &mut capture_clip::ClipReservations,
    warnings: &mut Vec<String>,
    dependency_ctx: &mut DependencyContext,
) -> Result<PlannedCaptureItem, CaptureError> {
    let mut parsed = parsed_item.parsed;
    // A dependency-only capture updates an explicitly selected existing
    // task: no new task, empty child, link toggle, or Pomodoro action.
    // It never flows through the toggle/link planners below.
    if let Some(target) = parsed.dependency_target.clone()
        && target.kind == ParsedDependencyTargetKind::ExistingTask
        && matches!(
            parsed.kind,
            CaptureKind::TaskToggle { .. } | CaptureKind::PomodoroLink { .. }
        )
    {
        return plan_dependency_only_item(
            request,
            &parsed,
            &target,
            planner,
            warnings,
            dependency_ctx,
        );
    }
    // Temporary planner arm: `!note:block-id` parses but its writer has
    // not landed yet, so every complete token refuses here. `execute`
    // replaces this arm with the staged batch writer.
    if matches!(parsed.kind, CaptureKind::TaskComplete { .. }) {
        return Err(CaptureError::usage(
            crate::native::capture_language::BANG_EXECUTE_PENDING_ERROR,
        ));
    }
    if let CaptureKind::PomodoroClose { spec } = parsed.kind.clone() {
        return plan_pomodoro_close_item(
            request, parsed, spec, now, today, planner, warnings,
        );
    }
    if let CaptureKind::PomodoroStart {
        spec,
        pomodoro_name,
    } = parsed.kind.clone()
    {
        return plan_pomodoro_start_item(
            request,
            parsed,
            spec,
            pomodoro_name,
            now,
            today,
            planner,
            warnings,
        );
    }
    if let CaptureKind::PomodoroAdjust { spec } = parsed.kind.clone() {
        return plan_pomodoro_adjust_item(
            request, parsed, spec, today, planner,
        );
    }
    if let CaptureKind::PomodoroShift { spec } = parsed.kind.clone() {
        return plan_pomodoro_shift_item(request, parsed, spec, today, planner);
    }
    if let Some(target) = request.forced_sub_bullet_target.as_ref() {
        parsed.kind = CaptureKind::SubBullet {
            target: target.clone(),
            section: None,
        };
        // A forced sub-bullet parent owns the item's modifiers like an
        // explicit marker does; a picker-ref target cannot.
        if !parsed.dependencies.is_empty() {
            match target {
                SubBulletTarget::BlockId(block_id) => {
                    let Some(route) = parsed.route.clone() else {
                        return Err(CaptureError::usage(
                            "task dependencies with --task need --route for the dependent note",
                        ));
                    };
                    parsed.dependency_target = Some(ParsedDependencyTarget {
                        kind: ParsedDependencyTargetKind::ExistingTask,
                        route: Some(route),
                        block_id: Some(block_id.clone()),
                        inherited: false,
                    });
                }
                SubBulletTarget::Ref { .. } => {
                    return Err(CaptureError::usage(
                        "task dependencies cannot attach to a --task-ref picker selection",
                    ));
                }
            }
        }
    }
    if let Some(title) = request.forced_task_section.as_ref() {
        match &mut parsed.kind {
            CaptureKind::SubBullet { section, .. } => {
                *section = Some(TaskSectionSelector {
                    text: title.clone(),
                    exact: true,
                });
            }
            _ => {
                return Err(CaptureError::usage(
                    "--task-section requires --task or --task-ref",
                ));
            }
        }
    }
    if let Some(clip) = request.forced_clip.as_ref() {
        parsed.clip = Some(clip.clone());
    }
    if let CaptureKind::TaskToggle {
        block_id,
        pomodoro_name,
        intent,
    } = &parsed.kind
    {
        reject_task_toggle_conflicts(&parsed, request)?;
        let route = parsed.route.as_deref().ok_or_else(|| {
            CaptureError::io(
                "task toggle capture invariant failed: route is missing",
            )
        })?;
        let created = date_string(today);
        let relative_target = relative_target(Some(route));
        let target = request.bob_dir.join(&relative_target);
        let mut note_plan = plan_task_toggle_capture(
            planner,
            &request.bob_dir,
            &target,
            route,
            block_id,
            pomodoro_name.as_deref(),
            *intent,
            today,
            warnings,
        )?;
        let pomodoro_refs = std::mem::take(&mut note_plan.pomodoro_refs);
        let toggle = note_plan.toggle.as_ref().ok_or_else(|| {
            CaptureError::io(
                "task toggle capture invariant failed: missing toggle details",
            )
        })?;
        return Ok(PlannedCaptureItem {
            result: CaptureItemResult {
                ok: true,
                dry_run: request.dry_run,
                routed: true,
                route: Some(route.to_string()),
                route_label: route_label(route),
                relative_target: relative_target.to_string_lossy().into_owned(),
                target: target.display().to_string(),
                text: String::new(),
                task_line: toggle.task_line.clone(),
                kind: capture_kind_label(&parsed.kind),
                created,
                scheduled: None,
                priority: None,
                priority_label: None,
                placement: note_plan.placement,
                sub_bullets: Vec::new(),
                clip: None,
                schedule_log: toggle.schedule_log.clone(),
                block_id: Some(toggle.block_id.clone()),
                day_file: Some(toggle.day_file.clone()),
                block_link: Some(toggle.block_link.clone()),
                pomodoro_link_placement: toggle.pomodoro_link_placement,
                parent_line: None,
                parent_text: None,
                parent_section: None,
                parent_status_symbol: None,
                parent_status_name: None,
                toggle_direction: Some(toggle.direction.label()),
                previous_task_line: Some(toggle.previous_task_line.clone()),
                status_symbol: Some(toggle.status_symbol),
                status_name: Some(toggle.status_name.clone()),
                previous_status_symbol: Some(toggle.previous_status_symbol),
                previous_status_name: Some(toggle.previous_status_name.clone()),
                pomodoro_name: toggle.pomodoro_name.clone(),
                creates_pomodoro: Some(toggle.creates_pomodoro),
                pomodoro_already_linked: Some(toggle.pomodoro_already_linked),
                removed_pomodoro_links: Some(toggle.removed_pomodoro_links),
                removed_scheduled: toggle.removed_scheduled.clone(),
                pomodoro_selector_unused: Some(toggle.pomodoro_selector_unused),
                toggle_behavior: toggle.toggle_behavior,
                status_changed: toggle.status_changed,
                pomodoro_link_action: toggle.pomodoro_link_action,
                pomodoro_link_source: toggle.pomodoro_link_source.clone(),
                pomodoro_link_destination: toggle
                    .pomodoro_link_destination
                    .clone(),
                project_note: None,
                pomodoro_start: None,
                pomodoro_adjust: None,
                pomodoro_shift: None,
                pomodoro_close: None,
                dependency_update: None,
                toggle_task_description: Some(toggle.task_description.clone()),
            },
            clip_plan: None,
            pomodoro_refs,
            task_block_refs: Vec::new(),
        });
    }
    if let CaptureKind::PomodoroLink {
        block_id,
        pomodoro_name,
        start,
        close,
        ..
    } = &parsed.kind.clone()
    {
        if let Some(close_spec) = close.clone() {
            return plan_pomodoro_close_link_item(
                request, parsed, now, today, planner, warnings, block_id,
                close_spec,
            );
        }
        reject_pomodoro_link_conflicts(&parsed, request)?;
        let route = parsed.route.as_deref().ok_or_else(|| {
            CaptureError::io(
                "pomodoro link capture invariant failed: route is missing",
            )
        })?;
        let created = date_string(today);
        let relative_target = relative_target(Some(route));
        let target = request.bob_dir.join(&relative_target);
        let mut note_plan = plan_pomodoro_link_capture(
            planner,
            &request.bob_dir,
            &target,
            route,
            block_id,
            pomodoro_name.as_deref(),
            start.as_ref(),
            now,
            today,
            warnings,
        )?;
        let pomodoro_refs = std::mem::take(&mut note_plan.pomodoro_refs);
        let link = note_plan.pomodoro_link.as_ref().ok_or_else(|| {
            CaptureError::io(
                "pomodoro link capture invariant failed: missing link details",
            )
        })?;
        return Ok(PlannedCaptureItem {
            result: CaptureItemResult {
                ok: true,
                dry_run: request.dry_run,
                routed: true,
                route: Some(route.to_string()),
                route_label: route_label(route),
                relative_target: relative_target.to_string_lossy().into_owned(),
                target: target.display().to_string(),
                text: String::new(),
                task_line: link.task_line.clone(),
                kind: capture_kind_label(&parsed.kind),
                created,
                scheduled: None,
                priority: None,
                priority_label: None,
                placement: note_plan.placement,
                sub_bullets: Vec::new(),
                clip: None,
                schedule_log: link.schedule_log.clone(),
                block_id: Some(link.block_id.clone()),
                day_file: Some(link.day_file.clone()),
                block_link: Some(link.block_link.clone()),
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
                creates_pomodoro: Some(link.creates_pomodoro),
                pomodoro_already_linked: None,
                removed_pomodoro_links: None,
                removed_scheduled: link.removed_scheduled.clone(),
                pomodoro_selector_unused: None,
                toggle_behavior: None,
                status_changed: Some(link.status_changed),
                pomodoro_link_action: Some(link.pomodoro_link_action),
                pomodoro_link_source: link.pomodoro_link_source.clone(),
                pomodoro_link_destination: link
                    .pomodoro_link_destination
                    .clone(),
                project_note: None,
                pomodoro_start: link.pomodoro_start.clone(),
                pomodoro_adjust: None,
                pomodoro_shift: None,
                pomodoro_close: None,
                dependency_update: None,
                toggle_task_description: Some(link.task_description.clone()),
            },
            clip_plan: None,
            pomodoro_refs,
            task_block_refs: Vec::new(),
        });
    }
    if matches!(parsed.kind, CaptureKind::ProjectNote { .. }) {
        return plan_project_note_item(
            request,
            parsed,
            now,
            today,
            roll_seed,
            parsed_item.index,
            planner,
        );
    }
    let created = date_string(today);
    let priority = match parsed.priority_level {
        Some(number) => Some(resolve_priority(
            number,
            parsed.scheduled_offset,
            item_roll_seed(roll_seed, parsed_item.index),
        )?),
        None => None,
    };
    let priority_field = priority
        .as_ref()
        .map(|resolved| (resolved.name.as_str(), resolved.value.as_str()));
    let schedule_log_reason = priority.as_ref().and_then(|resolved| {
        resolved.rolled_offset.map(|rolled_days| {
            capture_schedule_log::priority_roll_reason(
                capture_schedule_log::IMPLICIT_LEVEL_LABEL,
                &resolved.label,
                rolled_days,
                resolved.min_days,
                resolved.max_days,
            )
        })
    });
    let scheduled_offset = parsed.scheduled_offset.or_else(|| {
        priority
            .as_ref()
            .and_then(|resolved| resolved.rolled_offset)
    });
    let scheduled = scheduled_offset
        .map(|offset| scheduled_date_string(today, offset))
        .transpose()?;
    let capture_line = match &parsed.kind {
        CaptureKind::Task => format_task_line(
            &parsed.body,
            &created,
            priority_field,
            scheduled.as_deref(),
        ),
        CaptureKind::TaskWithBlockId { block_id } => {
            format_task_with_block_id_line(
                &parsed.body,
                &created,
                priority_field,
                scheduled.as_deref(),
                block_id,
            )
        }
        CaptureKind::Bullet { .. } => format_bullet_line(
            &parsed.body,
            &created,
            priority_field,
            scheduled.as_deref(),
        ),
        CaptureKind::SubBullet { .. } => format_sub_bullet_line(
            &parsed.body,
            priority_field,
            scheduled.as_deref(),
        ),
        CaptureKind::Pomodoro { block_id, .. } => format_pomodoro_task_line(
            &parsed.body,
            &created,
            priority_field,
            scheduled.as_deref(),
            block_id,
        ),
        CaptureKind::PomodoroNote => {
            format_sub_bullet_line(&parsed.body, None, None)
        }
        CaptureKind::ProjectNote { .. } => {
            unreachable!(
                "project-note capture is planned by plan_project_note_item"
            )
        }
        CaptureKind::TaskToggle { .. } => {
            unreachable!("task toggle capture is rejected before this point")
        }
        CaptureKind::PomodoroAdjust { .. } => {
            unreachable!(
                "pomodoro adjustment capture is planned before this point"
            )
        }
        CaptureKind::PomodoroShift { .. } => {
            unreachable!("pomodoro shift capture is planned before this point")
        }
        CaptureKind::PomodoroLink { .. } => {
            unreachable!("pomodoro link capture is planned before this point")
        }
        CaptureKind::PomodoroClose { .. } => {
            unreachable!("pomodoro close capture is planned before this point")
        }
        CaptureKind::PomodoroStart { .. } => {
            unreachable!("pomodoro start capture is planned before this point")
        }
        CaptureKind::TaskComplete { .. } => {
            unreachable!("task complete capture is planned before this point")
        }
    };
    let kind_label = capture_kind_label(&parsed.kind);
    let task_block_id = match &parsed.kind {
        CaptureKind::TaskWithBlockId { block_id } => Some(block_id.clone()),
        _ => None,
    };
    let (relative_target, target) = match &parsed.kind {
        CaptureKind::PomodoroNote => {
            let day_file = pomodoro::day_file_for(&request.bob_dir);
            let relative_target = day_file
                .strip_prefix(&request.bob_dir)
                .map(Path::to_path_buf)
                .unwrap_or_else(|_| day_file.clone());
            (relative_target, day_file)
        }
        _ => {
            let relative_target = relative_target(parsed.route.as_deref());
            let target = request.bob_dir.join(&relative_target);
            (relative_target, target)
        }
    };
    let child_indent = (parsed.clip.is_some()
        || schedule_log_reason.is_some()
        || !parsed.sub_bullets.is_empty())
    .then(|| child_indent_unit(planner, &target))
    .transpose()?;
    let sub_bullet_lines: Vec<String> = if parsed.sub_bullets.is_empty() {
        Vec::new()
    } else {
        let indent = child_indent.as_deref().unwrap_or("\t");
        render_authored_sub_bullets(&parsed.sub_bullets, indent)
    };
    let clip_plan = match parsed.clip.as_ref() {
        Some(ClipRequest::Current { header }) => {
            let clipboard =
                capture_clip::read_clipboard().map_err(CaptureError::io)?;
            Some(
                capture_clip::plan_with_reservations(
                    &request.bob_dir,
                    header.as_deref(),
                    &clipboard,
                    now,
                    child_indent.as_deref().unwrap_or("\t"),
                    clip_reservations,
                )
                .map_err(CaptureError::io)?,
            )
        }
        Some(ClipRequest::History { count }) if count.get() == 1 => {
            let clipboard =
                capture_clip::read_clipboard().map_err(CaptureError::io)?;
            Some(
                capture_clip::plan_with_reservations(
                    &request.bob_dir,
                    None,
                    &clipboard,
                    now,
                    child_indent.as_deref().unwrap_or("\t"),
                    clip_reservations,
                )
                .map_err(CaptureError::io)?,
            )
        }
        Some(ClipRequest::History { count }) => {
            let clipboards = capture_clip::read_clipboard_history(count.get())
                .map_err(CaptureError::io)?;
            Some(
                capture_clip::plan_history_with_reservations(
                    &request.bob_dir,
                    &clipboards,
                    now,
                    child_indent.as_deref().unwrap_or("\t"),
                    clip_reservations,
                )
                .map_err(CaptureError::io)?,
            )
        }
        None => None,
    };
    let clip_output = clip_plan.as_ref().map(|plan| plan.output.clone());
    let schedule_log = schedule_log_reason.and_then(|reason| {
        scheduled.as_deref().map(|scheduled| {
            capture_schedule_log::plan(
                child_indent.as_deref().unwrap_or("\t"),
                scheduled,
                reason,
            )
        })
    });
    // Dependencies for a task this capture creates augment the block
    // before insertion: the managed child leads the authored children,
    // so duplicate bodies in one batch each land correctly.
    let new_task_dependencies = match &parsed.dependency_target {
        Some(target)
            if target.kind == ParsedDependencyTargetKind::NewTask
                && matches!(
                    parsed.kind,
                    CaptureKind::Task
                        | CaptureKind::TaskWithBlockId { .. }
                        | CaptureKind::Pomodoro { .. }
                ) =>
        {
            let status = match &parsed.kind {
                CaptureKind::Pomodoro { .. } => '*',
                _ => scheduled.as_deref().map(|_| '?').unwrap_or(' '),
            };
            let (augmented, parts) = plan_new_task_dependencies(
                dependency_ctx,
                planner,
                &request.bob_dir,
                parsed.route.as_deref(),
                target.block_id.as_deref(),
                &capture_line,
                status,
                child_indent.as_deref(),
                &parsed.dependencies,
                warnings,
            )?;
            Some((augmented, parts))
        }
        Some(target) if target.kind == ParsedDependencyTargetKind::NewTask => {
            return Err(CaptureError::io(
                "dependency invariant failed: new-task target on a non-task capture",
            ));
        }
        _ => None,
    };
    let (block_first_line, block_child_lines) = match &new_task_dependencies {
        Some((augmented, _)) => {
            let mut children = vec![augmented.dep_child.clone()];
            children.extend(sub_bullet_lines.iter().cloned());
            (augmented.task_line.clone(), children)
        }
        None => (capture_line.clone(), sub_bullet_lines.clone()),
    };
    let capture_block = assemble_capture_block(
        &block_first_line,
        (!block_child_lines.is_empty()).then_some(block_child_lines.as_slice()),
        clip_plan.as_ref().map(|plan| plan.output.lines.as_slice()),
        schedule_log.as_ref().map(|log| log.lines.as_slice()),
    );
    let mut note_plan = match &parsed.kind {
        CaptureKind::SubBullet {
            target: sub_bullet_target,
            section,
        } => {
            let route = parsed.route.as_deref().ok_or_else(|| {
                CaptureError::io(
                    "sub-bullet capture invariant failed: route is missing",
                )
            })?;
            plan_sub_bullet_capture(
                planner,
                &request.bob_dir,
                &target,
                route,
                sub_bullet_target,
                section.as_ref(),
                &capture_block,
            )?
        }
        CaptureKind::Pomodoro {
            block_id,
            pomodoro_name,
            start,
            close,
        } => {
            let route = parsed.route.as_deref().ok_or_else(|| {
                CaptureError::io(
                    "Pomodoro capture invariant failed: route is missing",
                )
            })?;
            if let Some(close_spec) = close.clone() {
                // The augmented block already carries the managed child;
                // the close path only needs the summary parts.
                let close_dependencies =
                    new_task_dependencies.as_ref().map(|(augmented, parts)| {
                        (augmented.task_line.clone(), parts.clone())
                    });
                return plan_pomodoro_close_task_item(
                    request,
                    parsed.clone(),
                    now,
                    today,
                    planner,
                    warnings,
                    route,
                    block_id,
                    &close_spec,
                    &capture_block,
                    close_dependencies,
                );
            }
            plan_capture_with_pomodoro_link(
                planner,
                &request.bob_dir,
                &target,
                route,
                block_id,
                pomodoro_name.as_deref(),
                start.as_ref(),
                &capture_block,
                now,
            )?
        }
        CaptureKind::PomodoroNote => {
            plan_pomodoro_note_capture(planner, &target, &capture_block)?
        }
        CaptureKind::ProjectNote { .. } => {
            return Err(CaptureError::io(
                "project-note capture invariant failed: wrong write planner",
            ));
        }
        _ => plan_capture_to_target(
            planner,
            &target,
            &capture_block,
            &parsed.kind,
        )?,
    };
    let pomodoro_refs = std::mem::take(&mut note_plan.pomodoro_refs);
    let special = note_plan.pomodoro.as_ref();
    let sub_bullet = note_plan.sub_bullet.as_ref();
    let pomodoro_note = note_plan.pomodoro_note.as_ref();
    // Dependency effects report through `task_blocks` with the new
    // `dependency` / `dependency_target` roles, and through the
    // `dependency_update` preview detail.
    let mut dependency_update: Option<DependencyUpdateJson> = None;
    let mut dependency_block_refs: Vec<TaskBlockRef> = Vec::new();
    match &parsed.dependency_target {
        Some(target) if target.kind == ParsedDependencyTargetKind::NewTask => {
            if let Some((augmented, parts)) = &new_task_dependencies {
                if let Some(tracked) = locate_new_task_ref(
                    planner,
                    &request.bob_dir,
                    parts,
                    &augmented.task_line,
                ) {
                    dependency_block_refs.push(tracked);
                }
                dependency_block_refs.extend(parts.target_refs.clone());
                dependency_update = Some(parts.summary.clone());
            }
        }
        Some(target)
            if target.kind == ParsedDependencyTargetKind::ExistingTask
                && matches!(parsed.kind, CaptureKind::SubBullet { .. }) =>
        {
            let (route, block_id) = match target {
                ParsedDependencyTarget {
                    route: Some(route),
                    block_id: Some(block_id),
                    ..
                } => (route.clone(), block_id.clone()),
                _ => {
                    return Err(CaptureError::io(
                        "dependency invariant failed: existing-task target without route and block ID",
                    ));
                }
            };
            // Prose captures normally first; the modifiers belong to the
            // explicit parent task, outside any child section.
            let update = plan_existing_task_dependencies(
                dependency_ctx,
                planner,
                &request.bob_dir,
                &route,
                &block_id,
                &parsed.dependencies,
                warnings,
            )?;
            dependency_block_refs.push(update.dependent_ref);
            dependency_block_refs.extend(update.target_refs);
            dependency_update = Some(update.summary);
        }
        _ => {}
    }
    let task_block_refs = match &parsed.kind {
        CaptureKind::SubBullet { .. } => {
            if let Some(details) = note_plan.sub_bullet.as_ref() {
                let route_str =
                    parsed.route.as_deref().unwrap_or("").to_string();
                let parent_index = details.parent_line.saturating_sub(1);
                if cfg!(debug_assertions)
                    && let Some(post) = planner.peek_text(&target)
                {
                    let settings = note_tasks::read_settings(&request.bob_dir);
                    let scan = note_tasks::scan(&post, &settings);
                    debug_assert!(
                        scan.task_at(parent_index).is_some(),
                        "sub-bullet parent missing in post-state"
                    );
                }
                vec![TaskBlockRef {
                    target: target.clone(),
                    relative_target: relative_target
                        .to_string_lossy()
                        .into_owned(),
                    route: route_str,
                    line: parent_index,
                    block_id: details.block_id.clone(),
                    role: TaskBlockRole::SubBullet,
                }]
            } else {
                debug_assert!(false, "sub-bullet missing details");
                Vec::new()
            }
        }
        _ => Vec::new(),
    };

    Ok(PlannedCaptureItem {
        result: CaptureItemResult {
            ok: true,
            dry_run: request.dry_run,
            routed: parsed.route.is_some(),
            route_label: parsed
                .route
                .as_deref()
                .map(route_label)
                .unwrap_or_default(),
            route: parsed.route,
            relative_target: relative_target.to_string_lossy().into_owned(),
            target: target.display().to_string(),
            text: parsed.body,
            task_line: block_first_line,
            kind: kind_label,
            created,
            scheduled,
            priority: priority.as_ref().map(|resolved| resolved.value.clone()),
            priority_label: priority
                .as_ref()
                .map(|resolved| resolved.label.clone()),
            placement: note_plan.placement,
            sub_bullets: sub_bullet_lines,
            clip: clip_output,
            schedule_log,
            block_id: task_block_id
                .or_else(|| {
                    special.as_ref().map(|edit| edit.details.block_id.clone())
                })
                .or_else(|| sub_bullet.and_then(|edit| edit.block_id.clone())),
            day_file: special
                .as_ref()
                .map(|edit| edit.details.day_file.clone())
                .or_else(|| pomodoro_note.map(|note| note.day_file.clone())),
            block_link: special
                .as_ref()
                .map(|edit| edit.details.block_link.clone()),
            pomodoro_link_placement: special
                .as_ref()
                .map(|edit| edit.details.pomodoro_link_placement),
            parent_line: sub_bullet
                .map(|edit| edit.parent_line)
                .or_else(|| pomodoro_note.map(|note| note.pomodoro_line)),
            parent_text: sub_bullet
                .map(|edit| edit.parent_text.clone())
                .or_else(|| {
                    pomodoro_note.map(|note| note.pomodoro_text.clone())
                }),
            parent_section: sub_bullet
                .and_then(|edit| edit.parent_section.clone()),
            parent_status_symbol: sub_bullet
                .map(|edit| edit.parent_status_symbol),
            parent_status_name: sub_bullet
                .map(|edit| edit.parent_status_name.clone()),
            toggle_direction: None,
            previous_task_line: None,
            status_symbol: None,
            status_name: None,
            previous_status_symbol: None,
            previous_status_name: None,
            pomodoro_name: special
                .as_ref()
                .and_then(|edit| edit.details.pomodoro_name.clone()),
            creates_pomodoro: special
                .as_ref()
                .map(|edit| edit.details.creates_pomodoro),
            pomodoro_already_linked: None,
            removed_pomodoro_links: None,
            removed_scheduled: None,
            pomodoro_selector_unused: None,
            toggle_behavior: None,
            status_changed: None,
            pomodoro_link_action: None,
            pomodoro_link_source: None,
            pomodoro_link_destination: special.as_ref().and_then(|edit| {
                edit.details.pomodoro_link_destination.clone()
            }),
            project_note: None,
            pomodoro_start: special
                .as_ref()
                .and_then(|edit| edit.start.clone()),
            pomodoro_adjust: None,
            pomodoro_shift: None,
            pomodoro_close: None,
            dependency_update,
            toggle_task_description: None,
        },
        clip_plan,
        pomodoro_refs,
        task_block_refs: {
            let mut refs = task_block_refs;
            refs.extend(dependency_block_refs);
            refs
        },
    })
}

pub(super) fn item_roll_seed(base: u64, item_index: usize) -> u64 {
    base.wrapping_add((item_index as u64).wrapping_mul(0x9E3779B97F4A7C15))
}

/// Join the capture line with its authored children (if any), its clip
/// children (if any), and its schedule log lines (if any), in note order:
/// the captured line first, then the authored bullets the user typed
/// beneath it, then any clipboard children, and the schedule log last since
/// it documents the whole block above it.
pub(super) fn assemble_capture_block(
    capture_line: &str,
    sub_bullet_lines: Option<&[String]>,
    clip_lines: Option<&[String]>,
    schedule_log_lines: Option<&[String]>,
) -> String {
    let mut lines = vec![capture_line];
    if let Some(sub_bullet_lines) = sub_bullet_lines {
        lines.extend(sub_bullet_lines.iter().map(String::as_str));
    }
    if let Some(clip_lines) = clip_lines {
        lines.extend(clip_lines.iter().map(String::as_str));
    }
    if let Some(schedule_log_lines) = schedule_log_lines {
        lines.extend(schedule_log_lines.iter().map(String::as_str));
    }
    lines.join("\n")
}

pub(super) fn render_authored_sub_bullets(
    sub_bullets: &[AuthoredSubBullet],
    indent_unit: &str,
) -> Vec<String> {
    sub_bullets
        .iter()
        .map(|item| {
            let indentation = indent_unit.repeat(item.depth.indent_units());
            format!("{indentation}- {}", item.body)
        })
        .collect()
}

pub(super) fn capture_kind_label(kind: &CaptureKind) -> &'static str {
    match kind {
        CaptureKind::Task | CaptureKind::TaskWithBlockId { .. } => "task",
        CaptureKind::Bullet { .. } => "bullet",
        CaptureKind::Pomodoro { .. } => "pomodoro_task",
        CaptureKind::SubBullet { .. } => "sub_bullet",
        CaptureKind::PomodoroNote => "pomodoro_note",
        CaptureKind::ProjectNote { .. } => "project_note",
        CaptureKind::TaskToggle { .. } => "task_toggle",
        CaptureKind::PomodoroAdjust { .. } => "pomodoro_adjust",
        CaptureKind::PomodoroShift { .. } => "pomodoro_shift",
        CaptureKind::PomodoroLink { .. } => "pomodoro_link",
        CaptureKind::PomodoroClose { .. } => "pomodoro_close",
        CaptureKind::PomodoroStart { .. } => "pomodoro_start",
        CaptureKind::TaskComplete { .. } => "task_complete",
    }
}

pub(super) fn date_string(date: NaiveDate) -> String {
    format!("{:04}-{:02}-{:02}", date.year(), date.month(), date.day())
}

pub(super) fn scheduled_date_string(
    today: NaiveDate,
    offset_days: u64,
) -> Result<String, CaptureError> {
    let scheduled =
        today
            .checked_add_days(Days::new(offset_days))
            .ok_or_else(|| {
                CaptureError::usage("scheduled offset is out of range")
            })?;
    Ok(date_string(scheduled))
}

/// A resolved `p:<N>` level: its field name/value/label plus the level's
/// roll window and, when the date was actually rolled, the chosen offset.
pub(super) struct ResolvedPriority {
    pub(super) name: String,
    pub(super) value: String,
    pub(super) label: String,
    pub(super) min_days: u64,
    pub(super) max_days: u64,
    pub(super) rolled_offset: Option<u64>,
}

/// Resolve a `p:<N>` level. The rolled offset is only computed when no
/// explicit `s:<N>` offset is present, since an explicit offset always wins
/// the scheduled date.
pub(super) fn resolve_priority(
    number: u64,
    explicit_scheduled_offset: Option<u64>,
    roll_seed: u64,
) -> Result<ResolvedPriority, CaptureError> {
    let property = config::load_priority_property(&config::config_path())
        .map_err(|error| match error {
            config::ConfigError::Read(message) => CaptureError::io(message),
            config::ConfigError::Invalid(message) => {
                CaptureError::usage(message)
            }
        })?;
    let level = property.level(number).ok_or_else(|| {
        CaptureError::usage(format!(
            "p:{number} is not a configured priority level; use p:1 through p:{} ({})",
            property.level_count(),
            property.labels()
        ))
    })?;
    let rolled_offset = explicit_scheduled_offset
        .is_none()
        .then(|| level.roll_offset(roll_seed));
    Ok(ResolvedPriority {
        name: property.name().to_string(),
        value: level.value().to_string(),
        label: level.label().to_string(),
        min_days: level.min_days(),
        max_days: level.max_days(),
        rolled_offset,
    })
}

pub(super) fn relative_target(route: Option<&str>) -> PathBuf {
    route
        .map(|route| PathBuf::from(route_label(route)))
        .unwrap_or_else(|| PathBuf::from(INBOX_FILE))
}

pub(crate) fn inbox_route() -> &'static str {
    INBOX_FILE.strip_suffix(".md").unwrap_or(INBOX_FILE)
}

pub(crate) fn route_label(route: &str) -> String {
    format!("{route}.md")
}

pub(crate) fn format_task_line(
    body: &str,
    created: &str,
    priority: Option<(&str, &str)>,
    scheduled: Option<&str>,
) -> String {
    let status = capture_task_status(" ", scheduled);
    let mut line = format!("- [{status}] #task {body} [created::{created}]");
    append_priority_property(&mut line, priority);
    append_scheduled_property(&mut line, scheduled);
    line
}

pub(super) fn format_task_with_block_id_line(
    body: &str,
    created: &str,
    priority: Option<(&str, &str)>,
    scheduled: Option<&str>,
    block_id: &str,
) -> String {
    let mut line = format_task_line(body, created, priority, scheduled);
    append_block_id(&mut line, block_id);
    line
}

pub(super) fn format_bullet_line(
    body: &str,
    created: &str,
    priority: Option<(&str, &str)>,
    scheduled: Option<&str>,
) -> String {
    let mut line = format!("- {body} [created::{created}]");
    append_priority_property(&mut line, priority);
    append_scheduled_property(&mut line, scheduled);
    line
}

pub(super) fn format_sub_bullet_line(
    body: &str,
    priority: Option<(&str, &str)>,
    scheduled: Option<&str>,
) -> String {
    let mut line = format!("- {body}");
    append_priority_property(&mut line, priority);
    append_scheduled_property(&mut line, scheduled);
    line
}

pub(super) fn format_pomodoro_task_line(
    body: &str,
    created: &str,
    priority: Option<(&str, &str)>,
    scheduled: Option<&str>,
    block_id: &str,
) -> String {
    let status = capture_task_status("*", scheduled);
    let mut line = format!("- [{status}] #task {body} [created::{created}]");
    append_priority_property(&mut line, priority);
    append_scheduled_property(&mut line, scheduled);
    append_block_id(&mut line, block_id);
    line
}

pub(super) fn capture_task_status<'a>(
    default: &'a str,
    scheduled: Option<&str>,
) -> &'a str {
    if scheduled.is_some() {
        "?"
    } else {
        default
    }
}

pub(super) fn append_priority_property(
    line: &mut String,
    priority: Option<(&str, &str)>,
) {
    if let Some((property, value)) = priority {
        line.push_str(&format!(" [{property}::{value}]"));
    }
}

pub(super) fn append_scheduled_property(
    line: &mut String,
    scheduled: Option<&str>,
) {
    if let Some(scheduled) = scheduled {
        line.push_str(&format!(" [scheduled::{scheduled}]"));
    }
}

pub(super) fn append_block_id(line: &mut String, block_id: &str) {
    line.push_str(&format!(" ^{block_id}"));
}
