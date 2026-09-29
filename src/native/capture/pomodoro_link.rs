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
    start: Option<&PomodoroStartSpec>,
    capture_block: &str,
    now: chrono::NaiveDateTime,
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
        )));
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
    if let Some(spec) = start {
        let (updated_day, pomodoro_link_placement, summary) =
            plan_pomodoro_start(
                &original_day,
                &block_link,
                pomodoro_name,
                spec,
                now,
            )?;
        planner.stage(target, updated_target)?;
        planner.stage(&day_file, updated_day)?;
        let destination = destination_json(
            &capture_task_toggle::PomodoroEndpoint {
                line: summary.pomodoro_line,
                name: summary.pomodoro_name.clone(),
                time_range: Some(summary.time_range.clone()),
            },
            summary.created_pomodoro,
            pomodoro_name,
        );
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
        });
    }
    let insertion =
        insert_pomodoro_block_link(&original_day, &block_link, pomodoro_name)?;
    planner.stage(target, updated_target)?;
    planner.stage(&day_file, insertion.updated)?;

    Ok(CaptureWritePlan {
        placement,
        pomodoro: Some(PlannedPomodoroEdit {
            details: PomodoroCaptureDetails {
                block_id: block_id.to_string(),
                day_file: day_file_label,
                block_link,
                pomodoro_link_placement: insertion.placement,
                pomodoro_name: insertion.destination.name.clone(),
                creates_pomodoro: insertion.creates_pomodoro,
                pomodoro_link_destination: Some(insertion.destination),
            },
            start: None,
        }),
        sub_bullet: None,
        pomodoro_note: None,
        toggle: None,
        pomodoro_link: None,
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
            "Pomodoro link capture cannot be combined with p:<N>",
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
    pomodoro_name: Option<&str>,
    start: Option<&PomodoroStartSpec>,
    now: chrono::NaiveDateTime,
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

    match previous_status_symbol {
        ' ' | '?' | '*' | '/' => {}
        _ => {
            return Err(CaptureError::io(format!(
                "task ^{block_id} is {previous_status_name}; only Ready, Blocked, Next, and In Progress tasks can be linked to a Pomodoro"
            )));
        }
    }

    let task_plan =
        capture_task_toggle::plan_task_link(&contents, task_line_index, today)
            .ok_or_else(|| {
                CaptureError::io(
                    "pomodoro link capture invariant failed: task line could not be updated",
                )
            })?;
    let task_line =
        line_text_at(&task_plan.content, task_line_index)?.to_string();
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
    if status_changed && task_line_declares_dependencies(&previous_task_line) {
        warnings.push(format!(
            "^{block_id} still declares dependencies; bob task-status-hooks may return it to Blocked"
        ));
    }
    if task_plan.content != contents {
        planner.stage(target, task_plan.content.clone())?;
    }

    let day_file = pomodoro::day_file_for(bob_dir);
    if !day_file.is_file() && !planner.currently_exists(&day_file)? {
        return Err(CaptureError::io(format!(
            "Bob daily note does not exist: {}",
            day_file.display()
        )));
    }
    if paths_refer_to_same_file(target, &day_file) {
        return Err(CaptureError::io(
            "routed note and Bob daily note must be different files",
        ));
    }
    let day_contents = planner.read_existing(&day_file)?;
    let day_file_label = day_file.display().to_string();
    let block_link = format!("[[{route}#^{block_id}]]");

    if let Some(spec) = start {
        return plan_pomodoro_link_with_start(
            planner,
            &day_file,
            &day_file_label,
            &block_link,
            route,
            block_id,
            pomodoro_name,
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
        );
    }

    let relocation = capture_task_toggle::plan_pomodoro_link_ledger(
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
    let action = match relocation.action {
        capture_task_toggle::PomodoroLinkLedgerAction::Linked => "linked",
        capture_task_toggle::PomodoroLinkLedgerAction::Moved => "moved",
        capture_task_toggle::PomodoroLinkLedgerAction::AlreadyCurrent => {
            "already_current"
        }
    };
    let placement = relocation.placement.map(link_placement_to_placement);
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
            status_symbol: updated_task.status_symbol,
            status_name: updated_task.status_name.clone(),
            block_id: block_id.to_string(),
            day_file: day_file_label,
            block_link,
            pomodoro_link_placement: placement,
            pomodoro_name: relocation.destination.name.clone(),
            creates_pomodoro: relocation.creates_pomodoro,
            pomodoro_link_action: action,
            pomodoro_link_source: relocation.source.as_ref().map(endpoint_json),
            pomodoro_link_destination: Some(destination_json(
                &relocation.destination,
                relocation.creates_pomodoro,
                pomodoro_name,
            )),
            removed_scheduled: task_plan.removed_scheduled,
            schedule_log: task_plan.schedule_log,
            status_changed,
            pomodoro_start: None,
        }),
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
                "Pomodoro {name} is already running; finish the current Pomodoro first (close it with `=x`) or use `+N`/`-N` to adjust it or `++N`/`--N` to shift it"
            )));
        }
        return Err(CaptureError::io(
            "Bob daily note has an active timed Pomodoro; finish the current Pomodoro first (close it with `=x`)",
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
        let (updated_day, placement, summary) = plan_pomodoro_start(
            day_contents,
            block_link,
            pomodoro_name,
            spec,
            now,
        )?;
        planner.stage(day_file, updated_day)?;
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
                pomodoro_link_destination: Some(destination_json(
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
                )),
                removed_scheduled,
                schedule_log,
                status_changed,
                pomodoro_start: Some(summary),
            }),
        });
    }

    let (source_endpoint, q_entry_index, q_line_index, q_subtree_end) =
        queue.expect("queue exists");
    if let Some(selector) = pomodoro_name {
        match capture_pomodoros::select_named(&scan, selector) {
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
                            }),
                            pomodoro_link_destination: Some(dest_json),
                            removed_scheduled,
                            schedule_log,
                            status_changed,
                            pomodoro_start: Some(summary),
                        }),
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
                        }),
                        pomodoro_link_destination: Some(dest_json),
                        removed_scheduled,
                        schedule_log,
                        status_changed,
                        pomodoro_start: Some(summary),
                    }),
                });
            }
            capture_pomodoros::NamedSelection::CompletedOnly(_)
            | capture_pomodoros::NamedSelection::Missing { .. } => {
                let (with_placeholder, created_line, name) =
                    capture_task_toggle::insert_named_placeholder(
                        day_contents,
                        selector,
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
                        }),
                        pomodoro_link_destination: Some(dest_json),
                        removed_scheduled,
                        schedule_log,
                        status_changed,
                        pomodoro_start: Some(summary),
                    }),
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
    };
    let _ = (q_line_index, q_subtree_end);
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
            }),
            pomodoro_link_destination: Some(dest_json),
            removed_scheduled,
            schedule_log,
            status_changed,
            pomodoro_start: Some(summary),
        }),
    })
}

