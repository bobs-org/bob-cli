//! Immediate Blocked-dependent recovery for whole-item completion.
//!
//! Reuses reconcile's parsed [`FileScan`]/[`TaskLine`] types and the
//! open-dependency rule of [`task_dependency_states`], so there is exactly
//! one definition of "open dependency". This matches the Ctrl+Enter
//! recovery paragraph in `docs/task-status-hooks.md`: only Blocked
//! dependents that directly name a completed task, have no other open
//! dependency in the post-completion snapshot, and have no strictly
//! future `scheduled` date recover, and they always recover to Ready.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use chrono::NaiveDate;

use super::super::{
    capture_task_toggle::set_task_line_status,
    task_status_hooks::{
        parse_tasks, task_dependency_states, FileScan, TasksSettings,
    },
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
/// `dependsOn`, has no other open dependency, and has no strictly future
/// `scheduled` date. Unresolved ids never block.
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
    let mut changed_files = BTreeMap::new();
    let mut recovered = Vec::new();
    for (file_index, file) in files.iter().enumerate() {
        let mut updated: Option<String> = None;
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
            let open = states
                .get(&(file_index, task_index))
                .map(|state| !state.open_dependency_ids.is_empty())
                .unwrap_or(false);
            if open {
                continue;
            }
            if task.scheduled.is_some_and(|date| date > today) {
                continue;
            }
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
            let Some(replacement) = set_task_line_status(&line, ' ') else {
                continue;
            };
            let base = updated.take().unwrap_or_else(|| file.contents.clone());
            let start = line_start_offset(&base, task.line_index);
            let end = start + line.len();
            let mut next = base;
            next.replace_range(start..end, &replacement);
            updated = Some(next);
            recovered.push(RecoveredDependent {
                relative_path: file.relative_path.clone(),
                line: task.line_index + 1,
                block_id: task.block_id.clone(),
                text: task.description.clone(),
                previous_status_symbol: task.status,
                status_symbol: ' ',
            });
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
