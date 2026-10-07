//! Whole-item `!note:block-id` execution through the shared engine.
use super::*;
use crate::native::{
    capture_pomodoro_close::CloseVault, task_complete as engine,
    task_dependencies, vault_links::LinkResolution,
};
use std::collections::{BTreeMap, BTreeSet};

/// Plan one `TaskComplete` item: resolve vault-wide, validate status and
/// recurrence, run the engine (tree close, scoped ledger retirement,
/// dependent recovery), and stage every changed file.
#[allow(clippy::too_many_arguments)]
pub(super) fn plan_task_complete_item(
    request: &CaptureRequest,
    parsed: &ParsedCaptureText,
    raw: &str,
    note: &str,
    block_id: &str,
    today: NaiveDate,
    planner: &mut CaptureBatchPlanner,
    dependency_ctx: &mut DependencyContext,
) -> Result<PlannedCaptureItem, CaptureError> {
    reject_task_complete_conflicts(parsed, request, raw)?;
    let bob_dir = &request.bob_dir;
    let settings = note_tasks::read_settings(bob_dir);
    // Resolve the note vault-wide, exactly like `&`: an explicit relative
    // path first, else a unique basename. Notes staged earlier in the
    // batch also count.
    let relative =
        resolve_prerequisite_note(dependency_ctx, planner, bob_dir, note)?;
    let absolute = bob_dir.join(&relative);
    let rel_display = display_relative(&relative);
    // Current staged text for the note.
    let contents = current_note_text(planner, &absolute)?;
    let scan = note_tasks::scan(&contents, &settings);
    let task = lookup_staged_task(&scan, &rel_display, block_id)?;
    let task_line_index = task.line_index;
    let previous_task_line = contents
        .lines()
        .nth(task_line_index)
        .unwrap_or_default()
        .to_string();
    let previous_status_symbol = task.status_symbol;
    let previous_status_name = task.status_name.clone();
    let task_description = task.description.clone();
    // Validate the status before any staging.
    if is_done_status(task.status_symbol, &settings) {
        return Ok(already_done_item(
            request,
            raw,
            note,
            &relative,
            &absolute,
            block_id,
            &previous_task_line,
            previous_status_symbol,
            &previous_status_name,
            &task_description,
            today,
        ));
    }
    if task.status_symbol == '-'
        || settings.status_types.get(&task.status_symbol)
            == Some(
                &crate::native::task_status_hooks::TaskStatusType::Cancelled,
            )
    {
        return Err(CaptureError::io(format!(
            "`^{block_id}` in {rel_display} is Canceled; reopen it before completing it"
        )));
    }
    if !engine::is_completable_status(task.status_symbol) {
        return Err(CaptureError::io(format!(
            "`^{block_id}` in {rel_display} has status `[{}]`; only Ready, Blocked, Next, and In Progress tasks can be completed",
            task.status_symbol
        )));
    }
    if engine::is_recurring_task_line(&previous_task_line) {
        return Err(CaptureError::io(format!(
            "`^{block_id}` in {rel_display} repeats; complete recurring tasks in Obsidian so Tasks writes the next occurrence"
        )));
    }
    let completion_date = date_string(today);
    // No staged preimage recheck here: `contents` above already read the
    // batch's staged text, and nothing staged this note in between, so a
    // fresh re-read would compare the value with itself. The guard against
    // external edits is the shared disk-preimage validation in
    // `write_staged_files` (commit.rs), which refuses the batch when
    // current disk bytes differ from the planned preimage. Temporary files
    // and rollback only make the commit atomic; they do not detect
    // external edits.
    // Day file identity up front so pre-item coordinates are available
    // for pomodoro-block removal refs below.
    let day_file = pomodoro::day_file_for(bob_dir);
    let day_relative = capture_pomodoros::relative_day_file(&day_file, bob_dir);
    let pre_day = planner.peek_text(&day_file);
    // Close the tree through the staged snapshot vault.
    let vault = SnapshotCloseVault::from_planner(planner, bob_dir);
    let outcome = engine::complete_task_tree(
        &vault,
        &absolute,
        block_id,
        &completion_date,
        engine::RootPolicy::Explicit,
    )
    .map_err(CaptureError::io)?;
    let Some(root) = outcome.root.clone() else {
        return Err(CaptureError::io(
            "task complete capture invariant failed: root did not close"
                .to_string(),
        ));
    };
    // Stage tree post-images. No per-file preimage recheck: the tree
    // close read the same staged snapshot, and nothing else staged these
    // paths in between, so the check would compare staged text with
    // itself. The shared disk-preimage validation in `write_staged_files`
    // (commit.rs) remains the guard against external edits; rollback only
    // restores already-replaced targets on failure.
    for (path, text) in &outcome.changed_files {
        planner.stage(path, text.clone())?;
    }
    // Completed identities for retirement + recovery (absolute paths).
    let mut completed: BTreeSet<(PathBuf, String)> = BTreeSet::new();
    completed.insert((root.absolute_path.clone(), root.block_id.clone()));
    for closed in &outcome.closed_subtasks {
        completed
            .insert((closed.absolute_path.clone(), closed.block_id.clone()));
    }
    // Scoped ledger retirement over the post-tree staged day file.
    let day_exists = planner.currently_exists(&day_file)? || day_file.is_file();
    let mut ledger_json: Option<TaskCompleteLedgerJson> = None;
    let mut pomodoro_refs: Vec<PomodoroBlockRef> = Vec::new();
    if day_exists && let Some(day_text) = planner.current_contents(&day_file)? {
        let link_statuses =
            compute_link_statuses(bob_dir, planner, &day_file, &completed);
        let tasks_settings =
            crate::native::task_status_hooks::read_tasks_settings(bob_dir);
        let retirement = engine::retire_completed_links(
            &day_text,
            &completed,
            &link_statuses,
            &tasks_settings,
        );
        if retirement.changed {
            planner.stage(&day_file, retirement.text.clone())?;
        }
        if retirement.changed
            || retirement.struck > 0
            || !retirement.struck_in.is_empty()
            || !retirement.moved.is_empty()
            || !retirement.deduplicated.is_empty()
            || !retirement.removed_placeholders.is_empty()
        {
            pomodoro_refs.extend(removal_refs(
                pre_day.as_deref(),
                &day_text,
                &retirement,
            ));
            ledger_json = Some(ledger_json_for(
                bob_dir,
                &day_text,
                &day_relative,
                &retirement,
            ));
        }
    }
    // Blocked-dependent recovery over the staged snapshot (post-tree,
    // post-retirement). The on-disk vault walk is built once per batch
    // and cached on the batch context; staged overlays are applied per
    // item so recovery still sees earlier items' staged edits.
    let snapshot =
        staged_snapshot_for_recovery(bob_dir, planner, dependency_ctx);
    let mut completed_ids: BTreeSet<String> = BTreeSet::new();
    for (path, id) in &completed {
        let relative_path = path
            .strip_prefix(bob_dir)
            .map(Path::to_path_buf)
            .unwrap_or_else(|_| path.clone());
        let text = snapshot
            .iter()
            .find(|(rel, _)| rel == &relative_path)
            .map(|(_, text)| text.as_str())
            .unwrap_or("");
        let line = text
            .lines()
            .nth(completed_line_hint(&outcome, path, id).unwrap_or(0));
        if let Some(line) = line {
            let metadata =
                crate::native::task_status_hooks::task_metadata(line, Some(id));
            if let Some(task_id) = metadata.task_id {
                completed_ids.insert(task_id);
            }
        }
        if let Ok(canonical) =
            task_dependencies::dependency_id(&relative_path, id)
        {
            completed_ids.insert(canonical);
        }
    }
    let recovery = engine::recover_blocked_dependents(
        snapshot,
        &completed_ids,
        today,
        &crate::native::task_status_hooks::read_tasks_settings(bob_dir),
    );
    for (relative_path, text) in &recovery.changed_files {
        let absolute_path = bob_dir.join(relative_path);
        planner.stage(&absolute_path, text.clone())?;
    }
    // Final post-images for JSON + task blocks.
    let final_note_text =
        planner.current_contents(&absolute)?.unwrap_or_default();
    let final_scan = note_tasks::scan(&final_note_text, &settings);
    let final_task = match final_scan.by_block_id(block_id) {
        BlockIdLookup::Found(task) => task,
        _ => {
            return Err(CaptureError::io(
                "task complete capture invariant failed: completed task disappeared"
                    .to_string(),
            ));
        }
    };
    let task_line = final_note_text
        .lines()
        .nth(final_task.line_index)
        .unwrap_or_default()
        .to_string();
    let created = date_string(today);
    let relative_target = rel_display.clone();
    let target = absolute.display().to_string();
    let route_label_value = rel_display.clone();
    let mut without_extension = relative.clone();
    without_extension.set_extension("");
    let _route_value = display_relative(&without_extension);
    // Subtasks JSON in close order. Engine descriptions keep the
    // `#task` tag (looked up with the global filter cleared) and
    // recovery descriptions keep inline fields, so clean both for
    // display, the way the close planner's `close_task_text` does.
    let subtasks = outcome
        .closed_subtasks
        .iter()
        .map(|closed| TaskCompleteSubtaskJson {
            note_path: display_relative(&PathBuf::from(
                closed.relative_path.clone(),
            )),
            block_id: closed.block_id.clone(),
            line: closed.line,
            text: note_tasks::clean_description(
                &closed.text,
                &settings.global_filter,
                Some(&closed.block_id),
            ),
            previous_status_symbol: closed.previous_status_symbol,
            previous_status_name: closed.previous_status_name.clone(),
            status_symbol: closed.status_symbol,
            status_name: closed.status_name.clone(),
        })
        .collect::<Vec<_>>();
    let left_open = outcome
        .left_open
        .iter()
        .map(|left| TaskCompleteLeftOpenJson {
            note_path: display_relative(&PathBuf::from(
                left.relative_path.clone(),
            )),
            block_id: left.block_id.clone(),
            line: left.line,
            text: note_tasks::clean_description(
                &left.text,
                &settings.global_filter,
                Some(&left.block_id),
            ),
            status_symbol: left.status_symbol,
            status_name: left.status_name.clone(),
            reason: left.reason.as_str().to_string(),
        })
        .collect::<Vec<_>>();
    let unblocked = recovery
        .recovered
        .iter()
        .map(|dependent| TaskCompleteUnblockedJson {
            note_path: display_relative(&dependent.relative_path),
            block_id: dependent.block_id.clone().unwrap_or_default(),
            line: dependent.line,
            text: note_tasks::clean_description(
                &dependent.text,
                &settings.global_filter,
                dependent.block_id.as_deref(),
            ),
            previous_status_symbol: dependent.previous_status_symbol,
            previous_status_name: status_name_for(
                bob_dir,
                dependent.previous_status_symbol,
            ),
            status_symbol: dependent.status_symbol,
            status_name: status_name_for(bob_dir, dependent.status_symbol),
        })
        .collect::<Vec<_>>();
    // Clean display text for the root task with the configured global
    // filter, inline fields, and trailing block ID removed: the same
    // string the human output prints.
    let root_text = task_complete_display_text(
        &task_line,
        &settings.global_filter,
        Some(block_id),
    );
    let summary = TaskCompleteSummaryJson {
        raw: raw.to_string(),
        note: note.to_string(),
        note_path: rel_display.clone(),
        block_id: block_id.to_string(),
        action: "completed",
        completion_date: Some(completion_date.clone()),
        text: root_text,
        subtasks,
        subtasks_left_open: left_open,
        ledger: ledger_json,
        unblocked: unblocked.clone(),
    };
    // Task blocks: completed (root + closed subtasks) + unblocked.
    let mut task_block_refs = Vec::new();
    push_completed_ref(
        &mut task_block_refs,
        bob_dir,
        &root.absolute_path,
        &root.relative_path,
        root.line.saturating_sub(1),
        &root.block_id,
    );
    for closed in &outcome.closed_subtasks {
        push_completed_ref(
            &mut task_block_refs,
            bob_dir,
            &closed.absolute_path,
            &closed.relative_path,
            closed.line.saturating_sub(1),
            &closed.block_id,
        );
    }
    for dependent in &recovery.recovered {
        let absolute_path = bob_dir.join(&dependent.relative_path);
        let rel = display_relative(&dependent.relative_path);
        let mut stem = dependent.relative_path.clone();
        stem.set_extension("");
        push_unblocked_ref(
            &mut task_block_refs,
            absolute_path,
            rel,
            display_relative(&stem),
            dependent.line.saturating_sub(1),
            dependent.block_id.clone(),
        );
    }
    Ok(PlannedCaptureItem {
        result: CaptureItemResult {
            ok: true,
            dry_run: request.dry_run,
            routed: true,
            route: None,
            route_label: route_label_value,
            relative_target: relative_target.clone(),
            target: target.clone(),
            text: String::new(),
            task_line,
            kind: "task_complete",
            created,
            scheduled: None,
            priority: None,
            priority_label: None,
            placement: Placement::Completed,
            sub_bullets: Vec::new(),
            clip: None,
            schedule_log: None,
            block_id: Some(block_id.to_string()),
            day_file: None,
            block_link: None,
            pomodoro_link_placement: None,
            parent_line: None,
            parent_text: None,
            parent_section: None,
            parent_status_symbol: None,
            parent_status_name: None,
            toggle_direction: None,
            previous_task_line: Some(previous_task_line),
            status_symbol: Some(final_task.status_symbol),
            status_name: Some(final_task.status_name.clone()),
            previous_status_symbol: Some(previous_status_symbol),
            previous_status_name: Some(previous_status_name),
            pomodoro_name: None,
            creates_pomodoro: None,
            pomodoro_already_linked: None,
            removed_pomodoro_links: None,
            removed_scheduled: None,
            pomodoro_selector_unused: None,
            toggle_behavior: None,
            status_changed: Some(
                previous_status_symbol != final_task.status_symbol,
            ),
            pomodoro_link_action: None,
            pomodoro_link_source: None,
            pomodoro_link_destination: None,
            project_note: None,
            pomodoro_start: None,
            pomodoro_adjust: None,
            pomodoro_shift: None,
            pomodoro_close: None,
            dependency_update: None,
            toggle_task_description: None,
            r#ref: None,
            task_complete: Some(summary),
        },
        clip_plan: None,
        pomodoro_refs,
        task_block_refs,
    })
}

