//! Project-note planning for plus-sigil captures.
use super::*;

pub(super) fn reject_project_note_conflicts(
    parsed: &ParsedCaptureText,
    request: &CaptureRequest,
) -> Result<(), CaptureError> {
    if request.forced_clip.is_some() {
        return Err(CaptureError::usage(
            "project-note capture cannot be combined with --clip",
        ));
    }
    if parsed.clip.is_some() {
        return Err(CaptureError::usage(
            "project-note capture cannot be combined with % clipboard markers",
        ));
    }
    Ok(())
}

pub(super) fn validate_project_note_parent(
    planner: &mut CaptureBatchPlanner,
    bob_dir: &Path,
    route: &str,
) -> Result<(), CaptureError> {
    let parent_path = bob_dir.join(format!("{route}.md"));
    let Some(contents) = planner.current_contents(&parent_path)? else {
        return Err(CaptureError::io(format!(
            "cannot create a project note under {route}.md: note does not exist (run 'bob capture-targets' to list routable notes)"
        )));
    };
    let Some(frontmatter) = parse_frontmatter(&contents) else {
        return Err(CaptureError::io(format!(
            "cannot create a project note under {route}.md: note is not an area or project note"
        )));
    };
    if frontmatter_is_area(&frontmatter) {
        return Ok(());
    }
    if !frontmatter_is_project(&frontmatter) {
        return Err(CaptureError::io(format!(
            "cannot create a project note under {route}.md: note is not an area or project note"
        )));
    }
    let status =
        ProjectStatus::parse(frontmatter_value(&frontmatter, "status"));
    if status.is_terminal() {
        return Err(CaptureError::io(format!(
            "cannot create a project note under {route}.md: note is a {} project",
            status.label()
        )));
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(super) fn plan_project_note_item(
    request: &CaptureRequest,
    parsed: ParsedCaptureText,
    now: chrono::NaiveDateTime,
    today: NaiveDate,
    roll_seed: u64,
    item_index: usize,
    planner: &mut CaptureBatchPlanner,
) -> Result<PlannedCaptureItem, CaptureError> {
    let CaptureKind::ProjectNote {
        block_id,
        pomodoro_name,
    } = parsed.kind.clone()
    else {
        return Err(CaptureError::io(
            "project-note capture invariant failed: wrong capture kind",
        ));
    };
    // The grammar rejects a `#pomodoro` name with no ` :` task before
    // planning; this guards planners that bypass it.
    if let Some(name) = pomodoro_name {
        return Err(CaptureError::usage(unused_project_note_pomodoro_error(
            &name,
        )));
    }
    let route = parsed.route.clone().ok_or_else(|| {
        CaptureError::io(
            "project-note capture invariant failed: route is missing",
        )
    })?;
    reject_project_note_conflicts(&parsed, request)?;

    let created = date_string(today);
    let priority = match parsed.priority_level {
        Some(number) => Some(resolve_priority(
            number,
            parsed.scheduled_offset,
            item_roll_seed(roll_seed, item_index),
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
    let schedule_log = schedule_log_reason.and_then(|reason| {
        scheduled.as_deref().map(|scheduled| {
            capture_schedule_log::plan("\t", scheduled, reason)
        })
    });

    validate_project_note_parent(planner, &request.bob_dir, &route)?;

    let basename =
        capture_project_note::project_note_basename(&route, &block_id);
    let new_target = request.bob_dir.join(&basename);
    if planner.currently_exists(&new_target)? {
        return Err(CaptureError::io(format!(
            "project note already exists: {}",
            new_target.display()
        )));
    }
    validate_target_parent(&new_target)?;

    let schedule_log_lines: Vec<String> = schedule_log
        .as_ref()
        .map(|log| log.lines.clone())
        .unwrap_or_default();
    let render_input = capture_project_note::ProjectNoteRenderInput {
        route: &route,
        block_id: &block_id,
        body: &parsed.body,
        now,
        scheduled: scheduled.as_deref(),
        priority: priority_field,
        schedule_log_lines: &schedule_log_lines,
        sub_bullets: &parsed.sub_bullets,
    };
    let rendered = capture_project_note::render_project_note(&render_input);

    // A project note with no ` :` task never reads the daily note: the
    // `^prj` task is never linked, and Task Links arrive in a later phase.
    planner.stage(&new_target, rendered.contents.clone())?;

    Ok(PlannedCaptureItem {
        result: CaptureItemResult {
            ok: true,
            dry_run: request.dry_run,
            routed: true,
            route_label: basename.clone(),
            route: Some(route.clone()),
            relative_target: basename.clone(),
            target: new_target.display().to_string(),
            text: parsed.body,
            task_line: rendered.task_line.clone(),
            kind: "project_note",
            created,
            scheduled,
            priority: priority.as_ref().map(|resolved| resolved.value.clone()),
            priority_label: priority
                .as_ref()
                .map(|resolved| resolved.label.clone()),
            placement: Placement::Created,
            sub_bullets: Vec::new(),
            clip: None,
            schedule_log,
            block_id: Some("prj".to_string()),
            day_file: None,
            block_link: None,
            pomodoro_link_placement: None,
            parent_line: None,
            parent_text: None,
            parent_section: None,
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
            project_note: Some(ProjectNoteSummary {
                basename: rendered.basename.clone(),
                parent_route: route.clone(),
                parent_link: format!("[[{route}]]"),
                tasks: rendered.task_count,
                sections: rendered.sections.clone(),
            }),
            pomodoro_start: None,
            pomodoro_adjust: None,
            pomodoro_shift: None,
            pomodoro_close: None,
            toggle_task_description: None,
        },
        clip_plan: None,
    })
}
