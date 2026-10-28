//! Pure cancel-label planning (`docs/task-relations.md` §17).

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use chrono::NaiveDate;

use crate::native::{
    note_tasks::NoteTaskSettings, task_status_hooks::TasksSettings,
    vault_links::NoteIndex,
};

pub(crate) mod line;
pub(crate) mod prune;
pub(crate) mod recovery;
pub(crate) mod verdicts;

#[cfg(test)]
mod tests;

pub(crate) use line::CancelLogOutcome;
pub(crate) use prune::{PruneResult, TaskIdentity};
pub(crate) use recovery::RecoveredTask;
use verdicts::{cancel_verdict, CancelSourceVerdict};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DailyReadState {
    Absent,
    Present(String),
    Failed(String),
}

pub(crate) struct TaskCancelInput<'a> {
    pub(crate) source_path: PathBuf,
    pub(crate) source_block_id: Option<String>,
    pub(crate) source_line_index: usize,
    /// Current staged source note bytes, including the relation edit.
    pub(crate) source_text: String,
    pub(crate) source_is_new: bool,
    /// Ordered LINKS fallback reasons, already formatted with their exact
    /// labels and target-link spelling.
    pub(crate) ordered_reasons: Vec<String>,
    pub(crate) today: NaiveDate,
    pub(crate) note_index: &'a NoteIndex,
    pub(crate) tasks_settings: &'a TasksSettings,
    pub(crate) note_settings: &'a NoteTaskSettings,
    pub(crate) daily_path: PathBuf,
    pub(crate) daily_state: DailyReadState,
    /// Relevant, already-staged note snapshots. Source and daily entries
    /// are replaced by the composed postimages before recovery runs.
    pub(crate) dependency_snapshots: Vec<(PathBuf, String)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TaskCancelError {
    Refused(CancelSourceVerdict),
    DailyRead(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TaskCancelPlan {
    pub(crate) verdict: CancelSourceVerdict,
    pub(crate) source_postimage: String,
    pub(crate) daily_postimage: Option<String>,
    pub(crate) cancel_log: CancelLogOutcome,
    pub(crate) previous_status: Option<char>,
    pub(crate) removed_pomodoro_links: usize,
    pub(crate) removed_pomodoro_bullets: usize,
    pub(crate) removed_pomodoro_entries: usize,
    pub(crate) unresolved_pomodoro_links: usize,
    pub(crate) prune: PruneResult,
    pub(crate) recovered: Vec<RecoveredTask>,
    /// Changed note postimages other than the source and daily note. These
    /// share the same final snapshot as `source_postimage` and
    /// `daily_postimage`, including when all three paths overlap.
    pub(crate) recovery_file_postimages: BTreeMap<PathBuf, String>,
    pub(crate) postimages: BTreeMap<PathBuf, String>,
}

pub(crate) fn plan_task_cancel(
    input: TaskCancelInput<'_>,
) -> Result<TaskCancelPlan, TaskCancelError> {
    let source_line = input
        .source_text
        .split_inclusive('\n')
        .nth(input.source_line_index)
        .map(logical_line)
        .unwrap_or_default();
    let verdict =
        cancel_verdict(source_line, input.tasks_settings, input.source_is_new);
    match verdict {
        CancelSourceVerdict::Recurring | CancelSourceVerdict::NewTask => {
            return Err(TaskCancelError::Refused(verdict));
        }
        CancelSourceVerdict::AlreadyClosed { status } => {
            let daily_error = match input.daily_state {
                DailyReadState::Failed(error) => Some(error),
                DailyReadState::Absent | DailyReadState::Present(_) => None,
            };
            if let Some(error) = daily_error {
                return Err(TaskCancelError::DailyRead(error));
            }
            return Ok(TaskCancelPlan {
                verdict,
                source_postimage: input.source_text,
                daily_postimage: None,
                cancel_log: CancelLogOutcome::None,
                previous_status: Some(status),
                removed_pomodoro_links: 0,
                removed_pomodoro_bullets: 0,
                removed_pomodoro_entries: 0,
                unresolved_pomodoro_links: 0,
                prune: prune::prune_daily(
                    &input.daily_path,
                    None,
                    &BTreeSet::new(),
                    input.note_index,
                ),
                recovered: Vec::new(),
                recovery_file_postimages: BTreeMap::new(),
                postimages: BTreeMap::new(),
            });
        }
        CancelSourceVerdict::Cancels { .. } => {}
    }

    let daily_text = match input.daily_state {
        DailyReadState::Absent => None,
        DailyReadState::Present(text) => Some(text),
        DailyReadState::Failed(error) => {
            return Err(TaskCancelError::DailyRead(error));
        }
    };
    let daily_preimage = daily_text.clone();
    let source_edit = line::edit_source(
        &input.source_text,
        input.source_line_index,
        input.today,
        &input.ordered_reasons,
    );
    let daily_base = if input.daily_path == input.source_path {
        Some(source_edit.content.as_str())
    } else {
        daily_text.as_deref()
    };
    let targets = input
        .source_block_id
        .as_ref()
        .map(|block_id| {
            BTreeSet::from([TaskIdentity {
                path: input.source_path.clone(),
                block_id: block_id.clone(),
            }])
        })
        .unwrap_or_default();
    let prune = prune::prune_daily(
        &input.daily_path,
        daily_base,
        &targets,
        input.note_index,
    );

    let mut snapshots = input
        .dependency_snapshots
        .into_iter()
        .collect::<BTreeMap<_, _>>();
    snapshots.insert(input.source_path.clone(), source_edit.content.clone());
    if let Some(daily_postimage) = &prune.content {
        snapshots.insert(input.daily_path.clone(), daily_postimage.clone());
    }
    let snapshot = snapshots.into_iter().collect::<Vec<_>>();
    let source_identity =
        input.source_block_id.as_ref().map(|block_id| TaskIdentity {
            path: input.source_path.clone(),
            block_id: block_id.clone(),
        });
    let recovery = source_identity.as_ref().map_or_else(
        recovery::RecoveryPlan::default,
        |source| {
            recovery::recover_cancelled_source(
                source,
                input.source_line_index,
                &source_edit.content,
                &snapshot,
                &input.daily_path,
                prune.content.as_deref(),
                input.today,
                input.tasks_settings,
                input.note_settings,
                input.note_index,
            )
        },
    );

    let mut final_snapshots = snapshot.into_iter().collect::<BTreeMap<_, _>>();
    for (path, text) in &recovery.changed_files {
        final_snapshots.insert(path.clone(), text.clone());
    }
    let source_postimage = final_snapshots
        .get(&input.source_path)
        .cloned()
        .unwrap_or(source_edit.content);
    let daily_postimage = daily_preimage.as_ref().map(|_| {
        final_snapshots
            .get(&input.daily_path)
            .cloned()
            .unwrap_or_else(|| prune.content.clone().unwrap_or_default())
    });
    let recovery_file_postimages = recovery
        .changed_files
        .iter()
        .filter(|(path, _)| {
            *path != &input.source_path && *path != &input.daily_path
        })
        .map(|(path, text)| (path.clone(), text.clone()))
        .collect();
    let postimages = final_snapshots
        .into_iter()
        .filter(|(path, text)| {
            if path == &input.source_path {
                text != &input.source_text
            } else if path == &input.daily_path {
                daily_preimage.as_ref().is_some_and(|before| before != text)
            } else {
                recovery.changed_files.contains_key(path)
            }
        })
        .collect();

    Ok(TaskCancelPlan {
        verdict,
        source_postimage,
        daily_postimage,
        cancel_log: source_edit.log,
        previous_status: match verdict {
            CancelSourceVerdict::Cancels {
                previous_status, ..
            } => Some(previous_status),
            _ => None,
        },
        removed_pomodoro_links: prune.removed_link_count,
        removed_pomodoro_bullets: prune.removed_bullet_count,
        removed_pomodoro_entries: prune.removed_entry_count,
        unresolved_pomodoro_links: prune.unresolved_count,
        prune,
        recovered: recovery.recovered,
        recovery_file_postimages,
        postimages,
    })
}

fn logical_line(segment: &str) -> &str {
    let without_lf = segment.strip_suffix('\n').unwrap_or(segment);
    without_lf.strip_suffix('\r').unwrap_or(without_lf)
}