pub(super) fn relocation_plan_error(
    error: capture_task_toggle::LinkRelocationError,
    block_link: &str,
    route: &str,
    block_id: &str,
) -> CaptureError {
    match error {
        capture_task_toggle::LinkRelocationError::NoPomodorosSection
        | capture_task_toggle::LinkRelocationError::NoEligibleOpenEntry
        | capture_task_toggle::LinkRelocationError::MultipleOpenTimedEntries
        | capture_task_toggle::LinkRelocationError::InvalidPomodoroName => {
            link_plan_error(match error {
                capture_task_toggle::LinkRelocationError::NoPomodorosSection => {
                    capture_task_toggle::LinkPlanError::NoPomodorosSection
                }
                capture_task_toggle::LinkRelocationError::NoEligibleOpenEntry => {
                    capture_task_toggle::LinkPlanError::NoEligibleOpenEntry
                }
                capture_task_toggle::LinkRelocationError::MultipleOpenTimedEntries => {
                    capture_task_toggle::LinkPlanError::MultipleOpenTimedEntries
                }
                capture_task_toggle::LinkRelocationError::InvalidPomodoroName => {
                    capture_task_toggle::LinkPlanError::InvalidPomodoroName
                }
                _ => capture_task_toggle::LinkPlanError::NoEligibleOpenEntry,
            })
        }
        capture_task_toggle::LinkRelocationError::NoMovableLink => CaptureError::io(format!(
            "no movable open Pomodoro Task Link for {block_link}; use @{route}+{block_id}! to run the explicit toggle and add one"
        )),
        capture_task_toggle::LinkRelocationError::MultipleMovableLinks => {
            CaptureError::io(format!(
                "found more than one movable open Pomodoro Task Link for {block_link}; make the dedicated Task Link unique before capturing"
            ))
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

pub(super) fn task_line_declares_dependencies(line: &str) -> bool {
    line.contains("[dependsOn::") || line.contains("(dependsOn::")
}

pub(super) fn link_plan_error(
    error: capture_task_toggle::LinkPlanError,
) -> CaptureError {
    match error {
        capture_task_toggle::LinkPlanError::NoPomodorosSection => {
            CaptureError::io("Bob daily note has no Pomodoros section")
        }
        capture_task_toggle::LinkPlanError::NoEligibleOpenEntry => {
            CaptureError::io("Bob daily note has no eligible open Pomodoro")
        }
        capture_task_toggle::LinkPlanError::MultipleOpenTimedEntries => {
            CaptureError::io("Bob daily note has multiple open timed Pomodoros")
        }
        capture_task_toggle::LinkPlanError::InvalidPomodoroName => {
            CaptureError::usage(capture_pomodoros::POMODORO_NAME_USAGE)
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
