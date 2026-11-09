//! Whole-item `!note:block-id` execution through the shared engine.
use super::*;
use crate::native::{
    capture_pomodoro_close::CloseVault, task_complete as engine,
    vault_links::LinkResolution,
};
use std::collections::{BTreeMap, BTreeSet};

/// Reusable existing-task completion for `!` and relation closes
/// (`docs/task-relations.md` §18 CP8): validates the staged source, runs
/// the shared Done tree/successor/retirement, stages every changed file,
/// and reports the existing `TaskCompleteSummaryJson` plus the transition
/// details, daily-note effect, and tracking refs adapters need. `!` token
/// parsing and its whole-item conflict check stay in
/// [`plan_task_complete_item`]; the relation adapter preflights its
/// different already-closed semantics before calling here.
pub(super) struct CompleteExistingTaskOutcome {
    pub(super) summary: TaskCompleteSummaryJson,
    #[allow(dead_code)]
    pub(super) relative: PathBuf,
    pub(super) absolute: PathBuf,
    pub(super) rel_display: String,
    pub(super) previous_task_line: String,
    pub(super) previous_status_symbol: char,
    pub(super) previous_status_name: String,
    pub(super) task_line: String,
    pub(super) final_status_symbol: char,
    pub(super) final_status_name: String,
    pub(super) top_day_file: Option<String>,
    pub(super) pomodoro_refs: Vec<PomodoroBlockRef>,
    pub(super) task_block_refs: Vec<TaskBlockRef>,
}

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
    now: chrono::NaiveDateTime,
    planner: &mut CaptureBatchPlanner,
    dependency_ctx: &mut DependencyContext,
    warnings: &mut Vec<CaptureWarning>,
) -> Result<PlannedCaptureItem, CaptureError> {
    reject_task_complete_conflicts(parsed, request, raw)?;
    let outcome = complete_existing_task(
        request,
        raw,
        note,
        block_id,
        today,
        now,
        planner,
        dependency_ctx,
        warnings,
    )?;
    let summary = outcome.summary.clone();
    let status_changed =
        outcome.previous_status_symbol != outcome.final_status_symbol;
    Ok(PlannedCaptureItem {
        result: CaptureItemResult {
            note_pull: None,
            record_note: None,
            task_theme: None,
            type_relocation: None,
            ok: true,
            dry_run: request.dry_run,
            routed: true,
            route: None,
            route_label: outcome.rel_display.clone(),
            relative_target: outcome.rel_display.clone(),
            target: outcome.absolute.display().to_string(),
            text: String::new(),
            task_line: outcome.task_line.clone(),
            kind: "task_complete",
            fanout: None,
            created: date_string(today),
            scheduled: None,
            priority: None,
            priority_label: None,
            placement: Placement::Completed,
            sub_bullets: Vec::new(),
            notes: Vec::new(),
            drawers: Vec::new(),
            clip: None,
            schedule_log: None,
            block_id: Some(block_id.to_string()),
            day_file: outcome.top_day_file.clone(),
            block_link: None,
            pomodoro_link_placement: None,
            parent_line: None,
            parent_text: None,
            drawer: None,
            parent_status_symbol: None,
            parent_status_name: None,
            toggle_direction: None,
            previous_task_line: Some(outcome.previous_task_line.clone()),
            status_symbol: Some(outcome.final_status_symbol),
            status_name: Some(outcome.final_status_name.clone()),
            previous_status_symbol: Some(outcome.previous_status_symbol),
            previous_status_name: Some(outcome.previous_status_name.clone()),
            pomodoro_name: None,
            creates_pomodoro: None,
            pomodoro_already_linked: None,
            removed_pomodoro_links: None,
            removed_scheduled: None,
            pomodoro_selector_unused: None,
            toggle_behavior: None,
            status_changed: Some(status_changed),
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
            toggle_task_description: None,
            r#ref: None,
            gmail: Vec::new(),
            task_complete: Some(summary),
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
        },
        clip_plan: None,
        pomodoro_refs: outcome.pomodoro_refs.clone(),
        task_block_refs: outcome.task_block_refs.clone(),
        project_move_landings: Vec::new(),
    })
}

