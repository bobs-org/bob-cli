//! Immediate Blocked-dependent recovery for whole-item completion.
//!
//! Reuses reconcile's parsed [`FileScan`]/[`TaskLine`] types and the
//! open-dependency rule of [`task_dependency_states`], so there is exactly
//! one definition of "open dependency". This matches the Ctrl+Enter
//! recovery paragraph in `docs/task-status-hooks.md`: only Blocked
//! dependents that directly name a completed task, have no other open
//! dependency in the post-completion snapshot, have no strictly
//! future `scheduled` date, and are not still blocked by their
//! project recover, and they always recover to Ready. When a `^prj`
//! recovers, residents with no remaining own reason recover with it
//! (`docs/task-dependencies.md` §12.2 PROJECT RECOVERY); every Ready
//! recovery takes the project-close writes (tag strip plus
//! `[unblocked::]`).

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use chrono::NaiveDate;

use super::super::{
    capture_task_toggle::set_task_line_status,
    project_dependencies::{blocking_open_dependency_ids, is_project_task_id},
    projects::edits::normalize_prj_is_blocked_tag,
    task_status_hooks::{
        parse_tasks, task_dependency_states, FileScan, TasksSettings,
    },
    vault_links::NoteIndex,
};
use super::project_close::{
    apply_recovery_writes, has_own_remaining_reason, project_states_for_files,
    project_still_blocks, unblock_scope_for,
};

/// One recovered dependent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RecoveredDependent {
    pub(crate) relative_path: PathBuf,
    /// 1-based line number.
    pub(crate) line: usize,
    pub(crate) block_id: Option<String>,
    pub(crate) text: String,
    pub(crate) previous_status_symbol: char,
    pub(crate) status_symbol: char,
}

/// Result of [`recover_blocked_dependents`].
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct DependentRecovery {
    /// Post-images for changed files only, keyed by relative path.
    pub(crate) changed_files: BTreeMap<PathBuf, String>,
    pub(crate) recovered: Vec<RecoveredDependent>,
}