/// Pomodoro-block removal refs for placeholders retirement deleted, in
/// pre-item coordinates: match each removed entry's name against the
/// pre-item scan, falling back to the retirement line number.
fn removal_refs(
    pre_day: Option<&str>,
    pre_retirement_day: &str,
    retirement: &engine::LedgerRetirement,
) -> Vec<PomodoroBlockRef> {
    if retirement.removed_placeholders.is_empty() {
        return Vec::new();
    }
    // Prefer pre-item coordinates (what the block tracker compares
    // against); fall back to pre-retirement coordinates when the day
    // file was untouched before this item.
    let pre_text = pre_day.unwrap_or(pre_retirement_day);
    let pre_scan = crate::native::capture_pomodoros::scan(pre_text);
    let pre_lines: Vec<&str> = pre_text.lines().collect();
    retirement
        .removed_placeholders
        .iter()
        .filter_map(|removed| {
            let wanted = removed.line.trim_end().to_string();
            // Match by full entry line first, then by short name.
            let headline = pre_scan
                .entries
                .iter()
                .find(|entry| {
                    pre_lines
                        .get(entry.line.saturating_sub(1))
                        .is_some_and(|line| line.trim_end() == wanted)
                })
                .map(|entry| entry.line.saturating_sub(1))
                .or_else(|| {
                    // Fall back to the retirement line number against
                    // whichever snapshot has that many lines.
                    let fallback = removed.line_number.saturating_sub(1);
                    (fallback < pre_lines.len()).then_some(fallback)
                })?;
            Some(PomodoroBlockRef::removed(headline))
        })
        .collect()
}