/// Shared Done path for one existing source after `!` token resolution.
/// See [`CompleteExistingTaskOutcome`] for the contract.
#[allow(clippy::too_many_arguments)]
pub(super) fn complete_existing_task(
    request: &CaptureRequest,
    raw: &str,
    note: &str,
    block_id: &str,
    today: NaiveDate,
    now: chrono::NaiveDateTime,
    planner: &mut CaptureBatchPlanner,
    dependency_ctx: &mut DependencyContext,
    warnings: &mut Vec<CaptureWarning>,
) -> Result<CompleteExistingTaskOutcome, CaptureError> {
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
        let _ = task_description;
        let _ = note;
        let global_filter =
            note_tasks::read_settings(&request.bob_dir).global_filter;
        let clean_text = task_complete_display_text(
            &previous_task_line,
            &global_filter,
            Some(block_id),
        );
        let summary = TaskCompleteSummaryJson {
            raw: raw.to_string(),
            note: note.to_string(),
            note_path: rel_display.clone(),
            block_id: block_id.to_string(),
            action: "already_done",
            completion_date: None,
            text: clean_text,
            subtasks: Vec::new(),
            subtasks_left_open: Vec::new(),
            ledger: None,
            unblocked: Vec::new(),
            keep_moving: Vec::new(),
            relation_successors: Vec::new(),
            relation_held: Vec::new(),
            relation_check: None,
            relation_groups: Vec::new(),
            still_blocked: Vec::new(),
            unblocked_check: "checked",
            project_folds: Vec::new(),
            keep_moving_folds: Vec::new(),
        };
        return Ok(CompleteExistingTaskOutcome {
            summary,
            relative: relative.clone(),
            absolute: absolute.clone(),
            rel_display: rel_display.clone(),
            previous_task_line: previous_task_line.clone(),
            previous_status_symbol,
            previous_status_name: previous_status_name.clone(),
            task_line: previous_task_line.clone(),
            final_status_symbol: previous_status_symbol,
            final_status_name: previous_status_name.clone(),
            top_day_file: None,
            pomodoro_refs: Vec::new(),
            task_block_refs: Vec::new(),
        });
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
    // Successor linking (`docs/task-dependencies.md` §12) runs before
    // ledger retirement: the completed set C and its anchors are read
    // from the staged post-tree day text (before this gesture's strikes,
    // moves, or retirement), the planner recovers newly unblocked
    // dependents and inserts successor links, and retirement then strikes
    // the predecessor in place while the successor keeps the vacated
    // slot. The §12.10 gate runs first: the staged post-close texts of C's
    // notes are scanned for pull-in candidates, and the vault-wide
    // dependents snapshot is built only when some C task carries `[id::]`,
    // a candidate R carries `[dependsOn::]`, or a `[?]` R has an open
    // resident with `[dependsOn::]`; otherwise successor planning is
    // skipped with no extra reads. Dependents that name only the canonical
    // id of a target without `[id::]` are left to the hooks
    // (successor-links gate edge).
    let day_exists = planner.currently_exists(&day_file)? || day_file.is_file();
    let mut predecessors: Vec<engine::CompletedTask> = Vec::new();
    for (path, id) in &completed {
        let line_index = completed_line_hint(&outcome, path, id).unwrap_or(0);
        let staged_line = planner
            .peek_text(path)
            .as_deref()
            .and_then(|text| text.lines().nth(line_index))
            .unwrap_or_default()
            .to_string();
        let metadata = crate::native::task_status_hooks::task_metadata(
            &staged_line,
            Some(id),
        );
        // Keep root/ancestry data separate from the dependency-ID gate:
        // an ID-less root is retained with an empty `task_id` so a
        // closed ID-bearing subtask can inherit its anchor. Matching
        // still uses only nonempty explicit IDs, and no dependency ID
        // is ever minted here.
        let task_id = metadata.task_id.unwrap_or_default();
        let is_root = outcome.root.as_ref().is_some_and(|root| {
            root.absolute_path == *path && root.block_id == *id
        });
        predecessors.push(engine::CompletedTask {
            relative_path: path
                .strip_prefix(bob_dir)
                .map(Path::to_path_buf)
                .unwrap_or_else(|_| PathBuf::from(&staged_line)),
            block_id: id.clone(),
            task_id,
            text: task_complete_display_text(
                &staged_line,
                &settings.global_filter,
                Some(id),
            ),
            is_root,
        });
    }
    // Trigger (a) reads the staged post-close text of C's notes, including
    // project notes that the filtered dependents snapshot omits.
    let mut note_texts =
        staged_completed_note_texts(bob_dir, planner, &predecessors)?;
    let tasks_settings =
        crate::native::task_status_hooks::read_tasks_settings(bob_dir);
    // Trigger (c) preload: when C closed a sub-project's `^prj`, load
    // its parent N through the staged planner before the gate. The byte
    // prefilter keeps this (and the catalog index it needs, a full vault
    // walk) off every other close: one resolution plus one read of N,
    // and only for `^prj` closes with a `parent:`.
    let mut note_index = None;
    if engine::closed_prj_may_have_parent(&predecessors, &note_texts) {
        let index = staged_note_index(bob_dir, planner, dependency_ctx);
        let preload = engine::inspect_pull_ins(
            &predecessors,
            &note_texts,
            &tasks_settings,
            today,
            Some(&index),
        );
        for parent in preload.parent_notes {
            if note_texts.iter().any(|(path, _)| path == &parent) {
                continue;
            }
            let absolute = bob_dir.join(&parent);
            if let Some(contents) = planner.current_contents(&absolute)? {
                note_texts.push((parent, contents));
            }
        }
        note_index = Some(index);
    }
    let gate = engine::inspect_pull_ins(
        &predecessors,
        &note_texts,
        &tasks_settings,
        today,
        note_index.as_ref(),
    );
    // Day text before this gesture's strikes, moves, or retirement: the
    // already-planned baseline for anchors and derived ranks. The tree
    // close never rewrites Pomodoro bullets, so the post-tree staged
    // text is the right baseline.
    let pre_day_text = if day_exists {
        planner.current_contents(&day_file)?
    } else {
        None
    };
    let mut successor_plan = engine::SuccessorPlan::default();
    let mut unblocked_check = "checked";
    // Keep-moving G1-G3 pre-gate over held completed-note and baseline day
    // text: no I/O, so a non-leaf close adds no reads.
    let held_notes: Vec<(PathBuf, String)> = note_texts.clone();
    // Both switches gate the trigger itself, so a disabled group costs the
    // close nothing (the config is read only after G1-G3 pass).
    let keep_moving_candidate =
        crate::native::task_complete::keep_moving::keep_moving_pre_gate(
            &predecessors,
            &held_notes,
            pre_day_text.as_deref(),
            today,
            &tasks_settings,
        ) && dependency_ctx.keep_moving()
            && dependency_ctx.link_unblocked();
    // The snapshot proves prerequisites closed, so it is built only when a
    // candidate or `^prj` actually carries a `[dependsOn::]` (§12.15).
    let keep_moving_needs_proof = keep_moving_candidate
        && crate::native::task_complete::keep_moving::keep_moving_needs_dependents(
            &predecessors,
            &held_notes,
            &tasks_settings,
        );
    // Cheap G0–G3: nonempty labels, link_unblocked, a Done event from C,
    // and a target block ID. No catalog walk before this gate.
    let relation_candidate = !dependency_ctx.successor_labels().is_empty()
        && dependency_ctx.link_unblocked()
        && predecessors.iter().any(|task| !task.block_id.is_empty());
    if gate.has_candidates
        || gate.needs_dependents
        || keep_moving_candidate
        || relation_candidate
    {
        // Build the vault-wide snapshot only when legacy gates need it
        // or keep-moving dependency proof does. Otherwise only staged
        // project text is needed.
        let needs_snapshot = gate.needs_dependents || keep_moving_needs_proof;
        if relation_candidate {
            dependency_ctx.remember_relation_event_ids(
                predecessors
                    .iter()
                    .map(|task| task.block_id.as_str())
                    .filter(|id| !id.is_empty()),
            );
        }
        let snapshot = if needs_snapshot {
            let overlay = staged_overlay_map(bob_dir, planner);
            let snapshot_ready = dependency_ctx
                .dependents_snapshot()
                .snapshot_complete(&overlay);
            if !snapshot_ready {
                // An incomplete snapshot is non-fatal: the close still
                // succeeds with legacy recovery suppressed, but KM31's
                // independently safe keep-moving rows still offer.
                unblocked_check = "unavailable";
                Vec::new()
            } else {
                staged_snapshot_for_recovery(bob_dir, planner, dependency_ctx)
            }
        } else {
            // Trigger (a) with no dependency fields needs only the staged
            // project text, so do not build the vault-wide snapshot.
            Vec::new()
        };
        {
            if note_index.is_none() {
                note_index =
                    Some(staged_note_index(bob_dir, planner, dependency_ctx));
            }
            let index = note_index.as_ref().expect("index built above");
            let mut relation = None;
            if relation_candidate {
                let done_events =
                    engine::relation_events_for_planner(&predecessors, &[]);
                if let Some(read) =
                    super::relation_sources::collect_relation_sources(
                        dependency_ctx,
                        planner,
                        bob_dir,
                        &done_events,
                        &[],
                        &note_texts,
                        &tasks_settings,
                        index,
                    )?
                {
                    overlay_note_texts(&mut note_texts, read.source_notes);
                    overlay_note_texts(&mut note_texts, read.target_notes);
                    // Dependency proof already lives in `snapshot` on the
                    // needs_snapshot path; overlay only paths the snapshot
                    // does not already carry.
                    let snapshot_paths: BTreeSet<&PathBuf> =
                        snapshot.iter().map(|(path, _)| path).collect();
                    let missing_dependency_notes: Vec<(PathBuf, String)> = read
                        .dependency_notes
                        .into_iter()
                        .filter(|(path, _)| !snapshot_paths.contains(path))
                        .collect();
                    overlay_note_texts(
                        &mut note_texts,
                        missing_dependency_notes,
                    );
                    relation = Some(read.input);
                }
            }
            successor_plan = engine::plan_successors(engine::SuccessorInput {
                bob_dir: Some(bob_dir),
                completed: predecessors,
                snapshot,
                note_texts,
                pre_day: pre_day_text.clone(),
                post_day: None,
                closing: None,
                day_relative: PathBuf::from(day_relative.clone()),
                today,
                tasks_settings: &tasks_settings,
                note_settings: &settings,
                link_unblocked: dependency_ctx.link_unblocked(),
                keep_moving: dependency_ctx.keep_moving(),
                snapshot_complete: unblocked_check != "unavailable",
                relation,
                selection: request.selection.clone(),
                index,
            });
            // The engine enforces the unavailable-snapshot boundary itself:
            // legacy verdicts are suppressed before writes, while KM31
            // keep-moving rows survive. No caller-side clearing.
            for (relative_path, text) in &successor_plan.changed_files {
                let absolute_path = bob_dir.join(relative_path);
                planner.stage(&absolute_path, text.clone())?;
            }
            if let Some(updated_day) = successor_plan.new_day_text.clone() {
                planner.stage(&day_file, updated_day)?;
            }
        }
    }
    // Scoped ledger retirement over the post-successor staged day file,
    // so link statuses resolve the inserted successor links as Live.
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
        let final_day_text = if retirement.changed {
            retirement.text.clone()
        } else {
            day_text.clone()
        };
        pomodoro_refs.extend(restamp_changed_timed_entries(
            planner,
            bob_dir,
            &day_file,
            Path::new(&day_relative),
            pre_day.as_deref().unwrap_or(&day_text),
            final_day_text,
            now,
            warnings,
        )?);
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
    let _created = date_string(today);
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
    let clean_row_text = |text: &str, block_id: &str| {
        note_tasks::clean_description(
            text,
            &settings.global_filter,
            if block_id.is_empty() {
                None
            } else {
                Some(block_id)
            },
        )
    };
    let map_row = |dependent: &engine::UnblockedRow| {
        unblocked_row_json(
            dependent,
            bob_dir,
            &day_relative,
            &settings.global_filter,
        )
    };
    let unblocked = successor_plan
        .unblocked
        .iter()
        .map(&map_row)
        .collect::<Vec<_>>();
    let keep_moving = successor_plan
        .keep_moving
        .iter()
        .map(&map_row)
        .collect::<Vec<_>>();
    let relation_successors = successor_plan
        .relation_successors
        .iter()
        .map(&map_row)
        .collect::<Vec<_>>();
    let relation_held = successor_plan
        .relation_held
        .iter()
        .map(|held| {
            relation_held_row_json(held, bob_dir, &settings.global_filter)
        })
        .collect::<Vec<_>>();
    let relation_check = relation_check_json(
        successor_plan.relation_check,
        &relation_successors,
        &relation_held,
    );
    let still_blocked = successor_plan
        .still_blocked
        .iter()
        .map(|blocked| TaskCompleteStillBlockedJson {
            note_path: display_relative(&blocked.note_path),
            block_id: blocked.block_id.clone(),
            line: blocked.line,
            text: clean_row_text(&blocked.text, &blocked.block_id),
            status_symbol: blocked.status_symbol,
            status_name: status_name_for(bob_dir, blocked.status_symbol),
            reason: blocked.reason,
            waits_on: blocked.waits_on,
            scheduled: blocked.scheduled.clone(),
            project: blocked
                .project
                .as_ref()
                .map(|path| display_relative(path)),
            trigger: blocked.trigger,
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
    // The top-level day file is set whenever the gesture changed the
    // day file, so Open Note(s) can include the daily note (§12.5).
    // Keep-moving picks write the day too.
    let day_changed = ledger_json.is_some()
        || successor_plan
            .unblocked
            .iter()
            .any(|row| row.link.is_some())
        || successor_plan
            .keep_moving
            .iter()
            .any(|row| row.link.is_some())
        || successor_plan
            .relation_successors
            .iter()
            .any(|row| row.link.is_some());
    let top_day_file = day_changed.then(|| day_file.display().to_string());
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
        keep_moving: keep_moving.clone(),
        relation_successors: relation_successors.clone(),
        relation_held: relation_held.clone(),
        relation_check,
        relation_groups: successor_plan.relation_groups.clone(),
        still_blocked,
        unblocked_check,
        project_folds: successor_plan.project_folds.clone(),
        keep_moving_folds: successor_plan.keep_moving_folds.clone(),
    };
    // Task blocks: completed (root + closed subtasks) + unblocked +
    // keep-moving.
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
    for dependent in &successor_plan.unblocked {
        let absolute_path = bob_dir.join(&dependent.note_path);
        let rel = display_relative(&dependent.note_path);
        let mut stem = dependent.note_path.clone();
        stem.set_extension("");
        push_unblocked_ref(
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
    for dependent in &successor_plan.keep_moving {
        // Only selected keep-moving rows write task lines; unpicked rows
        // write nothing. The tracker still records the touch so the block
        // renders; the role preserves the keep-moving class.
        if dependent.link.is_none() {
            continue;
        }
        let absolute_path = bob_dir.join(&dependent.note_path);
        let rel = display_relative(&dependent.note_path);
        let mut stem = dependent.note_path.clone();
        stem.set_extension("");
        push_keep_moving_ref(
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
    for dependent in &successor_plan.relation_successors {
        if dependent.link.is_none() {
            continue;
        }
        let absolute_path = bob_dir.join(&dependent.note_path);
        let rel = display_relative(&dependent.note_path);
        let mut stem = dependent.note_path.clone();
        stem.set_extension("");
        push_relation_successor_ref(
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
    Ok(CompleteExistingTaskOutcome {
        summary,
        relative: relative.clone(),
        absolute: absolute.clone(),
        rel_display: rel_display.clone(),
        previous_task_line: previous_task_line.clone(),
        previous_status_symbol,
        previous_status_name: previous_status_name.clone(),
        task_line: task_line.clone(),
        final_status_symbol: final_task.status_symbol,
        final_status_name: final_task.status_name.clone(),
        top_day_file: top_day_file.clone(),
        pomodoro_refs: pomodoro_refs.clone(),
        task_block_refs: task_block_refs.clone(),
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

pub(super) fn reject_task_complete_conflicts(
    parsed: &ParsedCaptureText,
    request: &CaptureRequest,
    raw: &str,
) -> Result<(), CaptureError> {
    if !request.forced_destination_flags.is_empty()
        || request.forced_route.is_some()
        || request.forced_section.is_some()
        || request.forced_sub_bullet_target.is_some()
        || request.forced_drawer.is_some()
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

pub(super) fn status_name_for(bob_dir: &Path, symbol: char) -> String {
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

pub(super) fn push_unblocked_ref(
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

pub(super) fn push_keep_moving_ref(
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
        role: TaskBlockRole::KeepMoving,
    });
}

pub(super) fn push_relation_successor_ref(
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
        role: TaskBlockRole::RelationSuccessor,
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
    // One read and one task scan per distinct linked note: a busy day
    // links the same note dozens of times, and rescanning it per link
    // dominates close latency on large vaults.
    let mut scans: BTreeMap<PathBuf, Option<note_tasks::NoteTaskScan>> =
        BTreeMap::new();
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
        let scan = scans.entry(resolved.clone()).or_insert_with(|| {
            let contents = vault.read_latest(&resolved).ok().flatten();
            contents.map(|text| note_tasks::scan(&text, &settings))
        });
        let task = match scan.as_ref().and_then(|scan| {
            match scan.by_block_id(&reference.block_id) {
                BlockIdLookup::Found(task) => Some(task.status_symbol),
                _ => None,
            }
        }) {
            Some(symbol) => symbol,
            _ => continue,
        };
        if is_done_status(task, &settings) {
            // Done elsewhere and not yet reconciled: untouched.
            continue;
        }
        statuses.insert(
            reference.clone(),
            engine::LinkStatus::Live {
                path: resolved,
                status: task,
            },
        );
    }
    // Also cover staged-new notes the resolver cannot see yet.
    statuses
}

/// Snapshot of vault notes with staged overlays for dependent recovery:
/// the batch's prefiltered on-disk snapshot plus every staged file.
/// The on-disk base is built at most once per batch (lazily, only when a
/// `!` item passes the `[id::]` gate) and borrowed afterwards; staged
/// overlays are applied per item so recovery still sees earlier items'
/// staged edits. An unavailable base yields an empty snapshot: the close
/// still succeeds and nothing is recovered.
/// Note index for successor link forms (`docs/task-dependencies.md`
/// §12.4): the same walk-only catalog `&` dependency links use, unioned
/// with the batch's staged `.md` files so batch-created notes resolve.
pub(super) fn staged_note_index(
    bob_dir: &Path,
    planner: &CaptureBatchPlanner,
    dependency_ctx: &DependencyContext,
) -> crate::native::vault_links::NoteIndex {
    let mut staged: Vec<PathBuf> = Vec::new();
    for (absolute, _) in planner.staged_snapshot() {
        let Ok(relative) =
            absolute.strip_prefix(bob_dir).map(Path::to_path_buf)
        else {
            continue;
        };
        if relative
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("md"))
        {
            staged.push(relative);
        }
    }
    dependency_ctx.catalog().index.with_additional(staged)
}

/// Staged post-gesture texts for the notes containing C. Project pull-in
/// trigger (a) must inspect these even when the dependents catalog would
/// omit a project note with no `[id::]` / `[dependsOn::]` markers.
pub(super) fn staged_completed_note_texts(
    bob_dir: &Path,
    planner: &mut CaptureBatchPlanner,
    completed: &[engine::CompletedTask],
) -> Result<Vec<(PathBuf, String)>, CaptureError> {
    let paths: BTreeSet<PathBuf> = completed
        .iter()
        .map(|task| task.relative_path.clone())
        .collect();
    let mut texts = Vec::new();
    for relative in paths {
        let absolute = bob_dir.join(&relative);
        if let Some(contents) = planner.current_contents(&absolute)? {
            texts.push((relative, contents));
        }
    }
    Ok(texts)
}

/// Build the batch-level move map note-pull rows follow when a later
/// item relocates a pulled task. Drawn only from each item's
/// `project_move_landings` and `transfer_move_output.moves`; project-note
/// and transfer execution are never changed here. Chains are followed at
/// lookup time, so one task moving twice still resolves to its final home.
pub(super) fn build_note_pull_move_map(
    items: &[PlannedCaptureItem],
    project_moves: &[ProjectMoveLanding],
    bob_dir: &Path,
) -> BTreeMap<(PathBuf, String), (PathBuf, String)> {
    let mut map: BTreeMap<(PathBuf, String), (PathBuf, String)> =
        BTreeMap::new();
    for landing in project_moves {
        let (Some(old_id), Some(new_id)) =
            (landing.old_block_id.clone(), landing.new_block_id.clone())
        else {
            continue;
        };
        let source_rel = landing
            .source
            .strip_prefix(bob_dir)
            .unwrap_or(&landing.source)
            .to_path_buf();
        let dest_rel = landing
            .dest
            .strip_prefix(bob_dir)
            .unwrap_or(&landing.dest)
            .to_path_buf();
        map.insert((source_rel, old_id), (dest_rel, new_id));
    }
    for item in items {
        for movement in &item.result.transfer_move_output.moves {
            let (Some(old_id), Some(new_id)) =
                (movement.old_block_id.clone(), movement.new_block_id.clone())
            else {
                continue;
            };
            let source_rel = PathBuf::from(&movement.source_path);
            let dest_rel = PathBuf::from(&movement.destination_path);
            map.insert((source_rel, old_id), (dest_rel, new_id));
        }
    }
    map
}

/// Follow move chains to the final identity. ID-less rows (empty block
/// id) can never be move targets, so they resolve to themselves and keep
/// today's drop behavior.
pub(super) fn resolve_move_final(
    map: &BTreeMap<(PathBuf, String), (PathBuf, String)>,
    note_path: &str,
    block_id: &str,
) -> Option<(PathBuf, String)> {
    if block_id.is_empty() {
        return None;
    }
    let mut current = (PathBuf::from(note_path), block_id.to_string());
    let mut resolved = false;
    for _ in 0..(map.len().saturating_add(1)) {
        let Some(next) = map.get(&current) else {
            break;
        };
        current = next.clone();
        resolved = true;
    }
    resolved.then_some(current)
}

/// Remapped `[[stem#^id]]` link for a moved note-pull row, matching the
/// vault-wide link repair the move execution stages in the day text.
fn remapped_block_link(final_path: &Path, final_id: &str) -> String {
    let stem_path = final_path.with_extension("");
    let stem = display_relative(&stem_path);
    format!("[[{stem}#^{final_id}]]")
}

/// Net batch post-pass for successor rows (`docs/task-dependencies.md`
/// §12.5 SL20): after every item is planned, drop each linked row whose
/// bullet no longer survives live in the final staged day text, or whose
/// task is no longer open. Offer rows are also dropped when a later item
/// completes, re-blocks, later-links, or `x~K`-drops them, or when a
/// `project_finished` emptied-project predicate no longer holds. A
/// keep-moving group is also dropped whole when a later item links an
/// open resident of its project or closes its `^prj` (KM21). Runs on
/// staged text, so dry-run and real-run JSON stay equal.
pub(super) fn apply_successor_net_post_pass(
    items: &mut [PlannedCaptureItem],
    planner: &mut CaptureBatchPlanner,
    dependency_ctx: &DependencyContext,
    bob_dir: &Path,
    keep_moving_trace: &super::keep_moving_net::KeepMovingTrace,
    project_moves: &[ProjectMoveLanding],
) {
    // Traverse whole-item `task_complete`, nested `relation_completes`,
    // and `pomodoro_close` rows together so nested completions share the
    // final net filtering with `!` closes. Both `unblocked[]` and
    // `keep_moving[]` share the per-row net checks; keep-moving groups
    // additionally drop whole by later-item provenance (KM21).
    let any_linked = items.iter().any(|item| {
        super::output::all_offer_rows(&item.result)
            .any(|row| row.link.is_some())
    });
    let any_pull_in = items.iter().any(|item| {
        super::output::all_offer_rows(&item.result)
            .any(|row| row.trigger == "project_empty")
    });
    let any_offer = items.iter().any(|item| {
        super::output::all_offer_rows(&item.result)
            .any(|row| row.offer.is_some())
    });
    if !any_linked && !any_pull_in && !any_offer {
        return;
    }
    super::keep_moving_net::drop_invalidated_groups(
        items,
        keep_moving_trace,
        planner,
        dependency_ctx,
        bob_dir,
    );
    // SA21: later live-linking a parked T drops its parked group, and
    // later linking S drops S's row, sharing the keep-moving provenance
    // so own picks never invalidate their own groups. Runs before the
    // final-state row checks so surviving multi-trigger rows keep their
    // pruned trigger lists.
    super::keep_moving_net::drop_invalidated_relation_groups(
        items,
        keep_moving_trace,
        planner,
        dependency_ctx,
        bob_dir,
    );
    let day_file = pomodoro::day_file_for(bob_dir);
    let day_relative = capture_pomodoros::relative_day_file(&day_file, bob_dir);
    let live = if any_linked || any_offer {
        planner
            .peek_text(&day_file)
            .map(|final_day| {
                if !final_day.contains("#^") {
                    return BTreeSet::new();
                }
                let index = staged_note_index(bob_dir, planner, dependency_ctx);
                engine::live_link_keys(
                    &final_day,
                    Path::new(&day_relative),
                    &index,
                )
            })
            .unwrap_or_default()
    } else {
        BTreeSet::new()
    };
    let tasks_settings =
        crate::native::task_status_hooks::read_tasks_settings(bob_dir);
    // Batch-level move map for note-pull net identity: a later project-note
    // or transfer move relocates a pulled task, so rows follow to their
    // final home instead of dropping on the old note.
    let move_map = build_note_pull_move_map(items, project_moves, bob_dir);
    // Cross-group earliest-wins ownership for explicit offer keys: one row
    // per key per draft across hand-off, relation, and note-pull groups.
    // The earliest item that offers a key owns it; later duplicates are
    // omitted even when unselected. Later explicit links still win via the
    // live-link net checks below (they carry no offer key), so NP18-style
    // drops are preserved. Note-pull rows additionally dedup on final
    // move-aware identity so a later pull of the destination omits a row an
    // earlier pull still owns under a different key.
    {
        use std::collections::BTreeSet;
        let mut seen: BTreeSet<String> = BTreeSet::new();
        let mut seen_finals: BTreeSet<(PathBuf, String)> = BTreeSet::new();
        let track_final =
            |row: &TaskCompleteUnblockedJson,
             seen_finals: &mut BTreeSet<(PathBuf, String)>| {
                if row.block_id.is_empty() {
                    return;
                }
                let final_identity = resolve_move_final(
                    &move_map,
                    &row.note_path,
                    &row.block_id,
                )
                .unwrap_or_else(|| {
                    (PathBuf::from(&row.note_path), row.block_id.clone())
                });
                seen_finals.insert(final_identity);
            };
        for item in items.iter_mut() {
            for vec in super::output::unblocked_vecs_mut(&mut item.result) {
                vec.retain(|row| {
                    let Some(offer) = row.offer.as_ref() else {
                        return true;
                    };
                    !seen.contains(&offer.key)
                });
                for row in vec.iter() {
                    if let Some(offer) = row.offer.as_ref() {
                        seen.insert(offer.key.clone());
                    }
                    track_final(row, &mut seen_finals);
                }
            }
            for vec in
                super::output::relation_successors_vecs_mut(&mut item.result)
            {
                vec.retain(|row| {
                    let Some(offer) = row.offer.as_ref() else {
                        return true;
                    };
                    !seen.contains(&offer.key)
                });
                for row in vec.iter() {
                    if let Some(offer) = row.offer.as_ref() {
                        seen.insert(offer.key.clone());
                    }
                    track_final(row, &mut seen_finals);
                }
            }
            for vec in super::output::note_pull_vecs_mut(&mut item.result) {
                vec.retain(|row| {
                    let Some(offer) = row.offer.as_ref() else {
                        return true;
                    };
                    if seen.contains(&offer.key) {
                        return false;
                    }
                    if !move_map.is_empty() && !row.block_id.is_empty() {
                        let final_identity = resolve_move_final(
                            &move_map,
                            &row.note_path,
                            &row.block_id,
                        )
                        .unwrap_or_else(|| {
                            (
                                PathBuf::from(&row.note_path),
                                row.block_id.clone(),
                            )
                        });
                        if seen_finals.contains(&final_identity) {
                            return false;
                        }
                    }
                    true
                });
                for row in vec.iter() {
                    if let Some(offer) = row.offer.as_ref() {
                        seen.insert(offer.key.clone());
                    }
                    track_final(row, &mut seen_finals);
                }
            }
            let rows = &mut item.result.relation_merge_output.offer_rows;
            rows.retain(|row| {
                row.offer
                    .as_ref()
                    .is_none_or(|offer| !seen.contains(&offer.key))
            });
            seen.extend(rows.iter().filter_map(|row| {
                row.offer.as_ref().map(|offer| offer.key.clone())
            }));
        }
    }
    for item in items.iter_mut() {
        for vec in super::output::unblocked_vecs_mut(&mut item.result) {
            vec.retain(|row| {
                keep_unblocked_row(
                    row,
                    &live,
                    planner,
                    bob_dir,
                    &tasks_settings,
                )
            });
            for row in vec.iter_mut() {
                refresh_project_offer_final_position(
                    row,
                    planner,
                    bob_dir,
                    &tasks_settings,
                );
            }
        }
        for vec in super::output::keep_moving_vecs_mut(&mut item.result) {
            vec.retain(|row| {
                keep_unblocked_row(
                    row,
                    &live,
                    planner,
                    bob_dir,
                    &tasks_settings,
                )
            });
        }
        for vec in super::output::relation_successors_vecs_mut(&mut item.result)
        {
            vec.retain(|row| {
                keep_unblocked_row(
                    row,
                    &live,
                    planner,
                    bob_dir,
                    &tasks_settings,
                )
            });
            for row in vec.iter_mut() {
                refresh_relation_row_final_position(
                    row,
                    planner,
                    bob_dir,
                    &tasks_settings,
                );
            }
        }
        // Note-pull per-row net filtering (SL20): a later item that
        // links, completes, re-blocks, or drops an offered task removes
        // that row. Own picks survive via the linked-row survive check.
        // No keep-moving whole-group invalidation applies here because a
        // note pull is a request, not a trigger. Rows a later move
        // relocated follow to their final home before the checks, keeping
        // the stable offer key; ID-less rows keep today's drop behavior.
        for vec in super::output::note_pull_vecs_mut(&mut item.result) {
            for row in vec.iter_mut() {
                let Some((final_path, final_id)) = resolve_move_final(
                    &move_map,
                    &row.note_path,
                    &row.block_id,
                ) else {
                    continue;
                };
                let final_path_str = display_relative(&final_path);
                if row.note_path != final_path_str || row.block_id != final_id {
                    row.note_path = final_path_str;
                    row.block_id = final_id.clone();
                    if let Some(link) = row.link.as_mut() {
                        link.block_link =
                            remapped_block_link(&final_path, &final_id);
                    }
                }
            }
            vec.retain(|row| {
                keep_unblocked_row(
                    row,
                    &live,
                    planner,
                    bob_dir,
                    &tasks_settings,
                )
            });
            for row in vec.iter_mut() {
                refresh_note_pull_row_final_position(
                    row,
                    planner,
                    bob_dir,
                    &tasks_settings,
                );
            }
        }
    }
    for item in items.iter_mut() {
        let output = &mut item.result.relation_merge_output;
        output.offer_rows.retain(|row| {
            keep_unblocked_row(row, &live, planner, bob_dir, &tasks_settings)
        });
        for row in &mut output.offer_rows {
            refresh_relation_row_final_position(
                row,
                planner,
                bob_dir,
                &tasks_settings,
            );
        }
        let surviving = output
            .offer_rows
            .iter()
            .filter_map(|row| row.offer.as_ref().map(|offer| offer.key.clone()))
            .collect::<BTreeSet<_>>();
        for record in &mut output.merges {
            record.offers.retain(|offer| surviving.contains(&offer.key));
        }
    }
    // A group with no surviving selectable row is silent, folds included.
    super::keep_moving_net::prune_empty_folds(items);
    // Relation groups with no surviving row are silent too; the
    // presentation phase must not render an empty parked group.
    prune_empty_relation_groups(items);
    apply_offer_close_post_pass(items, planner, dependency_ctx, bob_dir);
}

/// Attach close descriptors after batch successor rows survive net filtering.
/// This reads one shared dependency snapshot and simulates one recover-only
/// close for each surviving offer row against the final staged note text.
fn apply_offer_close_post_pass(
    items: &mut [PlannedCaptureItem],
    planner: &mut CaptureBatchPlanner,
    dependency_ctx: &DependencyContext,
    bob_dir: &Path,
) {
    let tasks_settings =
        crate::native::task_status_hooks::read_tasks_settings(bob_dir);
    let note_settings = dependency_ctx.settings();
    let staged = staged_overlay_map(bob_dir, planner);
    let dependencies = dependency_ctx.dependents_snapshot();
    let snapshot_complete = dependencies.snapshot_complete(&staged);
    let recovery_snapshot = dependencies.overlaid(staged);
    let index = staged_note_index(bob_dir, planner, dependency_ctx);
    let day_file = pomodoro::day_file_for(bob_dir);
    let day_relative = capture_pomodoros::relative_day_file(&day_file, bob_dir);
    let created_tasks: Vec<(String, Option<String>, String)> = items
        .iter()
        .filter(|item| item.result.kind == "task")
        .map(|item| {
            (
                item.result.relative_target.clone(),
                item.result.block_id.clone(),
                item.result.task_line.clone(),
            )
        })
        .collect();

    for item in items {
        super::output::for_each_offer_row_mut(&mut item.result, |row| {
            let Some(offer) = row.offer.as_ref() else {
                return;
            };
            let key = offer.key.clone();
            let group = offer.group;
            let prior_refusal =
                offer.close.as_ref().and_then(|close| close.refusal);
            let relative = PathBuf::from(&row.note_path);
            let absolute = bob_dir.join(&relative);
            let Some(source_text) =
                planner.peek_text(&absolute).or_else(|| {
                    recovery_snapshot
                        .iter()
                        .find(|(path, _)| path == &relative)
                        .map(|(_, text)| text.clone())
                })
            else {
                row.offer.as_mut().expect("offer still present").close =
                    Some(engine::offer_close::OfferCloseDescriptor::refused(
                        engine::offer_close::OfferCloseRefusal::ChangedIdentity,
                        engine::offer_close::why_changed_identity(),
                    ));
                return;
            };
            let Some(task) =
                successor_task_lookup(row, planner, bob_dir, &tasks_settings)
            else {
                row.offer.as_mut().expect("offer still present").close =
                    Some(engine::offer_close::OfferCloseDescriptor::refused(
                        engine::offer_close::OfferCloseRefusal::ChangedIdentity,
                        engine::offer_close::why_changed_identity(),
                    ));
                return;
            };
            let expected_line =
                crate::native::task_complete::successors::raw_task_line(
                    &source_text,
                    task.line_index,
                );
            let is_new_task = prior_refusal
                == Some(engine::offer_close::OfferCloseRefusal::NewTask)
                || capture_created_offer_task(
                    row,
                    &task,
                    &expected_line,
                    &created_tasks,
                    &tasks_settings,
                );
            let snapshot = snapshot_with_source(
                &recovery_snapshot,
                &relative,
                &source_text,
            );
            let descriptor = engine::offer_close::describe_offer_close(
                &engine::offer_close::OfferCloseInput {
                    key: &key,
                    path: &relative,
                    line_index: task.line_index,
                    expected_line: &expected_line,
                    source_text: &source_text,
                    group,
                    is_new_task,
                    muted: prior_refusal
                        == Some(engine::offer_close::OfferCloseRefusal::Muted),
                    today: dependency_ctx.today,
                    tasks_settings: &tasks_settings,
                    note_settings,
                    snapshot: &snapshot,
                    snapshot_complete,
                    day_relative: Path::new(&day_relative),
                    index: &index,
                },
            );
            row.offer.as_mut().expect("offer still present").close =
                Some(descriptor);
        });
    }
}

fn snapshot_with_source(
    snapshot: &[(PathBuf, String)],
    path: &Path,
    source_text: &str,
) -> Vec<(PathBuf, String)> {
    let mut snapshot = snapshot.to_vec();
    if let Some((_, contents)) =
        snapshot.iter_mut().find(|(candidate, _)| candidate == path)
    {
        *contents = source_text.to_string();
    } else {
        snapshot.push((path.to_path_buf(), source_text.to_string()));
        snapshot.sort_by(|left, right| left.0.cmp(&right.0));
    }
    snapshot
}

fn capture_created_offer_task(
    row: &TaskCompleteUnblockedJson,
    task: &crate::native::task_status_hooks::TaskLine,
    raw_line: &str,
    created_tasks: &[(String, Option<String>, String)],
    tasks_settings: &crate::native::task_status_hooks::TasksSettings,
) -> bool {
    created_tasks.iter().any(|(path, block_id, task_line)| {
        if path != &row.note_path {
            return false;
        }
        block_id
            .as_deref()
            .zip(task.block_id.as_deref())
            .is_some_and(|(created, current)| created == current)
            || raw_line.trim_end_matches('\r')
                == task_line.trim_end_matches('\r')
            || crate::native::task_status_hooks::parse_tasks(
                task_line,
                tasks_settings,
            )
            .first()
            .is_some_and(|created| {
                task.block_id.is_none()
                    && created.block_id.is_none()
                    && created.description == task.description
            })
    })
}

fn refresh_project_offer_final_position(
    row: &mut TaskCompleteUnblockedJson,
    planner: &mut CaptureBatchPlanner,
    bob_dir: &Path,
    tasks_settings: &crate::native::task_status_hooks::TasksSettings,
) {
    if row.offer.is_none()
        || row.trigger != crate::native::task_complete::PROJECT_CREATED_TRIGGER
    {
        return;
    }
    let Some(task) =
        successor_task_lookup(&*row, planner, bob_dir, tasks_settings)
    else {
        return;
    };
    row.line = task.line_index + 1;
    if row.status_symbol != task.status {
        row.status_symbol = task.status;
        row.status_name = status_name_for(bob_dir, task.status);
    }
}

/// Refresh a surviving relation row's line (and status spelling) from
/// the final staged text, so later items that insert context or mint
/// IDs do not leave a stale line behind. ID-less relation sources are
/// rare (R1–R7 need a target block ID, not a source ID), but the lookup
/// falls back to line identity like other rows when no ID exists.
fn refresh_relation_row_final_position(
    row: &mut TaskCompleteUnblockedJson,
    planner: &mut CaptureBatchPlanner,
    bob_dir: &Path,
    tasks_settings: &crate::native::task_status_hooks::TasksSettings,
) {
    if row.offer.is_none() {
        return;
    }
    let Some(task) =
        successor_task_lookup(&*row, planner, bob_dir, tasks_settings)
    else {
        return;
    };
    row.line = task.line_index + 1;
    if row.status_symbol != task.status {
        row.status_symbol = task.status;
        row.status_name = status_name_for(bob_dir, task.status);
    }
}

/// Refresh a surviving note-pull row's line (and status spelling) from
/// the final staged text, including ID-less unselected siblings whose
/// lines shift when later items insert context or mint IDs.
fn refresh_note_pull_row_final_position(
    row: &mut TaskCompleteUnblockedJson,
    planner: &mut CaptureBatchPlanner,
    bob_dir: &Path,
    tasks_settings: &crate::native::task_status_hooks::TasksSettings,
) {
    if row.offer.is_none() {
        return;
    }
    if row.trigger != crate::native::task_complete::NOTE_PULL_TRIGGER
        && row
            .offer
            .as_ref()
            .is_none_or(|offer| offer.group != "note_pull")
    {
        return;
    }
    let Some(task) =
        successor_task_lookup(&*row, planner, bob_dir, tasks_settings)
    else {
        return;
    };
    row.line = task.line_index + 1;
    if row.status_symbol != task.status {
        row.status_symbol = task.status;
        row.status_name = status_name_for(bob_dir, task.status);
    }
}

/// Drop relation groups with no surviving selectable row. A group is
/// referenced when some surviving row carries its target in the row's
/// trigger list with a matching label key and parked/done event; an
/// empty trigger list never references a group.
fn prune_empty_relation_groups(items: &mut [PlannedCaptureItem]) {
    for item in items.iter_mut() {
        let referenced: std::collections::BTreeSet<(
            PathBuf,
            String,
            String,
            String,
        )> = super::output::all_relation_successors(&item.result)
            .filter(|row| row.offer.is_some())
            .filter_map(|row| {
                let relation = row.relation.as_ref()?;
                let mut keys = Vec::new();
                for target in &relation.after {
                    keys.push((
                        PathBuf::from(&target.note_path),
                        target.block_id.clone(),
                        relation.key.clone(),
                        relation.event.to_string(),
                    ));
                }
                Some(keys)
            })
            .flatten()
            .collect();
        for vec in super::output::relation_groups_mut(&mut item.result) {
            vec.retain(|group| {
                referenced.contains(&(
                    group.target.note_path.clone(),
                    group.target.block_id.clone(),
                    group.label_key.clone(),
                    group.event.as_str().to_string(),
                ))
            });
        }
    }
}

fn keep_unblocked_row(
    row: &TaskCompleteUnblockedJson,
    live: &BTreeSet<(PathBuf, String)>,
    planner: &mut CaptureBatchPlanner,
    bob_dir: &Path,
    tasks_settings: &crate::native::task_status_hooks::TasksSettings,
) -> bool {
    if row.offer.is_some() {
        if !successor_task_is_open(row, planner, bob_dir, tasks_settings) {
            return false;
        }
        let in_live = live
            .contains(&(PathBuf::from(&row.note_path), row.block_id.clone()));
        if row.link.is_some()
            || (row.trigger == "merge"
                && row.offer.as_ref().is_some_and(|offer| offer.selected))
        {
            if !successor_row_survives(
                row,
                live,
                planner,
                bob_dir,
                tasks_settings,
            ) {
                return false;
            }
        } else if in_live {
            return false;
        }
        if matches!(row.status_symbol, ' ' | '*' | '/')
            && successor_task_status(row, planner, bob_dir, tasks_settings)
                == Some('?')
        {
            return false;
        }
        if row
            .offer
            .as_ref()
            .is_some_and(|offer| offer.group == "project_finished")
            && project_has_open_work(row, planner, bob_dir, tasks_settings)
        {
            return false;
        }
        return true;
    }
    if row.trigger == "project_empty" {
        if row.link.is_some() {
            successor_row_survives(row, live, planner, bob_dir, tasks_settings)
        } else {
            successor_task_is_open(row, planner, bob_dir, tasks_settings)
        }
    } else {
        row.link.is_none()
            || successor_row_survives(
                row,
                live,
                planner,
                bob_dir,
                tasks_settings,
            )
    }
}

/// One linked row's net check: its bullet still lives unstruck under an
/// open entry of the final staged day text, and its task is still open.
fn successor_row_survives(
    row: &TaskCompleteUnblockedJson,
    live: &BTreeSet<(PathBuf, String)>,
    planner: &mut CaptureBatchPlanner,
    bob_dir: &Path,
    tasks_settings: &crate::native::task_status_hooks::TasksSettings,
) -> bool {
    if !live.contains(&(PathBuf::from(&row.note_path), row.block_id.clone())) {
        return false;
    }
    successor_task_is_open(row, planner, bob_dir, tasks_settings)
}

/// Whether a reported pull-in task still exists open after every draft
/// item has been planned. Surfaced rows have no link but must also be
/// dropped when a later item closes their `^prj` task (SL20).
fn successor_task_is_open(
    row: &TaskCompleteUnblockedJson,
    planner: &mut CaptureBatchPlanner,
    bob_dir: &Path,
    tasks_settings: &crate::native::task_status_hooks::TasksSettings,
) -> bool {
    successor_task_lookup(row, planner, bob_dir, tasks_settings).is_some_and(
        |task| task.status_recognized && task.status_type.is_open(),
    )
}

fn successor_task_status(
    row: &TaskCompleteUnblockedJson,
    planner: &mut CaptureBatchPlanner,
    bob_dir: &Path,
    tasks_settings: &crate::native::task_status_hooks::TasksSettings,
) -> Option<char> {
    successor_task_lookup(row, planner, bob_dir, tasks_settings)
        .map(|task| task.status)
}

fn successor_task_lookup(
    row: &TaskCompleteUnblockedJson,
    planner: &mut CaptureBatchPlanner,
    bob_dir: &Path,
    tasks_settings: &crate::native::task_status_hooks::TasksSettings,
) -> Option<crate::native::task_status_hooks::TaskLine> {
    let absolute = bob_dir.join(&row.note_path);
    let contents = planner.current_contents(&absolute).ok().flatten()?;
    let tasks = crate::native::task_status_hooks::parse_tasks(
        &contents,
        tasks_settings,
    );
    if !row.block_id.is_empty() {
        return tasks.into_iter().find(|task| {
            task.block_id.as_deref() == Some(row.block_id.as_str())
        });
    }
    if row.trigger == crate::native::task_complete::PROJECT_CREATED_TRIGGER
        || row.trigger == crate::native::task_complete::NOTE_PULL_TRIGGER
        || row.offer.as_ref().is_some_and(|offer| {
            offer.group == "keep_moving" || offer.group == "note_pull"
        })
    {
        // Project-created, keep-moving, and note-pull ID-less rows have
        // no stable block identity, so resolve by cleaned description plus
        // their duplicate ordinal among the tasks that are still ID-less.
        // The planner records that settled ordinal after the picks have
        // minted their IDs; the ordinal inside the public key
        // (`path@digest` or `path@digest~N`) counts the pre-pick render
        // and is only the fallback. Line numbers shift when later items
        // insert context, and raw `row.text` keeps inline fields like
        // `[type:: ...]`, so compare cleaned text on both sides.
        let settings = note_tasks::read_settings(bob_dir);
        let expected = note_tasks::clean_description(
            &row.text,
            &settings.global_filter,
            None,
        );
        let ordinal = row
            .offer
            .as_ref()
            .map(|offer| {
                offer
                    .settled_ordinal
                    .unwrap_or_else(|| offer_key_duplicate_ordinal(&offer.key))
            })
            .unwrap_or(1);
        let mut matches = tasks.iter().filter(|task| {
            task.block_id.is_none()
                && note_tasks::clean_description(
                    &task.description,
                    &settings.global_filter,
                    None,
                ) == expected
        });
        if let Some(task) = matches.nth(ordinal.saturating_sub(1)) {
            return Some(task.clone());
        }
        return None;
    }
    tasks
        .iter()
        .find(|task| task.line_index + 1 == row.line)
        .cloned()
}

fn offer_key_duplicate_ordinal(key: &str) -> usize {
    key.rsplit('~')
        .next()
        .and_then(|suffix| suffix.parse::<usize>().ok())
        .filter(|_| key.contains('~'))
        .unwrap_or(1)
        .max(1)
}

fn project_has_open_work(
    row: &TaskCompleteUnblockedJson,
    planner: &mut CaptureBatchPlanner,
    bob_dir: &Path,
    tasks_settings: &crate::native::task_status_hooks::TasksSettings,
) -> bool {
    let absolute = bob_dir.join(&row.note_path);
    let Ok(Some(contents)) = planner.current_contents(&absolute) else {
        return false;
    };
    crate::native::task_status_hooks::parse_tasks(&contents, tasks_settings)
        .iter()
        .any(|task| {
            task.status_recognized
                && task.status_type.is_open()
                && task.block_id.as_deref() != Some("prj")
                && task.block_id.as_deref() != Some(row.block_id.as_str())
                && !crate::native::capture_dependency_tasks::has_hide_tag(
                    contents
                        .lines()
                        .nth(task.line_index)
                        .unwrap_or_default()
                        .trim_end_matches('\r'),
                )
        })
}

/// Block links of every linked row left after the net post-pass, with
/// the surviving link's class: keep-moving picks report
/// `reason: "keep_moving"` and dependency picks keep `"unblocked"`.
/// Includes nested `relation_completes` rows so relation closes share
/// the surviving-link set with `!` closes.
pub(super) fn surviving_successor_link_reasons(
    items: &[PlannedCaptureItem],
) -> BTreeMap<String, &'static str> {
    let mut links = BTreeMap::new();
    for item in items {
        for row in super::output::all_offer_rows(&item.result) {
            if let Some(link) = row.link.as_ref() {
                let reason = if row.trigger == "last_planned"
                    || row
                        .offer
                        .as_ref()
                        .is_some_and(|offer| offer.group == "keep_moving")
                {
                    "keep_moving"
                } else if row.trigger == "relation"
                    || row
                        .offer
                        .as_ref()
                        .is_some_and(|offer| offer.group == "relation")
                {
                    "relation"
                } else if row.trigger
                    == crate::native::task_complete::NOTE_PULL_TRIGGER
                    || row
                        .offer
                        .as_ref()
                        .is_some_and(|offer| offer.group == "note_pull")
                {
                    "note_pull"
                } else {
                    "unblocked"
                };
                links.insert(link.block_link.clone(), reason);
            }
        }
    }
    links
}

pub(super) fn overlay_note_texts(
    dest: &mut Vec<(PathBuf, String)>,
    extra: Vec<(PathBuf, String)>,
) {
    for (path, contents) in extra {
        if let Some(existing) =
            dest.iter_mut().find(|(known, _)| known == &path)
        {
            existing.1 = contents;
        } else {
            dest.push((path, contents));
        }
    }
}

/// The batch's staged `.md` overlay keyed by vault-relative path
/// (`None` for a staged deletion): what the prefiltered snapshot is
/// overlaid with, and what snapshot completeness is judged against. A
/// staged entry supersedes its disk image, so a failed disk read covered
/// here never reports the snapshot as unavailable.
pub(super) fn staged_overlay_map(
    bob_dir: &Path,
    planner: &CaptureBatchPlanner,
) -> BTreeMap<PathBuf, Option<String>> {
    let mut staged: BTreeMap<PathBuf, Option<String>> = BTreeMap::new();
    for (absolute, contents) in planner.staged_snapshot() {
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
    staged
}

pub(super) fn staged_snapshot_for_recovery(
    bob_dir: &Path,
    planner: &CaptureBatchPlanner,
    dependency_ctx: &DependencyContext,
) -> Vec<(PathBuf, String)> {
    let snapshot = dependency_ctx.dependents_snapshot();
    let staged = staged_overlay_map(bob_dir, planner);
    if !snapshot.snapshot_complete(&staged) {
        return Vec::new();
    }
    snapshot.overlaid(staged)
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