/// Recover Blocked dependents over a staged snapshot.
///
/// `snapshot` yields (vault-relative path, staged text) pairs with the
/// batch's overlays applied. `completed_ids` holds the batch's completed
/// task `[id::]` values. A dependent recovers to Ready via
/// `set_task_line_status` when it is Blocked, names a completed id in its
/// `dependsOn`, has no other open dependency, has no strictly future
/// `scheduled` date, and is not still blocked by its project. When a
/// `^prj` recovers, residents with no remaining own reason recover too.
/// Unresolved ids never block.
pub(crate) fn recover_blocked_dependents<I>(
    snapshot: I,
    completed_ids: &BTreeSet<String>,
    today: NaiveDate,
    settings: &TasksSettings,
) -> DependentRecovery
where
    I: IntoIterator<Item = (PathBuf, String)>,
{
    let mut files = Vec::new();
    for (relative_path, contents) in snapshot {
        let tasks = parse_tasks(&contents, settings);
        files.push(FileScan {
            path: relative_path.clone(),
            relative_path,
            contents,
            tasks,
        });
    }
    let states = task_dependency_states(&files, &BTreeSet::new());
    // Snapshot-scoped resolution for the tag proxy: a tagged `^prj`
    // whose parent is outside the snapshot keeps its own path as the
    // holder, so close recovery never contradicts the cascade.
    let index = NoteIndex::from_paths(
        files.iter().map(|file| file.relative_path.clone()),
    );
    let project_states =
        project_states_for_files(&files, &states, today, &index);
    let mut recover: BTreeSet<(usize, usize)> = BTreeSet::new();
    let mut recovering_projects: BTreeSet<usize> = BTreeSet::new();
    for (file_index, file) in files.iter().enumerate() {
        for (task_index, task) in file.tasks.iter().enumerate() {
            if task.status != '?' || !task.status_recognized {
                continue;
            }
            if !task
                .depends_on
                .iter()
                .any(|dependency| completed_ids.contains(dependency))
            {
                continue;
            }
            let open_count = blocking_open_dependency_ids(
                file,
                task,
                states
                    .get(&(file_index, task_index))
                    .map(|state| state.open_dependency_ids.as_slice())
                    .unwrap_or(&[]),
            )
            .len();
            if has_own_remaining_reason(task, open_count, today) {
                continue;
            }
            if project_still_blocks(file_index, task, &project_states).is_some()
            {
                continue;
            }
            // A tagged `^prj` inherits its block from an ancestor (PDC5):
            // close recovery never recovers it, whatever its own reasons.
            if task
                .block_id
                .as_deref()
                .is_some_and(|id| is_project_task_id(Some(id)))
                && project_states
                    .get(file_index)
                    .is_some_and(|state| state.inherited.is_some())
            {
                continue;
            }
            recover.insert((file_index, task_index));
            if task
                .block_id
                .as_deref()
                .is_some_and(|id| is_project_task_id(Some(id)))
            {
                recovering_projects.insert(file_index);
            }
        }
    }
    // PROJECT RECOVERY (§12.2): a recovered `^prj` recovers its
    // residents with no own remaining reason. The project's reasons
    // are empty whenever its `^prj` recovered (they are the `^prj`'s
    // own reasons); the guard below keeps that invariant explicit.
    for file_index in &recovering_projects {
        let Some(state) = project_states.get(*file_index) else {
            continue;
        };
        // An inherited (tagged) project recovers nothing here (PDC5).
        if !state.reasons.is_empty() || state.inherited.is_some() {
            continue;
        }
        let context = &state.context;
        let file = &files[*file_index];
        for (task_index, task) in file.tasks.iter().enumerate() {
            if !context.is_resident(task) || task.status != '?' {
                continue;
            }
            let open_count = blocking_open_dependency_ids(
                file,
                task,
                states
                    .get(&(*file_index, task_index))
                    .map(|state| state.open_dependency_ids.as_slice())
                    .unwrap_or(&[]),
            )
            .len();
            if has_own_remaining_reason(task, open_count, today) {
                continue;
            }
            recover.insert((*file_index, task_index));
        }
    }
    // PDC3: remaining open residents of a recovered project still lose
    // `#prj_is_blocked`, including rows that stay `[?]` for their own
    // reasons and open rows that were never `[?]`.
    let mut strip_only: BTreeSet<(usize, usize)> = BTreeSet::new();
    for file_index in recovering_projects {
        let Some(state) = project_states.get(file_index) else {
            continue;
        };
        if !state.reasons.is_empty() || state.inherited.is_some() {
            continue;
        }
        let context = &state.context;
        let file = &files[file_index];
        for (task_index, task) in file.tasks.iter().enumerate() {
            if !context.is_resident(task)
                || recover.contains(&(file_index, task_index))
            {
                continue;
            }
            strip_only.insert((file_index, task_index));
        }
    }
    let mut changed_files = BTreeMap::new();
    let mut recovered = Vec::new();
    let mut by_file: BTreeMap<usize, Vec<(usize, bool)>> = BTreeMap::new();
    for (file_index, task_index) in recover {
        by_file
            .entry(file_index)
            .or_default()
            .push((task_index, true));
    }
    for (file_index, task_index) in strip_only {
        by_file
            .entry(file_index)
            .or_default()
            .push((task_index, false));
    }
    for (file_index, mut task_indices) in by_file {
        task_indices.sort_unstable();
        let file = &files[file_index];
        let mut updated: Option<String> = None;
        for (task_index, recover_row) in task_indices {
            let task = &file.tasks[task_index];
            let contents = updated.as_ref().unwrap_or(&file.contents);
            let Some(line) = contents
                .split_inclusive('\n')
                .nth(task.line_index)
                .map(|segment| {
                    segment.strip_suffix('\n').unwrap_or(segment).to_string()
                })
            else {
                continue;
            };
            let had_cr = line.ends_with('\r');
            let trimmed =
                line.strip_suffix('\r').unwrap_or(line.as_str()).to_string();
            let mut replacement = if recover_row {
                let Some(flipped) = set_task_line_status(&trimmed, ' ') else {
                    continue;
                };
                // This engine never observes the day text, so every
                // recovery lands Ready and `is_today` scope reads false:
                // the UB4 landing skip still applies structurally through
                // the recovered status.
                let scope =
                    unblock_scope_for(&file.relative_path, &flipped, false);
                apply_recovery_writes(&flipped, today, ' ', &scope)
            } else {
                let stripped = normalize_prj_is_blocked_tag(&trimmed, false);
                if stripped == trimmed {
                    continue;
                }
                stripped
            };
            if had_cr {
                replacement.push('\r');
            }
            let base = updated.take().unwrap_or_else(|| file.contents.clone());
            let start = line_start_offset(&base, task.line_index);
            let end = start + line.len();
            let mut next = base;
            next.replace_range(start..end, &replacement);
            updated = Some(next);
            if recover_row {
                recovered.push(RecoveredDependent {
                    relative_path: file.relative_path.clone(),
                    line: task.line_index + 1,
                    block_id: task.block_id.clone(),
                    text: task.description.clone(),
                    previous_status_symbol: task.status,
                    status_symbol: ' ',
                });
            }
        }
        if let Some(contents) = updated
            && contents != file.contents
        {
            changed_files.insert(file.relative_path.clone(), contents);
        }
    }
    DependentRecovery {
        changed_files,
        recovered,
    }
}

/// Byte offset where logical line `line_index` starts.
fn line_start_offset(contents: &str, line_index: usize) -> usize {
    let mut offset = 0;
    for segment in contents.split_inclusive('\n').take(line_index) {
        offset += segment.len();
    }
    offset
}
