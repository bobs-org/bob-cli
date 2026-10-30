//! Cancellation recovery through the production successor/project rules.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use chrono::NaiveDate;

use crate::native::{
    note_tasks::NoteTaskSettings,
    project_dependencies::is_project_task_id,
    task_complete::successors::{
        plan_recovery_only, CompletedTask, SuccessorInput, SuccessorPlan,
        SuccessorSelection,
    },
    task_status_hooks::{parse_tasks, TasksSettings},
    vault_links::NoteIndex,
};

use super::prune::TaskIdentity;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct RecoveredTask {
    pub(crate) path: PathBuf,
    pub(crate) line: usize,
    pub(crate) block_id: Option<String>,
    pub(crate) text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct RecoveryPlan {
    pub(crate) changed_files: BTreeMap<PathBuf, String>,
    pub(crate) recovered: Vec<RecoveredTask>,
    pub(crate) successor: SuccessorPlan,
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn recover_cancelled_source(
    source: &TaskIdentity,
    source_line_index: usize,
    source_text: &str,
    snapshot: &[(PathBuf, String)],
    daily_path: &Path,
    daily_text: Option<&str>,
    today: NaiveDate,
    tasks_settings: &TasksSettings,
    note_settings: &NoteTaskSettings,
    index: &NoteIndex,
) -> RecoveryPlan {
    let source_tasks = parse_tasks(source_text, tasks_settings);
    let Some(source_task) = source_tasks
        .iter()
        .find(|task| task.line_index == source_line_index)
    else {
        return RecoveryPlan::default();
    };
    if source_task.block_id.as_deref() != Some(source.block_id.as_str()) {
        return RecoveryPlan::default();
    }
    let completed = CompletedTask {
        relative_path: source.path.clone(),
        block_id: source.block_id.clone(),
        task_id: source_task.task_id.clone().unwrap_or_default(),
        text: source_task.description.clone(),
        is_root: true,
    };
    let mut cancelled_projects = BTreeSet::new();
    if is_project_task_id(Some(&source.block_id)) {
        cancelled_projects.insert(source.path.clone());
    }
    let successor = plan_recovery_only(
        SuccessorInput {
            bob_dir: None,
            completed: vec![completed],
            snapshot: snapshot.to_vec(),
            note_texts: Vec::new(),
            pre_day: daily_text.map(str::to_string),
            post_day: daily_text.map(str::to_string),
            closing: None,
            day_relative: daily_path.to_path_buf(),
            today,
            tasks_settings,
            note_settings,
            link_unblocked: false,
            selection: SuccessorSelection::None,
            index,
        },
        &cancelled_projects,
    );
    let changed_files = successor.changed_files.clone();
    let recovered = recovered_tasks(snapshot, &changed_files, tasks_settings);
    RecoveryPlan {
        changed_files,
        recovered,
        successor,
    }
}

fn recovered_tasks(
    before: &[(PathBuf, String)],
    after: &BTreeMap<PathBuf, String>,
    settings: &TasksSettings,
) -> Vec<RecoveredTask> {
    let before_by_path = before
        .iter()
        .map(|(path, text)| (path, text))
        .collect::<BTreeMap<_, _>>();
    let mut recovered = Vec::new();
    for (path, postimage) in after {
        let Some(preimage) = before_by_path.get(path) else {
            continue;
        };
        let before_tasks = parse_tasks(preimage, settings);
        let after_tasks = parse_tasks(postimage, settings);
        for task in before_tasks.iter().filter(|task| task.status == '?') {
            let matching = after_tasks.iter().find(|candidate| {
                candidate.block_id == task.block_id
                    && candidate.line_index == task.line_index
            });
            let Some(matching) = matching else {
                continue;
            };
            if matching.status == '?' {
                continue;
            }
            recovered.push(RecoveredTask {
                path: path.clone(),
                line: matching.line_index + 1,
                block_id: matching.block_id.clone(),
                text: postimage
                    .lines()
                    .nth(matching.line_index)
                    .unwrap_or_default()
                    .to_string(),
            });
        }
    }
    recovered.sort();
    recovered
}
