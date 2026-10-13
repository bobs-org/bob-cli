//! Daily-note selection and task-status sync orchestration.
use super::*;

use super::super::capture_dependency_tasks::{
    has_hide_tag, has_prj_is_blocked_tag,
};
use super::super::freshness::placement::{
    mark_unblocked, prune_unblocked, UnblockScope,
};
use super::super::project_dependencies::{
    blocking_open_dependency_ids, effective_block_reasons,
    filter_internal_project_ids, project_block_reasons, BlockReason,
    ProjectBlockReasons, ProjectContext, ProjectTree,
};
use super::super::projects::edits::normalize_prj_is_blocked_tag;
use super::super::task_complete::is_recurring_task_line;

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

pub(crate) fn canonical_daily_date(relative_path: &Path) -> Option<NaiveDate> {
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
/// half plus the Today rows and lane meters. It never changes the
/// exit code and never writes. An invalid plan config yields `None`
/// (JSON `null`) and a single stderr warning; a missing Pomodoros
/// section also yields `None`.
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
    let lanes = plan_budget::count_lanes(bob_dir, anchor, &config);
    let today_result =
        plan_budget::today::today_tasks(bob_dir, &daily_file, daily_contents);
    Some(plan_budget::assemble_report(
        anchor,
        &daily_file,
        &config,
        &ledger,
        &lanes,
        &today_result,
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

/// Load `dependencies.blocks_lines`, falling back to `true` and warning
/// on stderr when the value is invalid (BR25).
fn load_blocks_lines_enabled() -> bool {
    match bob_config::load_dependencies_config(&bob_config::config_path()) {
        Ok(config) => config.blocks_lines,
        Err(error) => {
            eprintln!(
                "{COMMAND_NAME}: warning: invalid dependencies config: {error}; using dependencies.blocks_lines=true"
            );
            true
        }
    }
}

/// Files whose mtime is in the future or unreadable: the guarded write
/// would refuse them without waiting, so a mint on that note must not
/// be listed this run (BR5).
fn quiet_deferred_files(files: &[FileScan]) -> BTreeSet<usize> {
    let now = SystemTime::now();
    files
        .iter()
        .enumerate()
        .filter(|(_, file)| file_would_defer_quietly(&file.path, now))
        .map(|(index, _)| index)
        .collect()
}

fn file_would_defer_quietly(path: &Path, now: SystemTime) -> bool {
    let Ok(metadata) = fs::symlink_metadata(path) else {
        return true;
    };
    let Some(mtime) = metadata.modified().ok() else {
        return true;
    };
    now.duration_since(mtime).is_err()
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
        files.push(FileScan {
            path,
            relative_path,
            contents,
            tasks,
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
        false,
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
    }
    let note_index = NoteIndex::from_paths(
        files.iter().map(|file| file.relative_path.clone()),
    );
    // Recognition and `parent:` do not change in R1–R10. Build the
    // hierarchy from this pre-reconcile scan so both dependency projection
    // and the later status pass share one resolution of the tree.
    let project_contexts: Vec<ProjectContext> =
        files.iter().map(ProjectContext::for_note).collect();
    let empty_project_reasons =
        vec![ProjectBlockReasons::default(); files.len()];
    let mut project_tree = ProjectTree::build(
        &files,
        &project_contexts,
        &empty_project_reasons,
        &note_index,
    );
    let task_counts: Vec<usize> =
        files.iter().map(|file| file.tasks.len()).collect();
    let reconcile = reconcile_dependencies(
        &request.bob_dir,
        &mut files,
        &note_index,
        &project_tree,
        &settings,
        previous_daily_path.as_deref(),
        &mut inputs,
    );
    for file_index in &reconcile.touched_files {
        let file = &mut files[*file_index];
        file.tasks = parse_tasks(&file.contents, &settings);
    }
    for (file_index, file) in files.iter().enumerate() {
        debug_assert_eq!(
            file.tasks.len(),
            task_counts[file_index],
            "R1–R10 never adds or removes task lines"
        );
    }
    let previous_daily_file = previous_daily_path
        .as_ref()
        .and_then(|path| files.iter().position(|file| &file.path == path));
    let quiet_deferred = quiet_deferred_files(&files);
    let blocks_enabled = load_blocks_lines_enabled();
    let mut pre_reconcile = reconcile.original_contents;
    let blocks = reconcile_blocks_lines(
        &mut files,
        &note_index,
        &settings,
        &reconcile.explicit_edges,
        &reconcile.uncertain_dependents,
        previous_daily_file,
        &quiet_deferred,
        blocks_enabled,
    );
    for (file_index, original) in blocks.original_contents {
        pre_reconcile.entry(file_index).or_insert(original);
    }
    let mut reconcile_touched = reconcile.touched_files;
    reconcile_touched.extend(blocks.touched_files);
    for file_index in files.iter().enumerate().map(|(index, _)| index) {
        debug_assert_eq!(
            files[file_index].tasks.len(),
            task_counts[file_index],
            "BLOCKS projection never adds or removes task lines"
        );
    }
    let mut dependency_warnings = reconcile.warnings;
    dependency_warnings.extend(blocks.warnings);
    let task_blocks = task_blocks(&files);
    let mut unresolved = Vec::new();
    let dependency_edges = dependency_edges(
        &files,
        &note_index,
        &task_blocks,
        &project_tree,
        &mut unresolved,
    );
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
    // ADJ-11 limits dependency promotion: a `^prj` target reached
    // through a dep edge is never raised, but a directly linked `^prj`
    // is desired at Next like any linked task. The walk continues
    // through it either way.
    let project_task_identities = project_task_identities(&files);
    let desired = desired_statuses(
        &direct_desired,
        &dependency_edges,
        &task_blocks,
        &project_task_identities,
    );
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
        &project_task_identities,
    );
    let task_dependency_states =
        task_dependency_states(&files, &reconcile.archive_closed_ids);
    // Project blocking (`docs/task-dependencies.md` §13.3): derive own
    // reasons from the reconciled fields, then remove same-note and
    // descendant-internal targets before publishing them through the tree.
    let project_reasons: Vec<ProjectBlockReasons> = files
        .iter()
        .enumerate()
        .map(|(file_index, file)| {
            let context = &project_contexts[file_index];
            if !context.recognised {
                return ProjectBlockReasons::default();
            }
            let open_ids = context
                .project_task_index()
                .and_then(|task_index| {
                    task_dependency_states.get(&(file_index, task_index))
                })
                .map(|state| state.open_dependency_ids.clone())
                .unwrap_or_default();
            let open_ids = filter_internal_project_ids(file, &open_ids);
            let open_ids = project_tree
                .filter_descendant_ids(file_index, &files, &open_ids);
            project_block_reasons(context, &open_ids, anchor)
        })
        .collect();
    project_tree.set_project_reasons(&files, project_reasons);

    let mut marked_next = Vec::new();
    let mut marked_in_progress = Vec::new();
    let mut cleared = Vec::new();
    // Kept for JSON compatibility; sticky lanes never clear In Progress.
    let cleared_in_progress: Vec<ChangeItem> = Vec::new();
    let mut marked_blocked = Vec::new();
    let mut unblocked = Vec::new();
    let mut prj_is_blocked_tags_added = Vec::new();
    let mut prj_is_blocked_tags_stripped = Vec::new();
    let mut unblocked_fields_written = Vec::new();
    let mut unblocked_fields_pruned = Vec::new();
    let mut blocked_identities = BTreeSet::<(PathBuf, String)>::new();
    let mut blocked_reason_map =
        BTreeMap::<(PathBuf, String), Vec<BlockReason>>::new();
    let mut changes = Vec::new();
    let mut line_rewrite_files = BTreeSet::new();
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
        let file_lines = logical_lines(&file.contents);
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
            // Effective blocking (§13.3): the task's own reasons plus
            // its project's `^prj` reasons when it is a resident. A
            // hand-set `[?]` on `^prj` is not a reason and blocks
            // nobody; recovery needs every reason gone. Same-note
            // ids on `^prj` stay off the live graph (PDR7).
            let open_ids = blocking_open_dependency_ids(
                file,
                task,
                &dependency_state.open_dependency_ids,
            );
            let open_ids = if project_tree.project_task_index(file_index)
                == Some(task_index)
            {
                let open_ids = filter_internal_project_ids(file, &open_ids);
                project_tree
                    .filter_descendant_ids(file_index, &files, &open_ids)
            } else {
                open_ids
            };
            let block_reasons = effective_block_reasons(
                task,
                &open_ids,
                &project_contexts[file_index],
                project_tree.own_reasons(file_index),
                project_tree.inherited(file_index),
                anchor,
            );
            let has_derived_block = !block_reasons.is_empty();
            // Final Blocked set for consumption (§13.5): newly blocked
            // plus unchanged `[?]` with a remaining reason. Keyed by
            // identity, never by the `'?'` cache.
            if has_derived_block
                && task.status_recognized
                && task.status_type.is_open()
                && let Some(block_id) = &task.block_id
            {
                let identity = (file.relative_path.clone(), block_id.clone());
                blocked_identities.insert(identity.clone());
                blocked_reason_map.insert(identity, block_reasons.clone());
            }
            let transition = task_transition(
                task,
                desired_status,
                recovery_status,
                directly_recent,
                has_derived_block,
                is_daily_note,
            );
            let replacement = match transition {
                Transition::MarkNext => Some('*'),
                Transition::MarkInProgress => Some('/'),
                Transition::Clear => Some(' '),
                Transition::MarkBlocked => Some('?'),
                Transition::Unblock(status) => Some(status.checkbox()),
                Transition::KeptNext
                | Transition::KeptInProgress
                | Transition::Unchanged => None,
            };
            match transition {
                Transition::MarkNext => {
                    let dependency =
                        task.block_id.as_ref().is_some_and(|block_id| {
                            dependency_desired.contains(&(
                                file.relative_path.clone(),
                                block_id.clone(),
                            ))
                        });
                    marked_next.push(change_item(file, task, dependency));
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
                }
                Transition::Clear => {
                    cleared.push(change_item(file, task, false));
                }
                Transition::MarkBlocked => {
                    marked_blocked.push(dependency_status_change(
                        file,
                        task,
                        '?',
                        &dependency_state,
                        &block_reasons,
                    ));
                }
                Transition::Unblock(status) => {
                    unblocked.push(dependency_status_change(
                        file,
                        task,
                        status.checkbox(),
                        &dependency_state,
                        &block_reasons,
                    ));
                }
                Transition::KeptNext => kept_next += 1,
                Transition::KeptInProgress => kept_in_progress += 1,
                Transition::Unchanged => {}
            }
            // One line rewrite carries the status plus the derived
            // tag and `[unblocked::]` edits (§13.3, §13.7, UB vectors).
            // Closed lines are never edited.
            let task_open =
                task.status_recognized && task.status_type.is_open();
            let new_line = if task_open {
                plan_task_line_rewrite(
                    file,
                    task,
                    file_lines.get(task.line_index).copied().unwrap_or(""),
                    replacement,
                    &project_contexts[file_index],
                    project_tree.own_reasons(file_index),
                    project_tree.inherited(file_index),
                    transition,
                    anchor,
                    is_daily_note,
                    &direct_desired,
                    &mut prj_is_blocked_tags_added,
                    &mut prj_is_blocked_tags_stripped,
                    &mut unblocked_fields_written,
                    &mut unblocked_fields_pruned,
                )
            } else {
                None
            };
            if let Some(replacement) = replacement {
                // Status-only flips are the plain checkbox swap. Tag,
                // `[unblocked::]`, and prune rewrites are structural
                // and wait out the quiet period.
                let status_only = swap_status_byte(
                    file_lines.get(task.line_index).copied().unwrap_or(""),
                    task.status,
                    replacement,
                );
                if new_line.as_ref().is_some_and(|line| line != &status_only) {
                    line_rewrite_files.insert(file_index);
                }
                changes.push(PlannedChange {
                    file_index,
                    status_byte_offset: task.status_byte_offset,
                    replacement,
                    line_index: task.line_index,
                    new_line,
                });
            } else if let Some(new_line) = new_line {
                // Tag or prune edits with no status change still need
                // a plan entry so the rewrite reaches the write.
                // The byte swap is a no-op on the current status.
                line_rewrite_files.insert(file_index);
                changes.push(PlannedChange {
                    file_index,
                    status_byte_offset: task.status_byte_offset,
                    replacement: task.status,
                    line_index: task.line_index,
                    new_line: Some(new_line),
                });
            }
        }
    }

    if !marked_blocked.is_empty() || !unblocked.is_empty() {
        validate_blocked_status(&settings)?;
    }

    // Line rewrites (tag and `[unblocked::]` edits) are structural,
    // so their notes join the reconciled set for the quiet-period
    // guarded write.
    let mut structural_files = reconcile_touched;
    structural_files.extend(line_rewrite_files);
    let mut compose = compose_outputs(
        &files,
        &changes,
        ComposeContext {
            daily_path: &daily_path,
            previous_daily_path: previous_daily_path.as_deref(),
            daily_contents: &daily_contents,
            normalized_daily_contents: &normalized_daily,
            settings: &settings,
        },
        &structural_files,
        &pre_reconcile,
    );
    // Second rewrite pass after transitions (§13.5): consume live
    // links to the final Blocked set, then empty Pomodoros. Promotion
    // already seeded from those links via `direct_desired`.
    let (consumed_blocked_references, consumed_empty_pomodoros) =
        consume_blocked_links_after_compose(
            &mut compose,
            &files,
            &daily_path,
            canonical_daily_path.as_ref(),
            &daily_contents,
            &normalized_daily,
            daily_relative,
            &reference_resolver,
            &settings,
            &blocked_identities,
            &blocked_reason_map,
            &dependency_edges,
            &task_blocks,
            &project_task_identities,
        );
    let mut removed_empty_pomodoros = empty_pomodoro_plan.removed;
    removed_empty_pomodoros.extend(consumed_empty_pomodoros);
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
        consumed_blocked_references,
        removed_duplicate_lines,
        removed_empty_pomodoros,
        grouped_task_sections: compose.grouped_task_sections,
        grouping_warnings: compose.grouping_warnings,
        applied_files: apply_report.applied_files,
        deferred_files: apply_report.deferred_files,
        recovery_directory: apply_report.recovery_directory,
        kept_next,
        kept_in_progress,
        unresolved_references: unresolved,
        dependency_projection_updates: reconcile.projection_updates,
        adopted_dependency_lines: reconcile.adopted,
        healed_dependency_links: reconcile.healed,
        canonicalized_dependency_lines: reconcile.canonicalized,
        legacy_dependency_children: reconcile.legacy_children,
        dependency_warnings,
        blocks_line_updates: blocks.updates,
        minted_dependent_block_ids: blocks.mints,
        prj_is_blocked_tags_added,
        prj_is_blocked_tags_stripped,
        unblocked_fields_written,
        unblocked_fields_pruned,
        plan_budget,
    })
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct TaskDependencyState {
    pub(crate) open_dependency_ids: Vec<String>,
    pub(crate) unresolved_dependency_ids: Vec<String>,
}

