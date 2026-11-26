//! Capture adapter for the pure snooze planners. All reads use the batch
//! overlay; postimages join the same guarded, whole-draft transaction.
use super::dependencies::staged_note_index;
use super::*;
use crate::native::{
    snooze::{
        parse::parse_section,
        plan::{
            self as core, AddInput, AddRefusal, DropInput, IdentityMap,
            TaskFact,
        },
        state::{TaskIdentity, TaskState},
        when::{parse_snooze_when, ResolvedTime, SnoozeClock},
    },
    task_dependencies::block_link_spans,
    task_status_hooks::TaskStatusType,
    vault_links::LinkResolution,
};
use chrono::Timelike;

/// Additive item contract. Coordinates refer to the final settled batch;
/// `row` is absent after drop/link-now (or a later item removes the row).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct SnoozeJson {
    pub(super) action: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) time: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) wake_at: Option<String>,
    pub(super) route: String,
    pub(super) block_id: String,
    pub(super) daily_file: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) row: Option<usize>,
}

/// Guard configuration before capture's shared settings caches are populated.
pub(super) fn guard_settings(
    planner: &mut CaptureBatchPlanner,
    bob_dir: &Path,
    config_path: Option<&Path>,
) -> Result<(), CaptureError> {
    let mut paths = vec![
        bob_dir.join(".obsidian/plugins/obsidian-tasks-plugin/data.json"),
        config::config_path(),
    ];
    if let Some(path) = config_path {
        paths.push(path.to_path_buf());
    }
    paths.sort();
    paths.dedup();
    for path in paths {
        let text = planner.current_contents(&path)?;
        planner.guard_merge_read(&path, text);
    }
    Ok(())
}

pub(super) fn clock(
    now: chrono::NaiveDateTime,
) -> Result<SnoozeClock, CaptureError> {
    let zone = bob_env::configured_timezone()
        .or_else(crate::native::snooze::host_timezone)
        .ok_or_else(|| CaptureError::io("cannot identify the host timezone; set timezone in Bob's config"))?;
    Ok(SnoozeClock {
        date: now.date(),
        now_minutes: (now.hour() * 60 + now.minute()) as u16,
        zone,
    })
}

pub(super) fn resolve_time(
    raw: &str,
    clock: SnoozeClock,
) -> Result<ResolvedTime, CaptureError> {
    parse_snooze_when(raw, clock).map_err(|error| {
        let code = error.clone().as_str();
        CaptureError::usage_code(
            format!(
                "snooze: {}; link it now, or use `s:1` for tomorrow",
                error.message()
            ),
            code,
        )
    })
}

fn day_snapshot(
    request: &CaptureRequest,
    planner: &mut CaptureBatchPlanner,
    today: NaiveDate,
) -> Result<(PathBuf, String), CaptureError> {
    let path = pomodoro::day_file_for(&request.bob_dir);
    if path
        .file_name()
        .and_then(|name| name.to_str())
        .and_then(pomodoro::parse_day_file_date)
        .is_some_and(|date| date != today)
    {
        return Err(CaptureError::usage_code(
            "snooze rows are for today's daily note only",
            "not_today",
        ));
    }
    let text = planner.read_existing(&path)?;
    Ok((path, text))
}

/// Note membership decides basename resolution. Files read by the dependency
/// assessment decide eligibility even when none of those files are written.
fn guard_inputs(
    planner: &mut CaptureBatchPlanner,
    bob_dir: &Path,
) -> Result<(), CaptureError> {
    let paths = collect_done::link_repair_markdown_files(bob_dir)
        .map_err(|error| CaptureError::io(error.to_string()))?;
    planner.guard_merge_scan(paths)?;
    let inputs = planner
        .files
        .iter()
        .map(|file| {
            (
                file.path.clone(),
                file.existed.then(|| file.original.clone()),
            )
        })
        .collect::<Vec<_>>();
    for (path, text) in inputs {
        planner.guard_merge_read(&path, text);
    }
    Ok(())
}