#[allow(clippy::too_many_arguments)]
fn already_done_item(
    request: &CaptureRequest,
    raw: &str,
    note: &str,
    relative: &Path,
    absolute: &Path,
    block_id: &str,
    previous_task_line: &str,
    previous_status_symbol: char,
    previous_status_name: &str,
    task_description: &str,
    today: NaiveDate,
) -> PlannedCaptureItem {
    let _ = task_description;
    let rel_display = display_relative(relative);
    let mut without_extension = relative.to_path_buf();
    without_extension.set_extension("");
    let route_value = display_relative(&without_extension);
    let _ = route_value;
    // Clean display text with the configured global filter, matching the
    // human already-done line and the completed path.
    let global_filter =
        note_tasks::read_settings(&request.bob_dir).global_filter;
    let clean_text = task_complete_display_text(
        previous_task_line,
        &global_filter,
        Some(block_id),
    );
    PlannedCaptureItem {
        result: CaptureItemResult {
            ok: true,
            dry_run: request.dry_run,
            routed: true,
            route: None,
            route_label: rel_display.clone(),
            relative_target: rel_display,
            target: absolute.display().to_string(),
            text: String::new(),
            task_line: previous_task_line.to_string(),
            kind: "task_complete",
            created: date_string(today),
            scheduled: None,
            priority: None,
            priority_label: None,
            placement: Placement::Completed,
            sub_bullets: Vec::new(),
            clip: None,
            schedule_log: None,
            block_id: Some(block_id.to_string()),
            day_file: None,
            block_link: None,
            pomodoro_link_placement: None,
            parent_line: None,
            parent_text: None,
            parent_section: None,
            parent_status_symbol: None,
            parent_status_name: None,
            toggle_direction: None,
            previous_task_line: Some(previous_task_line.to_string()),
            status_symbol: Some(previous_status_symbol),
            status_name: Some(previous_status_name.to_string()),
            previous_status_symbol: Some(previous_status_symbol),
            previous_status_name: Some(previous_status_name.to_string()),
            pomodoro_name: None,
            creates_pomodoro: None,
            pomodoro_already_linked: None,
            removed_pomodoro_links: None,
            removed_scheduled: None,
            pomodoro_selector_unused: None,
            toggle_behavior: None,
            status_changed: Some(false),
            pomodoro_link_action: None,
            pomodoro_link_source: None,
            pomodoro_link_destination: None,
            project_note: None,
            pomodoro_start: None,
            pomodoro_adjust: None,
            pomodoro_shift: None,
            pomodoro_close: None,
            dependency_update: None,
            toggle_task_description: None,
            r#ref: None,
            task_complete: Some(TaskCompleteSummaryJson {
                raw: raw.to_string(),
                note: note.to_string(),
                note_path: display_relative(relative),
                block_id: block_id.to_string(),
                action: "already_done",
                completion_date: None,
                text: clean_text,
                subtasks: Vec::new(),
                subtasks_left_open: Vec::new(),
                ledger: None,
                unblocked: Vec::new(),
            }),
        },
        clip_plan: None,
        pomodoro_refs: Vec::new(),
        task_block_refs: Vec::new(),
    }
}

