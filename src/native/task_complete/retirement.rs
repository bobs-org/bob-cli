//! Scoped completed-reference retirement for whole-item completion.
//!
//! Built on the reconcile structural planner (`scan_pomodoros`,
//! `plan_structural_changes`, `apply_structural_plan`, and
//! `plan_empty_pomodoro_removals`), scoped to one batch's completed
//! identities. Retirement passes the planner's opt-in dedupe flag, so a
//! carried copy is dropped when the destination entry already links the
//! task (the bob-cli-2l scenario); reconcile keeps passing `false` and
//! stays byte-identical.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use super::super::{
    pomodoro as native_pomodoro,
    task_status_hooks::{
        apply_empty_pomodoro_plan, apply_structural_plan, logical_lines,
        plan_empty_pomodoro_removals, plan_structural_changes, scan_pomodoros,
        RemovedEmptyPomodoro, TasksSettings,
    },
    task_status_hooks::{PomodoroModel, RawReference, ResolvedReference},
};

/// Liveness of one day-file link, as resolved by the caller against the
/// staged batch snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum LinkStatus {
    /// The link resolves to a task completed by this batch.
    Done { path: PathBuf },
    /// The link resolves to an open task with this status character.
    Live { path: PathBuf, status: char },
}

/// One moved bullet's source and destination entries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LedgerMove {
    pub(crate) target: String,
    pub(crate) block_id: String,
    /// 1-based source entry line.
    pub(crate) source_line: usize,
    pub(crate) source_context: String,
    pub(crate) source_open: bool,
    /// 1-based destination entry line.
    pub(crate) destination_line: usize,
    pub(crate) destination_context: String,
    pub(crate) destination_open: bool,
}

/// One deduplicated bullet's source and destination entries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LedgerDedupe {
    pub(crate) target: String,
    pub(crate) block_id: String,
    /// 1-based source entry line.
    pub(crate) source_line: usize,
    pub(crate) source_context: String,
    /// 1-based destination entry line.
    pub(crate) destination_line: usize,
    pub(crate) destination_context: String,
}

/// Result of [`retire_completed_links`].
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct LedgerRetirement {
    pub(crate) text: String,
    pub(crate) changed: bool,
    pub(crate) struck: usize,
    pub(crate) moved: Vec<LedgerMove>,
    pub(crate) deduplicated: Vec<LedgerDedupe>,
    pub(crate) removed_placeholders: Vec<RemovedEmptyPomodoro>,
}

fn entry_details(
    model: &PomodoroModel,
    context: &str,
) -> (usize, String, bool) {
    model
        .entries
        .iter()
        .find(|entry| entry.context == context)
        .map(|entry| (entry.line_index + 1, entry.context.clone(), entry.open))
        .unwrap_or((0, context.to_string(), false))
}

/// Retire one batch's completed references in today's day file.
///
/// `completed` holds the batch's completed identities as
/// (vault-relative path, block ID). `link_statuses` carries the caller's
/// per-link resolution against the staged snapshot: `Done` for links to
/// this batch's completed identities, `Live` for links to open tasks, and
/// omission for anything else. Done tasks completed elsewhere and not yet
/// reconciled are therefore left untouched, and mixed bullets with a live
/// second link keep the liveness the planner needs to hold them back.
pub(crate) fn retire_completed_links(
    day_text: &str,
    completed: &BTreeSet<(PathBuf, String)>,
    link_statuses: &BTreeMap<RawReference, LinkStatus>,
    settings: &TasksSettings,
) -> LedgerRetirement {
    let unchanged = |text: &str| LedgerRetirement {
        text: text.to_string(),
        changed: false,
        struck: 0,
        moved: Vec::new(),
        deduplicated: Vec::new(),
        removed_placeholders: Vec::new(),
    };
    let lines = logical_lines(day_text);
    let Some(section) = native_pomodoro::pomodoros_section_range(&lines) else {
        return unchanged(day_text);
    };
    let model = scan_pomodoros(&lines, section);
    let mut resolved = BTreeMap::<RawReference, ResolvedReference>::new();
    for (reference, status) in link_statuses {
        match status {
            LinkStatus::Done { path }
                if completed
                    .contains(&(path.clone(), reference.block_id.clone())) =>
            {
                resolved.insert(
                    reference.clone(),
                    ResolvedReference {
                        path: path.clone(),
                        statuses: vec!['x'],
                    },
                );
            }
            LinkStatus::Done { .. } => {
                // Completed elsewhere and not yet reconciled: untouched.
            }
            LinkStatus::Live { path, status } => {
                resolved.insert(
                    reference.clone(),
                    ResolvedReference {
                        path: path.clone(),
                        statuses: vec![*status],
                    },
                );
            }
        }
    }
    if resolved.is_empty() {
        return unchanged(day_text);
    }
    let plan = plan_structural_changes(
        &model,
        &resolved,
        &settings.done_statuses,
        &settings.status_types,
        &BTreeSet::new(),
        true,
    );
    let updated = apply_structural_plan(day_text, &model, &plan);
    let empty_plan = plan_empty_pomodoro_removals(&updated, &model);
    let text = apply_empty_pomodoro_plan(&updated, &empty_plan);
    let moved = plan
        .moved
        .iter()
        .map(|item| {
            let (source_line, source_context, source_open) =
                entry_details(&model, &item.source_pomodoro);
            let (destination_line, destination_context, destination_open) =
                entry_details(&model, &item.destination_pomodoro);
            LedgerMove {
                target: item.target.clone(),
                block_id: item.block_id.clone(),
                source_line,
                source_context,
                source_open,
                destination_line,
                destination_context,
                destination_open,
            }
        })
        .collect();
    let deduplicated = plan
        .deduplicated
        .iter()
        .map(|item| {
            let (source_line, source_context, _) =
                entry_details(&model, &item.source_pomodoro);
            let (destination_line, destination_context, _) =
                entry_details(&model, &item.destination_pomodoro);
            LedgerDedupe {
                target: item.target.clone(),
                block_id: item.block_id.clone(),
                source_line,
                source_context,
                destination_line,
                destination_context,
            }
        })
        .collect();
    LedgerRetirement {
        changed: text != day_text,
        text,
        struck: plan.struck.len(),
        moved,
        deduplicated,
        removed_placeholders: empty_plan.removed,
    }
}