fn guard_catalog(
    ctx: &DependencyContext,
    planner: &mut CaptureBatchPlanner,
    bob_dir: &Path,
) -> Result<(), CaptureError> {
    if !ctx.catalog().complete() {
        return Err(CaptureError::io(
            "cannot resolve snooze safely: vault note catalog is incomplete",
        ));
    }
    planner.guard_merge_scan(
        ctx.catalog()
            .index
            .relative_paths()
            .map(|path| bob_dir.join(path))
            .collect(),
    )
}

fn identity_map(
    ctx: &DependencyContext,
    planner: &CaptureBatchPlanner,
    bob_dir: &Path,
    day_path: &Path,
    day: &str,
) -> IdentityMap {
    let index = staged_note_index(ctx, planner, bob_dir);
    let mut identities = IdentityMap::new();
    for span in day.lines().flat_map(block_link_spans) {
        if let LinkResolution::Found(path) =
            index.resolve_detailed(Some(day_path), &span.target)
        {
            identities.insert(
                (span.target, span.block_id.clone()),
                TaskIdentity {
                    route: path,
                    block_id: span.block_id,
                },
            );
        }
    }
    identities
}

fn resolve_target(
    ctx: &DependencyContext,
    planner: &CaptureBatchPlanner,
    bob_dir: &Path,
    route: &str,
) -> Result<PathBuf, CaptureError> {
    match staged_note_index(ctx, planner, bob_dir).resolve_detailed(None, route)
    {
        LinkResolution::Found(path) => Ok(path),
        _ => Err(CaptureError::usage(format!(
            "snooze target @{route} must resolve to one note"
        ))),
    }
}

fn task_fact(
    request: &CaptureRequest,
    relative: &Path,
    block_id: &str,
    today: NaiveDate,
    planner: &mut CaptureBatchPlanner,
    ctx: &DependencyContext,
    themes: &mut CaptureTaskThemeContext,
    warnings: &mut Vec<CaptureWarning>,
) -> Result<TaskFact, CaptureError> {
    let settings_path = request
        .bob_dir
        .join(".obsidian/plugins/obsidian-tasks-plugin/data.json");
    let settings_text = planner.current_contents(&settings_path)?;
    planner.guard_merge_read(&settings_path, settings_text);
    let contents = planner.read_existing(&request.bob_dir.join(relative))?;
    let scan = note_tasks::scan(&contents, ctx.settings());
    let task =
        lookup_staged_task(&scan, &display_relative(relative), block_id)?;
    let line = contents.lines().nth(task.line_index).unwrap_or_default();
    let event = crate::native::event_task::event_task_at(
        relative,
        &contents,
        task.line_index,
    );
    if let Some(event) = event {
        return Err(CaptureError::usage_code(
            format!(
                "Event Tasks wait on their own clock ({})",
                event.span.text()
            ),
            "event_task",
        ));
    }
    let status = match task.status_type {
        TaskStatusType::Done => 'x',
        TaskStatusType::Cancelled => '-',
        _ => task.status_symbol,
    };
    let blocked = status == '?'
        || !matches!(
            assess_link_target(
                ctx,
                planner,
                &request.bob_dir,
                relative,
                block_id,
                today
            )?,
            LinkGuardVerdict::Link
        );
    let route = route_without_extension(relative);
    let placement = themes.placement_for_task(planner, &route, line, warnings);
    let fact = TaskFact {
        identity: TaskIdentity {
            route: relative.to_path_buf(),
            block_id: block_id.to_string(),
        },
        path: relative.to_path_buf(),
        contents: contents.clone(),
        line_index: task.line_index,
        state: TaskState {
            status,
            is_task: task.status_type != TaskStatusType::NonTask,
            is_event: false,
        },
        blocked,
        type_key: placement.as_ref().map(|p| p.suggestion.theme.key.clone()),
        type_name: placement.as_ref().map(|p| p.suggestion.theme.name.clone()),
        link_text: format!("[[{route}#^{block_id}]]"),
    };
    guard_inputs(planner, &request.bob_dir)?;
    Ok(fact)
}

fn add_error(error: AddRefusal) -> CaptureError {
    let message = match error {
        AddRefusal::EventTask => "Event Tasks wait on their own clock",
        AddRefusal::Blocked => "Blocked by prerequisites or schedule — snooze waits on the clock, not prerequisites",
        AddRefusal::Done => "completed tasks cannot be snoozed",
        AddRefusal::Cancelled => "cancelled tasks cannot be snoozed",
        AddRefusal::NotTask => "snooze needs one #task",
        AddRefusal::NoPomodorosSection => "today's note has no ## Pomodoros section",
    };
    CaptureError::usage_code(message, error.as_str())
}