pub(super) fn reject_task_complete_conflicts(
    parsed: &ParsedCaptureText,
    request: &CaptureRequest,
    raw: &str,
) -> Result<(), CaptureError> {
    if !request.forced_destination_flags.is_empty()
        || request.forced_route.is_some()
        || request.forced_section.is_some()
        || request.forced_sub_bullet_target.is_some()
        || request.forced_task_section.is_some()
        || request.forced_clip.is_some()
        || parsed.clip.is_some()
        || parsed.scheduled_offset.is_some()
        || parsed.priority_level.is_some()
        || !parsed.sub_bullets.is_empty()
        || !parsed.dependencies.is_empty()
    {
        return Err(CaptureError::usage(format!(
            "`{raw}` completes an existing task and must be the whole capture item; remove its forced destination flags"
        )));
    }
    Ok(())
}

fn is_done_status(
    symbol: char,
    settings: &note_tasks::NoteTaskSettings,
) -> bool {
    settings.done_statuses.contains(&symbol)
        || settings.status_types.get(&symbol)
            == Some(&crate::native::task_status_hooks::TaskStatusType::Done)
}

fn status_name_for(bob_dir: &Path, symbol: char) -> String {
    let settings = note_tasks::read_settings(bob_dir);
    settings
        .status_definitions
        .iter()
        .find(|definition| {
            let mut characters = definition.symbol.chars();
            characters.next() == Some(symbol) && characters.next().is_none()
        })
        .map(|definition| definition.name.clone())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| match symbol {
            ' ' => "Ready".to_string(),
            '?' => "Blocked".to_string(),
            '*' => "Next".to_string(),
            '/' => "In Progress".to_string(),
            'x' | 'X' => "Done".to_string(),
            '-' => "Canceled".to_string(),
            _ => "Unknown".to_string(),
        })
}

