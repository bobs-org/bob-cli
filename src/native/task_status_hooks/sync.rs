//! Daily-note selection and task-status sync orchestration.
use super::*;

pub(crate) fn daily_anchor_date(
    daily_path: &Path,
    effective_date: NaiveDate,
) -> NaiveDate {
    daily_path
        .file_name()
        .and_then(OsStr::to_str)
        .and_then(native_pomodoro::parse_day_file_date)
        .unwrap_or(effective_date)
}

pub(super) fn canonical_daily_date(relative_path: &Path) -> Option<NaiveDate> {
    let mut components = relative_path.components();
    let Component::Normal(year_component) = components.next()? else {
        return None;
    };
    let Component::Normal(file_component) = components.next()? else {
        return None;
    };
    if components.next().is_some() {
        return None;
    }
    let year_text = year_component.to_str()?;
    let file_name = file_component.to_str()?;
    if year_text.len() != 4
        || !year_text.bytes().all(|byte| byte.is_ascii_digit())
        || file_name.len() != 11
        || !file_name.ends_with(".md")
    {
        return None;
    }
    let date = native_pomodoro::parse_day_file_date(file_name)?;
    (year_text.parse::<i32>().ok()? == date.year()).then_some(date)
}

pub(super) fn previous_daily_path(
    vault: &Path,
    markdown_files: &[PathBuf],
    anchor: NaiveDate,
) -> Option<PathBuf> {
    markdown_files
        .iter()
        .filter_map(|path| {
            let relative_path = path.strip_prefix(vault).ok()?;
            let date = canonical_daily_date(relative_path)?;
            (date < anchor).then_some((date, path))
        })
        .max_by_key(|(date, _)| *date)
        .map(|(_, path)| path.clone())
}

/// The multiple-open-timed guard names each conflicting entry so the
/// 15-minute failure is actionable: name (or `(unnamed)`), 1-based line,
/// and time range, plus how to leave exactly one entry open.
pub(super) fn multiple_timed_error(
    daily_contents: &str,
    model: &PomodoroModel,
) -> SyncError {
    const PREFIX: &str = "Bob daily note has multiple open timed Pomodoros";
    let scan = capture_pomodoros::scan(daily_contents);
    let mut details = Vec::new();
    for entry in &model.entries {
        if !(entry.open && entry.timed && entry.has_child) {
            continue;
        }
        let line = entry.line_index + 1;
        let scanned = scan.entries.iter().find(|scanned| scanned.line == line);
        let (mut name, mut range) = scanned
            .map(|scanned| (scanned.name.clone(), scanned.time_range.clone()))
            .unwrap_or((None, None));
        // The capture scan only recognizes leading ranges, while this
        // guard accepts a range anywhere in the entry. Fall back to the
        // ledger parse so legacy `First (0900-0930)` lines still name
        // their entry and range.
        if name.is_none() || range.is_none() {
            let (fallback_name, fallback_range) =
                native_entry_name_and_range(&entry.context);
            if name.is_none() {
                name = fallback_name;
            }
            if range.is_none() {
                range = fallback_range;
            }
        }
        let name =
            name.unwrap_or_else(|| plan_budget::UNNAMED_THEME.to_string());
        details.push(match range {
            Some(range) => format!("{name} (line {line}, {range})"),
            None => format!("{name} (line {line})"),
        });
    }
    if details.is_empty() {
        return SyncError::new(PREFIX);
    }
    SyncError::new(format!(
        "{PREFIX}: {}; close all but one with `bob capture -- =x`, or mark it `[x]`",
        details.join(", ")
    ))
}

/// Entry name and time range from the hooks' own ledger parse: the
/// task text with its time range removed, minus a leading `()` placeholder
/// and `—` name marker. Returns `(None, range)` when only a range parses,
/// so the caller prints `(unnamed)` with the range.
fn native_entry_name_and_range(
    context: &str,
) -> (Option<String>, Option<String>) {
    let Some(task) = native_pomodoro::open_ledger_task(context) else {
        return (None, None);
    };
    let Some((raw_range, start, end)) = native_pomodoro::task_time_range(task)
    else {
        return (None, None);
    };
    let mut candidate = task.replacen(raw_range, "", 1).trim().to_string();
    if let Some(rest) = candidate.strip_prefix("()") {
        candidate = rest.trim().to_string();
    }
    if let Some(rest) = candidate.strip_prefix('—') {
        candidate = rest.trim().to_string();
    }
    let name = (!candidate.is_empty()).then_some(candidate);
    (name, Some(format!("{start}-{end}")))
}