pub(super) fn add_to_item(
    request: &CaptureRequest,
    item: &mut PlannedCaptureItem,
    clock: SnoozeClock,
    when: &ResolvedTime,
    planner: &mut CaptureBatchPlanner,
    ctx: &mut DependencyContext,
    themes: &mut CaptureTaskThemeContext,
    warnings: &mut Vec<CaptureWarning>,
) -> Result<(), CaptureError> {
    let relative = PathBuf::from(&item.result.relative_target);
    let block_id = item
        .result
        .block_id
        .as_deref()
        .ok_or_else(|| CaptureError::io("snooze task has no block ID"))?;
    let (day_path, day) = day_snapshot(request, planner, clock.date)?;
    let day_relative =
        day_path.strip_prefix(&request.bob_dir).unwrap_or(&day_path);
    guard_catalog(ctx, planner, &request.bob_dir)?;
    let fact = task_fact(
        request, &relative, block_id, clock.date, planner, ctx, themes,
        warnings,
    )?;
    let identities =
        identity_map(ctx, planner, &request.bob_dir, day_relative, &day);
    let retime =
        core::active_row_line(&day, &fact.identity, &identities).is_some();
    let plan = core::plan_add(&AddInput {
        day_path: day_relative,
        day: &day,
        task: &fact,
        when,
        clock,
        identities: &identities,
    })
    .map_err(add_error)?;
    for file in plan.files {
        planner.stage(&request.bob_dir.join(file.path), file.contents)?;
    }
    item.result.snooze = Some(SnoozeJson {
        action: if retime { "retime" } else { "add" },
        time: Some(when.hhmm.clone()),
        wake_at: Some(when.rfc3339.clone()),
        route: display_relative(&relative),
        block_id: block_id.to_string(),
        daily_file: display_relative(day_relative),
        row: None,
    });
    item.result.day_file = Some(day_path.display().to_string());
    item.result.block_link = Some(fact.link_text);
    item.result.previous_task_line = Some(
        fact.contents
            .lines()
            .nth(fact.line_index)
            .unwrap_or_default()
            .to_string(),
    );
    let contents = planner.read_existing(&request.bob_dir.join(&relative))?;
    let scan = note_tasks::scan(&contents, ctx.settings());
    let task =
        lookup_staged_task(&scan, &display_relative(&relative), block_id)?;
    item.result.task_line = contents
        .lines()
        .nth(task.line_index)
        .unwrap_or_default()
        .to_string();
    item.result.status_symbol = Some(task.status_symbol);
    item.result.status_name = Some(task.status_name.clone());
    item.task_block_refs.push(TaskBlockRef {
        target: request.bob_dir.join(&relative),
        relative_target: display_relative(&relative),
        route: route_without_extension(&relative),
        line: task.line_index,
        block_id: Some(block_id.to_string()),
        role: TaskBlockRole::Snoozed,
    });
    Ok(())
}