fn push_completed_ref(
    refs: &mut Vec<TaskBlockRef>,
    bob_dir: &Path,
    absolute: &Path,
    relative_str: &str,
    line: usize,
    block_id: &str,
) {
    let relative = PathBuf::from(relative_str);
    let mut stem = relative.clone();
    stem.set_extension("");
    refs.push(TaskBlockRef {
        target: absolute.to_path_buf(),
        relative_target: display_relative(&relative),
        route: display_relative(&stem),
        line,
        block_id: Some(block_id.to_string()),
        role: TaskBlockRole::Completed,
    });
    let _ = bob_dir;
}

fn push_unblocked_ref(
    refs: &mut Vec<TaskBlockRef>,
    absolute: PathBuf,
    relative_target: String,
    route: String,
    line: usize,
    block_id: Option<String>,
) {
    refs.push(TaskBlockRef {
        target: absolute,
        relative_target,
        route,
        line,
        block_id,
        role: TaskBlockRole::Unblocked,
    });
}

fn completed_line_hint(
    outcome: &engine::CompleteTreeOutcome,
    path: &Path,
    id: &str,
) -> Option<usize> {
    if outcome
        .root
        .as_ref()
        .is_some_and(|root| root.absolute_path == path && root.block_id == id)
    {
        return outcome
            .root
            .as_ref()
            .map(|root| root.line.saturating_sub(1));
    }
    outcome
        .closed_subtasks
        .iter()
        .find(|closed| closed.absolute_path == path && closed.block_id == id)
        .map(|closed| closed.line.saturating_sub(1))
}