/// The read-only plan budget for a successful sync: the pure ledger
/// half plus the NOW count. It never changes the exit code and never
/// writes. An invalid plan config yields `None` (JSON `null`) and a
/// single stderr warning; a missing Pomodoros section also yields
/// `None`.
pub(super) fn plan_budget_for_sync(
    bob_dir: &Path,
    daily_contents: &str,
    anchor: NaiveDate,
    daily_path: &Path,
) -> Option<plan_budget::PlanReport> {
    let config = match bob_config::load_plan_config(&bob_config::config_path())
    {
        Ok(config) => config,
        Err(error) => {
            let message = match error {
                bob_config::ConfigError::Read(message)
                | bob_config::ConfigError::Invalid(message) => message,
            };
            eprintln!(
                    "{COMMAND_NAME}: warning: invalid plan config: {message}; plan_budget is null"
                );
            return None;
        }
    };
    let daily_file = daily_path
        .strip_prefix(bob_dir)
        .map(|path| path.display().to_string())
        .unwrap_or_else(|_| daily_path.display().to_string());
    let ledger = plan_budget::compute_for_daily(
        daily_contents,
        &config,
        Some(&daily_file),
    );
    if !ledger.has_section {
        return None;
    }
    let now = plan_budget::count_now(bob_dir, anchor, &config);
    Some(plan_budget::assemble_report(
        anchor,
        &daily_file,
        &config,
        &ledger,
        now,
    ))
}

pub(super) fn note_kind(contents: &str) -> NoteKind {
    let Some(frontmatter) = projects::parse_frontmatter(contents) else {
        return NoteKind::Other;
    };
    if projects::frontmatter_is_area(&frontmatter) {
        NoteKind::Area
    } else if projects::frontmatter_is_project(&frontmatter) {
        NoteKind::Project
    } else {
        NoteKind::Other
    }
}