pub(super) fn plan_existing(
    request: &CaptureRequest,
    parsed: &ParsedCaptureText,
    block_id: &str,
    clock: SnoozeClock,
    when: Option<&ResolvedTime>,
    planner: &mut CaptureBatchPlanner,
    ctx: &mut DependencyContext,
    themes: &mut CaptureTaskThemeContext,
    warnings: &mut Vec<CaptureWarning>,
) -> Result<PlannedCaptureItem, CaptureError> {
    if !parsed.dependencies.is_empty()
        || !parsed.blocked_tasks.is_empty()
        || !parsed.relations.is_empty()
        || parsed.merge_in.is_some()
    {
        return Err(CaptureError::usage("snooze takes one task intent; edit its relationships in a separate item before z:<when>"));
    }
    // Preserve the ordinary forced-destination and existing-task restrictions.
    reject_pomodoro_link_conflicts(parsed, request)?;
    let route = parsed
        .route
        .as_deref()
        .ok_or_else(|| CaptureError::io("snooze route missing"))?;
    guard_catalog(ctx, planner, &request.bob_dir)?;
    let relative = resolve_target(ctx, planner, &request.bob_dir, route)?;
    let target = request.bob_dir.join(&relative);
    let contents = planner.read_existing(&target)?;
    let scan = note_tasks::scan(&contents, ctx.settings());
    let task = lookup_staged_task(&scan, route, block_id)?;
    let mut result =
        existing_result(request, &relative, task, &contents, clock.date);
    result.route = Some(route.to_string());
    result.route_label = route_label(route);
    let (day_path, day) = day_snapshot(request, planner, clock.date)?;
    let day_relative =
        day_path.strip_prefix(&request.bob_dir).unwrap_or(&day_path);
    if let Some(when) = when {
        if let Some(marker) = &parsed.type_marker {
            result.task_theme = Some(
                themes
                    .apply_type_marker_to_existing(
                        planner, &target, route, block_id, marker, warnings,
                    )?
                    .summary,
            );
        }
        let mut item = PlannedCaptureItem {
            result,
            clip_plan: None,
            pomodoro_refs: Vec::new(),
            task_block_refs: Vec::new(),
            project_move_landings: Vec::new(),
            req_owners: Vec::new(),
            req_closing: None,
        };
        add_to_item(
            request, &mut item, clock, when, planner, ctx, themes, warnings,
        )?;
        Ok(item)
    } else {
        if parsed.type_marker.is_some() {
            return Err(CaptureError::usage("drop the snooze row with @route:id! alone; remove the type marker"));
        }
        let identity = TaskIdentity {
            route: relative.clone(),
            block_id: block_id.to_string(),
        };
        let identities =
            identity_map(ctx, planner, &request.bob_dir, day_relative, &day);
        let plan = core::plan_drop(&DropInput { day: &day, identity: &identity, identities: &identities })
            .map_err(|_| CaptureError::usage_code("task is not snoozed: no active Snoozed row; use @route:id z:<when> to snooze it", "not_snoozed"))?;
        guard_inputs(planner, &request.bob_dir)?;
        planner.stage(&day_path, plan.day)?;
        result.snooze = Some(SnoozeJson {
            action: "drop",
            time: None,
            wake_at: None,
            route: display_relative(&relative),
            block_id: block_id.to_string(),
            daily_file: display_relative(day_relative),
            row: None,
        });
        result.day_file = Some(day_path.display().to_string());
        Ok(PlannedCaptureItem {
            result,
            clip_plan: None,
            pomodoro_refs: Vec::new(),
            task_block_refs: Vec::new(),
            project_move_landings: Vec::new(),
            req_owners: Vec::new(),
            req_closing: None,
        })
    }
}

pub(super) fn finish_link_now(
    request: &CaptureRequest,
    item: &mut PlannedCaptureItem,
    planner: &mut CaptureBatchPlanner,
    ctx: &DependencyContext,
) -> Result<(), CaptureError> {
    let Some(destination) = item.result.pomodoro_link_destination.as_ref()
    else {
        return Ok(());
    };
    let Some(block_id) = item.result.block_id.as_deref() else {
        return Ok(());
    };
    let day_path = pomodoro::day_file_for(&request.bob_dir);
    let Some(day) = planner.peek_text(&day_path) else {
        return Ok(());
    };
    if parse_section(&day.lines().collect::<Vec<_>>()).is_none() {
        return Ok(());
    }
    guard_catalog(ctx, planner, &request.bob_dir)?;
    let day_relative =
        day_path.strip_prefix(&request.bob_dir).unwrap_or(&day_path);
    let identity = TaskIdentity {
        route: PathBuf::from(&item.result.relative_target),
        block_id: block_id.to_string(),
    };
    let identities =
        identity_map(ctx, planner, &request.bob_dir, day_relative, &day);
    if let Some(updated) =
        core::plan_link_now(&day, &identity, &identities, destination.line)
    {
        guard_inputs(planner, &request.bob_dir)?;
        // Pruning a hand-written earlier Snoozed section can shift the
        // ledger. The operation preserves every entry, so ordinal pairing
        // remaps the actual destination and the block tracker's postimages.
        let before = capture_pomodoros::scan(&day);
        let after = capture_pomodoros::scan(&updated);
        let lines: BTreeMap<usize, usize> = before
            .entries
            .iter()
            .zip(&after.entries)
            .map(|(a, b)| (a.line, b.line))
            .collect();
        if let Some(destination) =
            item.result.pomodoro_link_destination.as_mut()
            && let Some(line) = lines.get(&destination.line)
        {
            destination.line = *line;
        }
        if let Some(start) = item.result.pomodoro_start.as_mut()
            && let Some(line) = lines.get(&start.pomodoro_line)
        {
            start.pomodoro_line = *line;
        }
        for reference in &mut item.pomodoro_refs {
            if let Some(after) = reference.after.as_mut()
                && let Some(line) = lines.get(&(*after + 1))
            {
                *after = *line - 1;
            }
        }
        planner.stage(&day_path, updated)?;
        item.result.snooze = Some(SnoozeJson {
            action: "link_now",
            time: None,
            wake_at: None,
            route: display_relative(&identity.route),
            block_id: block_id.to_string(),
            daily_file: display_relative(day_relative),
            row: None,
        });
    }
    Ok(())
}

