//! Task-toggle capture planning.
use super::*;

#[allow(clippy::too_many_arguments)]
pub(super) fn plan_task_toggle_capture(
    planner: &mut CaptureBatchPlanner,
    bob_dir: &Path,
    target: &Path,
    route: &str,
    block_id: &str,
    pomodoro_name: Option<&str>,
    intent: TaskToggleIntent,
    today: NaiveDate,
    warnings: &mut Vec<String>,
) -> Result<CaptureWritePlan, CaptureError> {
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
            let message = match scan.suggest_block_id(block_id) {
                Some(suggestion) => format!(
                    "no task with block ID ^{block_id} in {route}.md; did you mean ^{suggestion}? ({choices})"
                ),
                None => format!("no task with block ID ^{block_id} in {route}.md ({choices})"),
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

    if intent == TaskToggleIntent::EnsureNext {
        return plan_ensure_next_capture(
            planner,
            bob_dir,
            target,
            route,
            block_id,
            pomodoro_name,
            &contents,
            task_line_index,
            previous_status_symbol,
            previous_status_name,
            task_description,
            previous_task_line,
            today,
            warnings,
            &settings,
        );
    }

    let direction = match previous_status_symbol {
        ' ' | '?' => TaskToggleDirection::Next,
        '*' => TaskToggleDirection::Open,
        '/' => {
            return Err(CaptureError::io(format!(
                "task ^{block_id} is In Progress; toggling from In Progress needs a work summary, so use <ctrl+shift+enter> in Obsidian"
            )));
        }
        _ => {
            return Err(CaptureError::io(format!(
                "task ^{block_id} is {previous_status_name}; only Ready, Blocked, and Next tasks can be toggled"
            )));
        }
    };

    let task_plan = match direction {
        TaskToggleDirection::Next => {
            capture_task_toggle::plan_task_next(&contents, task_line_index, today)
        }
        TaskToggleDirection::Open => {
            capture_task_toggle::plan_task_open(&contents, task_line_index)
        }
    }
    .ok_or_else(|| {
        CaptureError::io("task toggle capture invariant failed: task line could not be updated")
    })?;

    let task_line =
        line_text_at(&task_plan.content, task_line_index)?.to_string();
    let updated_scan = note_tasks::scan(&task_plan.content, &settings);
    let updated_task = match updated_scan.by_block_id(block_id) {
        BlockIdLookup::Found(task) => task,
        _ => {
            return Err(CaptureError::io(
                "task toggle capture invariant failed: toggled task disappeared",
            ));
        }
    };

    if direction == TaskToggleDirection::Next
        && task_line_declares_dependencies(&previous_task_line)
    {
        warnings.push(format!(
            "^{block_id} still declares dependencies; bob task-status-hooks may return it to Blocked"
        ));
    }

    planner.stage(target, task_plan.content)?;

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

    let mut pomodoro_link_placement = None;
    let resolved_pomodoro_name;
    let mut creates_pomodoro = false;
    let mut pomodoro_already_linked = false;
    let removed_pomodoro_links;
    let mut pomodoro_selector_unused = false;

    match direction {
        TaskToggleDirection::Next => {
            match capture_task_toggle::plan_link_insertion(
                &day_contents,
                &block_link,
                pomodoro_name,
            )
            .map_err(link_plan_error)?
            {
                capture_task_toggle::LinkInsertionOutcome::Planned(plan) => {
                    pomodoro_link_placement = plan.placement.map(link_placement_to_placement);
                    resolved_pomodoro_name = plan.pomodoro_name;
                    pomodoro_already_linked = plan.already_linked;
                    removed_pomodoro_links = plan.removed_links;
                    if plan.has_changes {
                        planner.stage(&day_file, plan.content)?;
                    }
                }
                capture_task_toggle::LinkInsertionOutcome::NeedsPomodoroCreation {
                    canonical_name,
                } => {
                    let selector = pomodoro_name.ok_or_else(|| {
                        CaptureError::io(
                            "task toggle capture invariant failed: named creation without selector",
                        )
                    })?;
                    let (created_day, placement, _, _) = insert_pomodoro_child_block(
                        &day_contents,
                        &format!("- {block_link}"),
                        PomodoroSelection::NamedOrCreate(selector),
                    )?;
                    let cleanup = match capture_task_toggle::plan_link_insertion(
                        &created_day,
                        &block_link,
                        Some(selector),
                    )
                    .map_err(link_plan_error)?
                    {
                        capture_task_toggle::LinkInsertionOutcome::Planned(plan) => plan,
                        capture_task_toggle::LinkInsertionOutcome::NeedsPomodoroCreation {
                            ..
                        } => {
                            return Err(CaptureError::io(
                                "task toggle capture invariant failed: created Pomodoro was not selectable",
                            ));
                        }
                    };
                    pomodoro_link_placement = Some(placement);
                    resolved_pomodoro_name = Some(canonical_name);
                    creates_pomodoro = true;
                    removed_pomodoro_links = cleanup.removed_links;
                    planner.stage(&day_file, cleanup.content)?;
                }
            }
        }
        TaskToggleDirection::Open => {
            let scan = capture_pomodoros::scan(&day_contents);
            if !scan.has_section {
                return Err(CaptureError::io("Bob daily note has no Pomodoros section"));
            }
            let removal = capture_task_toggle::plan_link_removal(&day_contents, &block_link);
            removed_pomodoro_links = removal.removed_links;
            if removal.has_changes {
                planner.stage(&day_file, removal.content)?;
            }
            pomodoro_selector_unused = pomodoro_name.is_some();
            resolved_pomodoro_name =
                pomodoro_name.and_then(capture_pomodoros::canonicalize_pomodoro_name);
        }
    }

    Ok(CaptureWritePlan {
        placement: Placement::Toggled,
        pomodoro: None,
        sub_bullet: None,
        pomodoro_note: None,
        toggle: Some(TaskToggleCaptureDetails {
            direction,
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
            status_changed: None,
            pomodoro_link_action: None,
            pomodoro_link_source: None,
            pomodoro_link_destination: None,
        }),
        pomodoro_link: None,
        // Two-way toggles rely on block auto-detection; see
        // `PomodoroBlockTracker::track_item`.
        pomodoro_refs: Vec::new(),
    })
}
