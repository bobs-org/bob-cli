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
    contents: &str,
    task_line_index: usize,
    previous_status_symbol: char,
    previous_status_name: String,
    task_description: String,
    previous_task_line: String,
    today: NaiveDate,
    warnings: &mut Vec<String>,
    settings: &note_tasks::NoteTaskSettings,
) -> Result<CaptureWritePlan, CaptureError> {
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
    let task_plan = capture_task_toggle::plan_task_link(contents, task_line_index, today)
        .ok_or_else(|| {
            CaptureError::io("task toggle capture invariant failed: task line could not be updated")
        })?;
    let task_line =
        line_text_at(&task_plan.content, task_line_index)?.to_string();
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
    if status_changed && task_line_declares_dependencies(&previous_task_line) {
        warnings.push(format!(
            "^{block_id} still declares dependencies; bob task-status-hooks may return it to Blocked"
        ));
    }

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

    Ok(CaptureWritePlan {
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
        }),
        pomodoro_link: None,
        pomodoro_refs,
    })
}