pub(super) fn refresh_results(
    items: &mut [PlannedCaptureItem],
    planner: &mut CaptureBatchPlanner,
    bob_dir: &Path,
    ctx: &DependencyContext,
) -> Result<(), CaptureError> {
    for item in items {
        let Some(snooze) = item.result.snooze.as_mut() else {
            continue;
        };
        if matches!(snooze.action, "add" | "retime") {
            let path = PathBuf::from(&snooze.daily_file);
            let day = planner.read_existing(&bob_dir.join(&path))?;
            let identities = identity_map(ctx, planner, bob_dir, &path, &day);
            let identity = TaskIdentity {
                route: PathBuf::from(&snooze.route),
                block_id: snooze.block_id.clone(),
            };
            snooze.row = core::active_row_line(&day, &identity, &identities);
        }
    }
    Ok(())
}

fn existing_result(
    request: &CaptureRequest,
    relative: &Path,
    task: &note_tasks::NoteTask,
    contents: &str,
    today: NaiveDate,
) -> CaptureItemResult {
    CaptureItemResult {
        snooze: None,
        note_pull: None,
        record_note: None,
        notebook: None,
        requirements: Vec::new(),
        task_theme: None,
        type_relocation: None,
        ok: true,
        dry_run: request.dry_run,
        routed: true,
        route: Some(route_without_extension(relative)),
        route_label: display_relative(relative),
        relative_target: display_relative(relative),
        target: request.bob_dir.join(relative).display().to_string(),
        text: task.description.clone(),
        task_line: contents
            .lines()
            .nth(task.line_index)
            .unwrap_or_default()
            .to_string(),
        kind: "snooze",
        fanout: None,
        created: date_string(today),
        scheduled: None,
        priority: None,
        priority_label: None,
        placement: Placement::Updated,
        sub_bullets: Vec::new(),
        notes: Vec::new(),
        drawers: Vec::new(),
        clip: None,
        schedule_log: None,
        block_id: task.block_id.clone(),
        day_file: None,
        block_link: None,
        pomodoro_link_placement: None,
        parent_line: None,
        parent_text: None,
        drawer: None,
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
        project_note: None,
        pomodoro_start: None,
        pomodoro_adjust: None,
        pomodoro_shift: None,
        pomodoro_split: None,
        pomodoro_merge: None,
        pomodoro_close: None,
        pomodoro_past: None,
        pomodoro_reset: None,
        dependency_update: None,
        consumed_links: Vec::new(),
        task_complete: None,
        r#ref: None,
        gmail: Vec::new(),
        toggle_task_description: None,
        link_guard: None,
        dropped_dependencies: Vec::new(),
        blocks_line_updates: Vec::new(),
        minted_dependent_block_id: None,
        dependency_updates: Vec::new(),
        minted_blocker_block_id: None,
        relation_updates: Vec::new(),
        relation_cancels: Vec::new(),
        relation_completes: Vec::new(),
        relation_merge_output: Default::default(),
        transfer_move_output: Default::default(),
        linked_from_line_updates: Vec::new(),
        minted_relation_block_ids: Vec::new(),
        mirror_skipped: Vec::new(),
        list: None,
        relation_label_hides: Vec::new(),
        relation_successors: Vec::new(),
        relation_held: Vec::new(),
        relation_check: None,
        relation_groups: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snooze_batch_guards_task_daily_membership_and_eligibility_reads() {
        for capture_text in ["@jobs:work z:1530", "@jobs:work"] {
            for drift in ["task", "day", "prerequisite", "membership"] {
                let dir = tempfile::tempdir().unwrap();
                let day = dir.path().join("day.md");
                let task = dir.path().join("jobs.md");
                let prerequisite = dir.path().join("pre.md");
                let config = dir.path().join("bob.yml");
                let task_before =
                "- [ ] #task Work ^work\n  - ⛓️ **DEPENDS ON:** [[pre#^pre]]\n";
                let day_before = "## Pomodoros\n- [ ] ()\n\n## Snoozed\n- **1500** [[jobs#^work]]\n\t- carried note\n";
                fs::write(&day, day_before).unwrap();
                fs::write(&task, task_before).unwrap();
                fs::write(&prerequisite, "- [x] #task Finished ^pre\n")
                    .unwrap();
                fs::write(&config, "timezone: America/New_York\n").unwrap();
                let _env = bob_env::TestEnvGuard::set(&[
                    ("BOB_DAY_FILE", Some(day.as_os_str())),
                    ("BOB_CONFIG_FILE", Some(config.as_os_str())),
                    (
                        "BOB_NOW",
                        Some(std::ffi::OsStr::new("2026-11-25 12:00:00")),
                    ),
                ]);
                let request = CaptureRequest {
                    bob_dir: dir.path().to_path_buf(),
                    dry_run: false,
                    forced_clip: None,
                    forced_destination_flags: Vec::new(),
                    forced_route: None,
                    forced_section: None,
                    forced_sub_bullet_target: None,
                    forced_drawer: None,
                    inbox_routes: Vec::new(),
                    move_in: None,
                    mail_close: None,
                    no_clip: true,
                    no_ref: true,
                    pull_in: None,
                    jump_offer: None,
                    complete_offers: Vec::new(),
                    cancel_offers: Vec::new(),
                    offer_verdicts: Vec::new(),
                    carry: Vec::new(),
                    refuse_override: false,
                    require_inbox_route_choice: false,
                    require_pull_in_choice: false,
                    expect_past_windows: None,
                    argv: false,
                    template_origin: None,
                    raw_text: capture_text.to_string(),
                    req_choice: false,
                    req_selection: None,
                    selection:
                        crate::native::task_complete::SuccessorSelection::None,
                };
                let batch = plan_capture_batch(&request, None)
                    .expect("snooze plans with closed prerequisite");
                assert_eq!(
                    batch.items[0].result.snooze.as_ref().map(|s| s.action),
                    Some(if capture_text.contains(" z:") {
                        "retime"
                    } else {
                        "link_now"
                    }),
                    "{capture_text}: fixture must exercise a snooze mutation"
                );
                match drift {
                    "task" => {
                        fs::write(&task, "- [ ] #task External edit ^work\n")
                    }
                    "day" => fs::write(&day, "## Pomodoros\nexternal edit\n"),
                    "prerequisite" => {
                        fs::write(&prerequisite, "- [ ] #task Reopened ^pre\n")
                    }
                    _ => fs::write(dir.path().join("new.md"), "new member\n"),
                }
                .unwrap();
                let outcome =
                    write_staged_files_guarded(&batch.text_files, || {
                        batch.read_guards.validate(dir.path())
                    });
                assert!(outcome.is_err(), "{capture_text}/{drift}: stale snooze must refuse the entire batch");
                let error = outcome.unwrap_err();
                assert_eq!(
                    error.typed,
                    Some(CaptureFailureKind::NoteChanged),
                    "{drift}: {error:?}"
                );
                if drift != "task" {
                    assert_eq!(fs::read_to_string(&task).unwrap(), task_before);
                }
                if drift != "day" {
                    assert_eq!(fs::read_to_string(&day).unwrap(), day_before);
                }
            }
        }
    }
}