pub(crate) fn task_dependency_states(
    files: &[FileScan],
    archive_closed_ids: &BTreeSet<String>,
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
                if archive_closed_ids.contains(dependency) {
                    // Archive prerequisites are closed history: kept,
                    // never warned about, never blocking.
                    continue;
                }
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

/// One full line rewrite for an open task: the status swap plus the
/// derived `#prj_is_blocked` tag edit and the `[unblocked::]` write or
/// prune (`docs/task-dependencies.md` §13.3, §13.7, UB vectors).
/// Returns `None` when the line is already right. Closed lines never
/// reach here; fenced lines keep their tag state untouched.
#[allow(clippy::too_many_arguments)]
pub(super) fn plan_task_line_rewrite(
    file: &FileScan,
    task: &TaskLine,
    raw_line: &str,
    replacement: Option<char>,
    context: &ProjectContext,
    project_reasons: &ProjectBlockReasons,
    inherited_reasons: &[(PathBuf, ProjectBlockReasons)],
    transition: Transition,
    anchor: NaiveDate,
    is_daily_note: bool,
    direct_desired: &BTreeSet<(PathBuf, String)>,
    tags_added: &mut Vec<ProjectBlockedTagChange>,
    tags_stripped: &mut Vec<ProjectBlockedTagChange>,
    fields_written: &mut Vec<UnblockedFieldChange>,
    fields_pruned: &mut Vec<UnblockedFieldChange>,
) -> Option<String> {
    let mut line = raw_line.to_string();
    if let Some(to) = replacement {
        line = swap_status_byte(&line, task.status, to);
    }
    // Residents carry the tag when their project has any own or inherited
    // reason. A project's `^prj` carries it only for inherited reasons.
    if !context.is_fenced(task.line_index) {
        let want_tag = if context.is_project_task(task) {
            !inherited_reasons.is_empty()
        } else {
            context.is_resident(task)
                && (!project_reasons.is_empty()
                    || !inherited_reasons.is_empty())
        };
        let tagged = normalize_prj_is_blocked_tag(&line, want_tag);
        if tagged != line {
            let change = ProjectBlockedTagChange {
                path: display_path(&file.relative_path),
                line_number: task.line_index + 1,
                block_id: task.block_id.clone().unwrap_or_default(),
            };
            if want_tag {
                // Count only real additions: moving a misplaced
                // existing tag is a rewrite, not an add.
                if !has_prj_is_blocked_tag(raw_line) {
                    tags_added.push(change);
                }
            } else {
                tags_stripped.push(change);
            }
            line = tagged;
        }
    }
    let mut wrote_unblocked = false;
    if matches!(transition, Transition::Unblock(RankedStatus::Ready)) {
        let is_today = task.block_id.as_ref().is_some_and(|block_id| {
            direct_desired
                .contains(&(file.relative_path.clone(), block_id.clone()))
        });
        let scope = UnblockScope {
            recurring: is_recurring_task_line(&line),
            hidden: has_hide_tag(&line),
            is_daily_note,
            is_today,
        };
        if let Some(marked) = mark_unblocked(
            &line,
            anchor,
            RankedStatus::Ready.checkbox(),
            &scope,
        ) {
            fields_written.push(UnblockedFieldChange {
                path: display_path(&file.relative_path),
                line_number: task.line_index + 1,
                block_id: task.block_id.clone().unwrap_or_default(),
                date: anchor.to_string(),
            });
            line = marked;
            wrote_unblocked = true;
        }
    }
    if !wrote_unblocked && let Some(pruned) = prune_unblocked(&line, anchor) {
        fields_pruned.push(UnblockedFieldChange {
            path: display_path(&file.relative_path),
            line_number: task.line_index + 1,
            block_id: task.block_id.clone().unwrap_or_default(),
            date: unblocked_field_date(&line)
                .unwrap_or_else(|| anchor.to_string()),
        });
        line = pruned;
    }
    (line != raw_line).then_some(line)
}

/// Swap the checkbox byte on a parsed task line. The checkbox is the
/// first `[<status>]` group on the line, so a first-occurrence swap
/// hits it and never body text.
fn swap_status_byte(line: &str, from: char, to: char) -> String {
    if from == to {
        return line.to_string();
    }
    let mut output = line.to_string();
    let needle = format!("[{from}]");
    if let Some(start) = output.find(&needle) {
        output.replace_range(
            start + 1..start + 1 + from.len_utf8(),
            &to.to_string(),
        );
    }
    output
}

/// The date carried by the first `[unblocked:: …]` field on the line.
fn unblocked_field_date(line: &str) -> Option<String> {
    let start = line.find("[unblocked::")? + "[unblocked::".len();
    let end = line[start..].find(']')?;
    Some(line[start..start + end].trim().to_string())
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

fn consume_blocked_links_after_compose(
    compose: &mut ComposeResult,
    files: &[FileScan],
    daily_path: &Path,
    canonical_daily_path: Option<&PathBuf>,
    daily_contents: &str,
    normalized_daily: &str,
    daily_relative: Option<&Path>,
    reference_resolver: &TaskReferenceResolver<'_>,
    settings: &TasksSettings,
    blocked: &BTreeSet<(PathBuf, String)>,
    blocked_reasons: &BTreeMap<(PathBuf, String), Vec<BlockReason>>,
    edges: &BTreeMap<(PathBuf, String), BTreeSet<(PathBuf, String)>>,
    task_blocks: &BTreeMap<(PathBuf, String), Vec<char>>,
    project_tasks: &BTreeSet<(PathBuf, String)>,
) -> (Vec<ConsumedBlockedReference>, Vec<RemovedEmptyPomodoro>) {
    if blocked.is_empty() {
        return (Vec::new(), Vec::new());
    }
    let daily_output_index = compose.outputs.iter().position(|output| {
        output.path == daily_path
            || canonical_daily_path.is_some_and(|daily| {
                output.path.canonicalize().ok().as_ref() == Some(daily)
            })
    });
    let daily_text = daily_output_index
        .map(|index| compose.outputs[index].contents.clone())
        .or_else(|| {
            files.iter().find_map(|file| {
                (file.path == daily_path
                    || canonical_daily_path.is_some_and(|daily| {
                        file.path.canonicalize().ok().as_ref() == Some(daily)
                    }))
                .then(|| file.contents.clone())
            })
        })
        .unwrap_or_else(|| normalized_daily.to_string());
    let consume_lines = logical_lines(&daily_text);
    let mut consume_resolved = BTreeMap::new();
    let mut consume_unresolved = Vec::new();
    if let Some(section) =
        native_pomodoro::pomodoros_section_range(&consume_lines)
    {
        let consume_model = scan_pomodoros(&consume_lines, section);
        for reference in &consume_model.all_references {
            if let Some(resolved) = resolve_task_reference(
                reference,
                daily_relative,
                reference_resolver,
                ReferenceContext::CurrentDaily,
                settings,
                &mut consume_unresolved,
            ) {
                consume_resolved.insert(reference.clone(), resolved);
            }
        }
    }
    let consumption =
        apply_blocked_consumption(&daily_text, &consume_resolved, blocked);
    if consumption.contents != daily_text {
        if let Some(index) = daily_output_index {
            compose.outputs[index].contents = consumption.contents;
        } else if consumption.contents != daily_contents {
            compose.outputs.push(ComposedOutput {
                path: daily_path.to_path_buf(),
                contents: consumption.contents,
                structural_regrouping: false,
            });
        }
    }
    let daily_display = daily_relative
        .map(display_path)
        .unwrap_or_else(|| daily_path.to_string_lossy().into_owned());
    let consumed = enrich_consumed_blocked(
        &daily_display,
        consumption.consumed,
        blocked_reasons,
        edges,
        task_blocks,
        project_tasks,
    );
    (consumed, consumption.removed_empty)
}

fn enrich_consumed_blocked(
    daily_path: &str,
    consumed: Vec<RemovedBlockedReference>,
    blocked_reasons: &BTreeMap<(PathBuf, String), Vec<BlockReason>>,
    edges: &BTreeMap<(PathBuf, String), BTreeSet<(PathBuf, String)>>,
    task_blocks: &BTreeMap<(PathBuf, String), Vec<char>>,
    project_tasks: &BTreeSet<(PathBuf, String)>,
) -> Vec<ConsumedBlockedReference> {
    consumed
        .into_iter()
        .map(|item| {
            let identity = (item.task_path.clone(), item.block_id.clone());
            let reasons = blocked_reasons
                .get(&identity)
                .map(|reasons| {
                    reasons
                        .iter()
                        .map(|reason| reason.kind().to_string())
                        .collect()
                })
                .unwrap_or_default();
            let seed = BTreeSet::from([identity.clone()]);
            let promoted =
                desired_statuses(&seed, edges, task_blocks, project_tasks)
                    .into_keys()
                    .filter(|promoted| promoted != &identity)
                    .map(|(path, block_id)| DuplicateTaskIdentity {
                        path: display_path(&path),
                        block_id,
                    })
                    .collect();
            ConsumedBlockedReference {
                path: daily_path.to_string(),
                line: item.line_number,
                pomodoro: item.pomodoro,
                task: DuplicateTaskIdentity {
                    path: display_path(&item.task_path),
                    block_id: item.block_id,
                },
                reasons,
                promoted,
            }
        })
        .collect()
}