/// Link liveness for today's staged day file, resolved through the
/// staged snapshot vault: `Done` for this batch's completed identities,
/// `Live` for links to open tasks, omitted for anything else.
fn compute_link_statuses(
    bob_dir: &Path,
    planner: &CaptureBatchPlanner,
    day_file: &Path,
    completed: &BTreeSet<(PathBuf, String)>,
) -> BTreeMap<crate::native::task_status_hooks::RawReference, engine::LinkStatus>
{
    use crate::native::task_status_hooks::{logical_lines, scan_pomodoros};
    let mut statuses = BTreeMap::new();
    let day_text = match planner.peek_text(day_file) {
        Some(text) => text,
        None => {
            if !day_file.is_file() {
                return statuses;
            }
            match std::fs::read_to_string(day_file) {
                Ok(text) => text,
                Err(_) => return statuses,
            }
        }
    };
    let lines = logical_lines(&day_text);
    let Some(section) =
        crate::native::pomodoro::pomodoros_section_range(&lines)
    else {
        return statuses;
    };
    let model = scan_pomodoros(&lines, section);
    let vault = SnapshotCloseVault::from_planner(planner, bob_dir);
    let settings = note_tasks::read_settings(bob_dir);
    for reference in model.all_references.iter() {
        let resolved = match vault.resolve_target(day_file, &reference.target) {
            LinkResolution::Found(path) => path,
            _ => continue,
        };
        if completed.contains(&(resolved.clone(), reference.block_id.clone())) {
            statuses.insert(
                reference.clone(),
                engine::LinkStatus::Done { path: resolved },
            );
            continue;
        }
        let contents = match vault.read_latest(&resolved) {
            Ok(Some(contents)) => contents,
            _ => continue,
        };
        let scan = note_tasks::scan(&contents, &settings);
        let task = match scan.by_block_id(&reference.block_id) {
            BlockIdLookup::Found(task) => task,
            _ => continue,
        };
        if is_done_status(task.status_symbol, &settings) {
            // Done elsewhere and not yet reconciled: untouched.
            continue;
        }
        statuses.insert(
            reference.clone(),
            engine::LinkStatus::Live {
                path: resolved,
                status: task.status_symbol,
            },
        );
    }
    // Also cover staged-new notes the resolver cannot see yet.
    statuses
}