pub(super) fn sync_task_statuses(
    request: &Request,
) -> Result<SyncResult, SyncError> {
    let _lock = if request.dry_run {
        None
    } else {
        Some(acquire_maintenance_lock().map_err(SyncError::from_apply)?)
    };

    let daily_path = native_pomodoro::day_file_for(&request.bob_dir);
    let daily_snapshot = match capture_required(&daily_path, InputKind::Daily) {
        Ok(snapshot) => snapshot,
        Err(CaptureError::NotFound(_)) => {
            return Err(SyncError::new(format!(
                "daily note does not exist: {}",
                daily_path.display()
            )));
        }
        Err(error) => {
            return Err(capture_to_sync("read daily note", error));
        }
    };
    let daily_contents = required_utf8(&daily_snapshot)
        .map_err(|error| capture_to_sync("read daily note", error))?;
    let mut inputs = vec![daily_snapshot.clone()];
    let daily_lines = logical_lines(&daily_contents);
    let section = native_pomodoro::pomodoros_section_range(&daily_lines)
        .ok_or_else(|| {
            SyncError::new(format!(
                "daily note has no Pomodoros section: {}",
                daily_path.display()
            ))
        })?;
    let pomodoro_model = scan_pomodoros(&daily_lines, section.clone());
    let timed_open = pomodoro_model
        .entries
        .iter()
        .filter(|entry| entry.open && entry.timed && entry.has_child)
        .count();
    if timed_open > 1 {
        return Err(multiple_timed_error(&daily_contents, &pomodoro_model));
    }

    let settings_path = request.bob_dir.join(TASKS_SETTINGS);
    let settings_snapshot =
        capture_optional(&settings_path, InputKind::TasksSettings)
            .map_err(|error| capture_to_sync("read Tasks settings", error))?;
    let settings = parse_tasks_settings(
        &settings_path,
        settings_snapshot
            .utf8_contents()
            .map_err(|error| capture_to_sync("read Tasks settings", error))?
            .as_deref(),
    );
    push_unique_input(&mut inputs, settings_snapshot);
    let markdown_files = markdown_files(&request.bob_dir).map_err(|error| {
        SyncError::io("scan vault", &request.bob_dir, error)
    })?;
    let scan_paths = markdown_files.clone();
    let anchor =
        daily_anchor_date(&daily_path, bob_env::current_datetime().date());
    let previous_daily_path =
        previous_daily_path(&request.bob_dir, &markdown_files, anchor);
    let canonical_daily_path = daily_path.canonicalize().ok();
    let mut files = Vec::with_capacity(markdown_files.len());
    for path in markdown_files {
        let same_as_daily = path == daily_path
            || canonical_daily_path.as_ref().is_some_and(|daily| {
                path.canonicalize().ok().as_ref() == Some(daily)
            });
        let contents = if same_as_daily {
            daily_contents.clone()
        } else {
            let snapshot = capture_required(&path, InputKind::Note)
                .map_err(|error| capture_to_sync("read note", error))?;
            let contents = required_utf8(&snapshot)
                .map_err(|error| capture_to_sync("read note", error))?;
            push_unique_input(&mut inputs, snapshot);
            contents
        };
        let relative_path = path
            .strip_prefix(&request.bob_dir)
            .map(Path::to_path_buf)
            .map_err(|_| {
                SyncError::new(format!(
                    "note is not under the vault root: {}",
                    path.display()
                ))
            })?;
        let tasks = parse_tasks(&contents, &settings);
        let note_kind = note_kind(&contents);
        files.push(FileScan {
            path,
            relative_path,
            contents,
            tasks,
            note_kind,
        });
    }

    let original_note_index = NoteIndex::from_paths(
        files.iter().map(|file| file.relative_path.clone()),
    );
    let original_task_blocks = task_blocks(&files);
    let daily_relative = daily_path.strip_prefix(&request.bob_dir).ok();
    let previous_daily_relative =
        previous_daily_path.as_ref().and_then(|path| {
            path.strip_prefix(&request.bob_dir)
                .ok()
                .map(Path::to_path_buf)
        });
    let previous_daily_references = previous_daily_path
        .as_ref()
        .and_then(|path| files.iter().find(|file| &file.path == path))
        .and_then(|file| {
            let lines = logical_lines(&file.contents);
            let section = native_pomodoro::pomodoros_section_range(&lines)?;
            Some(scan_pomodoros(&lines, section).recent_references)
        })
        .unwrap_or_default();
    if let Some(path) = &previous_daily_path {
        if let Some(existing) =
            inputs.iter_mut().find(|input| input.path == *path)
        {
            existing.kind = InputKind::PreviousDaily;
        } else {
            let snapshot = capture_required(path, InputKind::PreviousDaily)
                .map_err(|error| {
                    capture_to_sync("read previous daily note", error)
                })?;
            push_unique_input(&mut inputs, snapshot);
        }
    }
    let archive_catalog = archive_reference_catalog(
        &request.bob_dir,
        &settings,
        pomodoro_model
            .all_references
            .iter()
            .chain(previous_daily_references.iter()),
        &mut inputs,
    )?;
    let original_reference_resolver = TaskReferenceResolver {
        note_index: &original_note_index,
        task_blocks: &original_task_blocks,
        archive_catalog: &archive_catalog,
    };
    let mut structural_unresolved = Vec::new();
    let mut original_resolved_references = BTreeMap::new();
    for reference in &pomodoro_model.all_references {
        let Some(resolved) = resolve_task_reference(
            reference,
            daily_relative,
            &original_reference_resolver,
            ReferenceContext::CurrentDaily,
            &settings,
            &mut structural_unresolved,
        ) else {
            continue;
        };
        original_resolved_references.insert(reference.clone(), resolved);
    }

    let removed_duplicate_lines = plan_duplicate_line_removals(
        &daily_lines,
        &pomodoro_model,
        &original_resolved_references,
    );
    let deleted_lines = removed_duplicate_lines
        .iter()
        .map(|item| item.line_number - 1)
        .collect::<BTreeSet<_>>();
    let structural_plan = plan_structural_changes(
        &pomodoro_model,
        &original_resolved_references,
        &settings.done_statuses,
        &settings.status_types,
        &deleted_lines,
    );
    let structurally_updated_daily = apply_structural_plan(
        &daily_contents,
        &pomodoro_model,
        &structural_plan,
    );
    let empty_pomodoro_plan = plan_empty_pomodoro_removals(
        &structurally_updated_daily,
        &pomodoro_model,
    );
    let normalized_daily = apply_empty_pomodoro_plan(
        &structurally_updated_daily,
        &empty_pomodoro_plan,
    );
    let updated_lines = logical_lines(&normalized_daily);
    let updated_section =
        native_pomodoro::pomodoros_section_range(&updated_lines)
            .expect("the structural rewrite preserves the Pomodoros section");
    let updated_pomodoro_model =
        scan_pomodoros(&updated_lines, updated_section);
    let mut files = files;
    let canonical_daily_path = daily_path.canonicalize().ok();
    if let Some(file) = files.iter_mut().find(|file| {
        file.path == daily_path
            || canonical_daily_path.as_ref().is_some_and(|daily| {
                file.path.canonicalize().ok().as_ref() == Some(daily)
            })
    }) {
        file.contents = normalized_daily.clone();
        file.tasks = parse_tasks(&file.contents, &settings);
        file.note_kind = note_kind(&file.contents);
    }
    let note_index = NoteIndex::from_paths(
        files.iter().map(|file| file.relative_path.clone()),
    );
    let task_blocks = task_blocks(&files);
    let mut unresolved = Vec::new();
    let dependency_edges =
        dependency_edges(&files, &note_index, &task_blocks, &mut unresolved);
    let reference_resolver = TaskReferenceResolver {
        note_index: &note_index,
        task_blocks: &task_blocks,
        archive_catalog: &archive_catalog,
    };
    let mut resolved_references = BTreeMap::new();
    for reference in &updated_pomodoro_model.all_references {
        let Some(resolved) = resolve_task_reference(
            reference,
            daily_relative,
            &reference_resolver,
            ReferenceContext::CurrentDaily,
            &settings,
            &mut unresolved,
        ) else {
            continue;
        };
        resolved_references.insert(reference.clone(), resolved);
    }
    let direct_desired = updated_pomodoro_model
        .raw_references
        .iter()
        .filter_map(|reference| {
            resolved_references.get(reference).map(|resolved| {
                (resolved.path.clone(), reference.block_id.clone())
            })
        })
        .collect::<BTreeSet<_>>();
    let desired =
        desired_statuses(&direct_desired, &dependency_edges, &task_blocks);
    let dependency_desired = desired
        .keys()
        .filter(|identity| !direct_desired.contains(*identity))
        .cloned()
        .collect::<BTreeSet<_>>();
    let mut recent_activity_roots = updated_pomodoro_model
        .recent_references
        .iter()
        .filter_map(|reference| {
            resolved_references.get(reference).map(|resolved| {
                (resolved.path.clone(), reference.block_id.clone())
            })
        })
        .collect::<BTreeSet<_>>();
    recent_activity_roots.extend(resolve_recent_references(
        &previous_daily_references,
        previous_daily_relative.as_deref(),
        &reference_resolver,
        &settings,
        &mut unresolved,
    ));
    let recent_activity_references = recent_activity_roots.len();
    let recovery_desired = desired_statuses(
        &recent_activity_roots,
        &dependency_edges,
        &task_blocks,
    );
    let task_dependency_states = task_dependency_states(&files);

    let mut marked_next = Vec::new();
    let mut marked_in_progress = Vec::new();
    let mut cleared = Vec::new();
    // Kept for JSON compatibility; sticky lanes never clear In Progress.
    let cleared_in_progress: Vec<ChangeItem> = Vec::new();
    let mut marked_blocked = Vec::new();
    let mut unblocked = Vec::new();
    let mut changes = Vec::new();
    let mut kept_next = 0;
    let mut kept_in_progress = 0;
    for (file_index, file) in files.iter().enumerate() {
        if previous_daily_path
            .as_ref()
            .is_some_and(|path| file.path == *path)
        {
            continue;
        }
        let is_daily_note = canonical_daily_date(&file.relative_path).is_some()
            || file.path == daily_path;
        for (task_index, task) in file.tasks.iter().enumerate() {
            let desired_status = task.block_id.as_ref().and_then(|block_id| {
                desired
                    .get(&(file.relative_path.clone(), block_id.clone()))
                    .copied()
            });
            let recovery_status = task.block_id.as_ref().and_then(|block_id| {
                let identity = (file.relative_path.clone(), block_id.clone());
                desired
                    .get(&identity)
                    .copied()
                    .into_iter()
                    .chain(recovery_desired.get(&identity).copied())
                    .max()
            });
            let directly_recent =
                task.block_id.as_ref().is_some_and(|block_id| {
                    recent_activity_roots.contains(&(
                        file.relative_path.clone(),
                        block_id.clone(),
                    ))
                });
            let dependency_state = task_dependency_states
                .get(&(file_index, task_index))
                .cloned()
                .unwrap_or_default();
            let future_scheduled_date =
                task.scheduled.filter(|scheduled| *scheduled > anchor);
            let has_derived_block =
                !dependency_state.open_dependency_ids.is_empty()
                    || future_scheduled_date.is_some();
            match task_transition(
                task,
                desired_status,
                recovery_status,
                directly_recent,
                has_derived_block,
                is_daily_note,
            ) {
                Transition::MarkNext => {
                    let dependency =
                        task.block_id.as_ref().is_some_and(|block_id| {
                            dependency_desired.contains(&(
                                file.relative_path.clone(),
                                block_id.clone(),
                            ))
                        });
                    marked_next.push(change_item(file, task, dependency));
                    changes.push(PlannedChange {
                        file_index,
                        status_byte_offset: task.status_byte_offset,
                        replacement: '*',
                    });
                }
                Transition::MarkInProgress => {
                    let dependency =
                        task.block_id.as_ref().is_some_and(|block_id| {
                            dependency_desired.contains(&(
                                file.relative_path.clone(),
                                block_id.clone(),
                            ))
                        });
                    marked_in_progress
                        .push(change_item(file, task, dependency));
                    changes.push(PlannedChange {
                        file_index,
                        status_byte_offset: task.status_byte_offset,
                        replacement: '/',
                    });
                }
                Transition::Clear => {
                    cleared.push(change_item(file, task, false));
                    changes.push(PlannedChange {
                        file_index,
                        status_byte_offset: task.status_byte_offset,
                        replacement: ' ',
                    });
                }
                Transition::MarkBlocked => {
                    marked_blocked.push(dependency_status_change(
                        file,
                        task,
                        '?',
                        &dependency_state,
                        future_scheduled_date,
                    ));
                    changes.push(PlannedChange {
                        file_index,
                        status_byte_offset: task.status_byte_offset,
                        replacement: '?',
                    });
                }
                Transition::Unblock(status) => {
                    let replacement = status.checkbox();
                    unblocked.push(dependency_status_change(
                        file,
                        task,
                        replacement,
                        &dependency_state,
                        future_scheduled_date,
                    ));
                    changes.push(PlannedChange {
                        file_index,
                        status_byte_offset: task.status_byte_offset,
                        replacement,
                    });
                }
                Transition::KeptNext => kept_next += 1,
                Transition::KeptInProgress => kept_in_progress += 1,
                Transition::Unchanged => {}
            }
        }
    }

    if !marked_blocked.is_empty() || !unblocked.is_empty() {
        validate_blocked_status(&settings)?;
    }

    let compose = compose_outputs(
        &files,
        &changes,
        ComposeContext {
            daily_path: &daily_path,
            previous_daily_path: previous_daily_path.as_deref(),
            daily_contents: &daily_contents,
            normalized_daily_contents: &normalized_daily,
            settings: &settings,
        },
    );
    let apply_report = if request.dry_run {
        ApplyReport::default()
    } else {
        apply_guarded_outputs(
            &request.bob_dir,
            &inputs,
            &scan_paths,
            compose.outputs,
        )?
    };

    let plan_budget = plan_budget_for_sync(
        &request.bob_dir,
        &daily_contents,
        anchor,
        &daily_path,
    );

    Ok(SyncResult {
        ok: true,
        dry_run: request.dry_run,
        daily_file: daily_relative
            .map(display_path)
            .unwrap_or_else(|| daily_path.to_string_lossy().into_owned()),
        previous_daily_file: previous_daily_relative
            .as_deref()
            .map(display_path),
        open_pomodoros: pomodoro_model.open_pomodoros,
        references: pomodoro_model.raw_references.len(),
        previous_daily_references: previous_daily_references.len(),
        recent_activity_references,
        dependency_references: dependency_desired.len(),
        scanned_files: files.len(),
        marked_next,
        marked_in_progress,
        cleared,
        cleared_in_progress,
        marked_blocked,
        unblocked,
        struck_completed_references: structural_plan.struck,
        embedded_completed_references: Vec::new(),
        moved_completed_references: structural_plan.moved,
        marker_added_references: structural_plan.marker_added,
        marker_removed_references: structural_plan.marker_removed,
        removed_canceled_references: structural_plan.removed_canceled,
        removed_duplicate_lines,
        removed_empty_pomodoros: empty_pomodoro_plan.removed,
        grouped_task_sections: compose.grouped_task_sections,
        grouping_warnings: compose.grouping_warnings,
        applied_files: apply_report.applied_files,
        deferred_files: apply_report.deferred_files,
        recovery_directory: apply_report.recovery_directory,
        kept_next,
        kept_in_progress,
        unresolved_references: unresolved,
        plan_budget,
    })
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct TaskDependencyState {
    pub(super) open_dependency_ids: Vec<String>,
    pub(super) unresolved_dependency_ids: Vec<String>,
}

pub(super) fn task_dependency_states(
    files: &[FileScan],
) -> BTreeMap<(usize, usize), TaskDependencyState> {
    let mut identities = BTreeMap::<String, bool>::new();
    for file in files {
        for task in &file.tasks {
            let Some(task_id) = &task.task_id else {
                continue;
            };
            let is_open = identities.entry(task_id.clone()).or_default();
            *is_open |= task.status_recognized && task.status_type.is_open();
        }
    }

    let mut states = BTreeMap::new();
    for (file_index, file) in files.iter().enumerate() {
        for (task_index, task) in file.tasks.iter().enumerate() {
            let mut state = TaskDependencyState::default();
            for dependency in &task.depends_on {
                match identities.get(dependency) {
                    Some(true) => {
                        state.open_dependency_ids.push(dependency.clone())
                    }
                    None => {
                        state.unresolved_dependency_ids.push(dependency.clone())
                    }
                    Some(false) => {}
                }
            }
            state.open_dependency_ids.sort();
            state.open_dependency_ids.dedup();
            state.unresolved_dependency_ids.sort();
            state.unresolved_dependency_ids.dedup();
            states.insert((file_index, task_index), state);
        }
    }
    states
}

pub(super) fn task_transition(
    task: &TaskLine,
    desired: Option<RankedStatus>,
    recovery_desired: Option<RankedStatus>,
    directly_recent: bool,
    has_derived_block: bool,
    is_daily_note: bool,
) -> Transition {
    if task.status_type.is_terminal()
        || !task.status_type.is_open()
        || !task.status_recognized
    {
        return Transition::Unchanged;
    }
    if has_derived_block {
        return if task.status == '?' {
            Transition::Unchanged
        } else {
            Transition::MarkBlocked
        };
    }
    if task.status == '?' {
        return Transition::Unblock(
            recovery_desired.unwrap_or(RankedStatus::Ready),
        );
    }
    // Sticky lanes: an unlinked Next outside daily notes stays Next without
    // counting as kept_next. Inside daily notes the old policy holds: a
    // directly recent Next is kept, otherwise it clears.
    if task.status == '*' && desired.is_none() {
        if !is_daily_note {
            return Transition::Unchanged;
        }
        if directly_recent {
            return Transition::KeptNext;
        }
    }
    transition(task.status, desired)
}

pub(super) fn transition(
    status: char,
    desired: Option<RankedStatus>,
) -> Transition {
    let Some(desired) = desired else {
        return if status == '*' {
            Transition::Clear
        } else {
            Transition::Unchanged
        };
    };
    let Some(current) = RankedStatus::from_checkbox(status) else {
        return Transition::Unchanged;
    };
    if current < desired {
        return match desired {
            RankedStatus::Next => Transition::MarkNext,
            RankedStatus::InProgress => Transition::MarkInProgress,
            RankedStatus::Ready => Transition::Unchanged,
        };
    }
    match current {
        RankedStatus::Next => Transition::KeptNext,
        RankedStatus::InProgress => Transition::KeptInProgress,
        RankedStatus::Ready => Transition::Unchanged,
    }
}