/// Snapshot of vault notes with staged overlays for dependent recovery:
/// the batch's cached on-disk markdown walk plus every staged file.
/// The on-disk walk is built at most once per batch (lazily, only when a
/// `!` item needs recovery) and cached on the batch context next to
/// `DependencyContext`; staged overlays are applied per item so recovery
/// still sees earlier items' staged edits.
fn staged_snapshot_for_recovery(
    bob_dir: &Path,
    planner: &CaptureBatchPlanner,
    dependency_ctx: &mut DependencyContext,
) -> Vec<(PathBuf, String)> {
    let mut staged: BTreeMap<PathBuf, String> = BTreeMap::new();
    for (absolute, contents) in planner.staged_snapshot() {
        let Some(contents) = contents else { continue };
        let Ok(relative) =
            absolute.strip_prefix(bob_dir).map(Path::to_path_buf)
        else {
            continue;
        };
        if relative
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("md"))
        {
            staged.insert(relative, contents);
        }
    }
    let mut snapshot: BTreeMap<PathBuf, String> =
        dependency_ctx.recovery_base_snapshot(bob_dir);
    for (relative, contents) in staged {
        snapshot.insert(relative, contents);
    }
    snapshot.into_iter().collect()
}

fn ledger_json_for(
    bob_dir: &Path,
    pre_day_text: &str,
    day_relative: &str,
    retirement: &engine::LedgerRetirement,
) -> TaskCompleteLedgerJson {
    // Map entry lines to short names + running/queued/completed via the
    // capture pomodoro scan of the pre-retirement day text. When an entry
    // has no name, `name` is "" and the human output uses `line N`.
    let scan = crate::native::capture_pomodoros::scan(pre_day_text);
    let entry_info = |line: usize| -> (String, String) {
        if let Some(entry) =
            scan.entries.iter().find(|entry| entry.line == line)
        {
            let name = entry.name.clone().unwrap_or_default();
            let status = if entry.state
                == crate::native::capture_pomodoros::PomodoroState::Completed
            {
                "completed"
            } else if entry.time_range.is_some() {
                "running"
            } else {
                "queued"
            };
            (name, status.to_string())
        } else {
            (String::new(), "queued".to_string())
        }
    };
    let endpoint_for = |line: usize| -> TaskCompleteLedgerEndpointJson {
        let (name, status) = entry_info(line);
        TaskCompleteLedgerEndpointJson { line, name, status }
    };
    let struck_in = retirement
        .struck_in
        .iter()
        .map(|entry| endpoint_for(entry.line))
        .collect();
    let moved = retirement
        .moved
        .iter()
        .map(|item| TaskCompleteLedgerMoveJson {
            from: endpoint_for(item.source_line),
            to: endpoint_for(item.destination_line),
        })
        .collect();
    let dropped = retirement
        .deduplicated
        .iter()
        .map(|item| TaskCompleteLedgerDroppedJson {
            from: endpoint_for(item.source_line),
            to: endpoint_for(item.destination_line),
        })
        .collect();
    let removed_placeholders = retirement
        .removed_placeholders
        .iter()
        .map(|removed| {
            let (name, _) = entry_info(removed.line_number);
            let name = if name.is_empty() {
                removed.line.clone()
            } else {
                name
            };
            TaskCompleteRemovedPlaceholderJson { name }
        })
        .collect();
    let _ = bob_dir;
    TaskCompleteLedgerJson {
        day_file: day_relative.to_string(),
        struck: retirement.struck,
        struck_in,
        moved,
        deduplicated: retirement.deduplicated.len(),
        dropped,
        removed_placeholders,
    }
}
