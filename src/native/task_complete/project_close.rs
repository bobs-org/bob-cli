//! Project-blocking close rules shared by the capture close paths
//! (`docs/task-dependencies.md` §12.2, PDC vectors in §13.16).
//!
//! When a `^prj` dependent recovers and its project has no remaining
//! reason, every `[?]` resident with no own remaining reason recovers
//! with it as `Recover("project_member")`: recover-only, never
//! successor-linked, outside the successor breaker, with
//! `#prj_is_blocked` stripped. A resident in a still-blocked project
//! stays Blocked (`StillBlockedProject`, PDC1). Every recovery that
//! lands Ready writes `[unblocked::]` through `mark_unblocked`
//! (`docs/freshness.md` §10b UB1 with the WRITE skips).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use chrono::NaiveDate;

use super::super::capture_dependency_tasks::has_hide_tag;
use super::super::freshness::placement::{mark_unblocked, UnblockScope};
use super::super::project_dependencies::{
    blocking_open_dependency_ids, filter_internal_project_ids,
    project_block_reasons, ProjectBlockReasons, ProjectContext,
};
use super::super::projects::edits::normalize_prj_is_blocked_tag;
use super::super::task_status_hooks::{
    canonical_daily_date, FileScan, TaskDependencyState, TaskLine,
};
use super::is_recurring_task_line;

/// Per-file project context plus remaining reasons after this
/// gesture's completions, in `files` order.
pub(crate) type ProjectCloseStates = Vec<(ProjectContext, ProjectBlockReasons)>;

/// Build [`ProjectCloseStates`] for staged files. Openness comes from
/// the staged post-close snapshot, so a prerequisite this gesture
/// completed already reads closed.
pub(crate) fn project_states_for_files(
    files: &[FileScan],
    states: &BTreeMap<(usize, usize), TaskDependencyState>,
    today: NaiveDate,
) -> ProjectCloseStates {
    files
        .iter()
        .enumerate()
        .map(|(file_index, file)| {
            let context = ProjectContext::for_note(file);
            let open_ids = context
                .project_task_index()
                .and_then(|task_index| {
                    states
                        .get(&(file_index, task_index))
                        .map(|state| state.open_dependency_ids.clone())
                })
                .unwrap_or_default();
            let open_ids = filter_internal_project_ids(file, &open_ids);
            let reasons = project_block_reasons(&context, &open_ids, today);
            (context, reasons)
        })
        .collect()
}

/// Whether `task` is a resident whose project still has a reason
/// after this gesture (PDC1): its own reasons are gone but the
/// project's remain, so it stays `[?]` with `still_blocked{reason:
/// project}`. `^prj` itself is never a resident and never matches.
pub(crate) fn project_still_blocks(
    file_index: usize,
    task: &TaskLine,
    project_states: &ProjectCloseStates,
) -> Option<PathBuf> {
    let (context, reasons) = project_states.get(file_index)?;
    if context.is_resident(task) && !reasons.is_empty() {
        Some(context.relative_path.clone())
    } else {
        None
    }
}

/// Whether a resident still has an own blocking reason in the staged
/// snapshot: an open prerequisite or a strictly future `scheduled`
/// date (PDC3). Only residents without one recover with their `^prj`.
pub(crate) fn has_own_remaining_reason(
    task: &TaskLine,
    open_count: usize,
    today: NaiveDate,
) -> bool {
    open_count > 0 || task.scheduled.is_some_and(|date| date > today)
}

/// Freshness scope for one recovered line (`docs/freshness.md` §10b).
/// `is_today` is membership in today's open Pomodoro Task Links,
/// which close planners read off the post-gesture day text; engines
/// without a day text pass false.
pub(crate) fn unblock_scope_for(
    relative_path: &Path,
    line: &str,
    is_today: bool,
) -> UnblockScope {
    UnblockScope {
        recurring: is_recurring_task_line(line),
        hidden: has_hide_tag(line),
        is_daily_note: canonical_daily_date(relative_path).is_some(),
        is_today,
    }
}

/// File indices whose `^prj` is recovering and whose project has no
/// remaining reason: every open resident loses `#prj_is_blocked`
/// (PDC2/PDC3), including residents that keep their own status.
pub(crate) fn recovered_unblocked_project_files(
    verdicts: &[super::successors::DependentVerdict],
    files: &[FileScan],
    project_states: &ProjectCloseStates,
) -> Vec<usize> {
    use super::successors::VerdictKind;
    use crate::native::capture_language::is_reserved_project_task_id;

    let mut files_out = Vec::new();
    for verdict in verdicts {
        if !matches!(verdict.kind, VerdictKind::Recover(_)) {
            continue;
        }
        let Some(task) = files
            .get(verdict.file)
            .and_then(|file| file.tasks.get(verdict.task))
        else {
            continue;
        };
        if !task
            .block_id
            .as_deref()
            .is_some_and(is_reserved_project_task_id)
        {
            continue;
        }
        let Some((_, reasons)) = project_states.get(verdict.file) else {
            continue;
        };
        if reasons.is_empty() {
            files_out.push(verdict.file);
        }
    }
    files_out.sort_unstable();
    files_out.dedup();
    files_out
}

/// When a `^prj` dependent recovers, recover every `[?]`
/// resident of its project that has no own remaining reason as
/// `Recover("project_member")` (PDC2): recover-only, never
/// successor-linked, outside the breaker. A resident that still has
/// an open prerequisite of its own stays Blocked (PDC3). The cascade
/// fires for any `^prj` recovery verdict, whatever reason replaced
/// `project_task` (§12.10 pull-in verdicts included).
pub(crate) fn recover_project_members(
    verdicts: &mut Vec<super::successors::DependentVerdict>,
    files: &[FileScan],
    states: &BTreeMap<(usize, usize), TaskDependencyState>,
    project_states: &ProjectCloseStates,
    today: NaiveDate,
) {
    use super::successors::VerdictKind;
    use crate::native::capture_language::is_reserved_project_task_id;

    let recovering: Vec<(usize, Vec<usize>)> = verdicts
        .iter()
        .filter(|verdict| {
            matches!(verdict.kind, VerdictKind::Recover(_))
                && files
                    .get(verdict.file)
                    .and_then(|file| file.tasks.get(verdict.task))
                    .and_then(|task| task.block_id.as_deref())
                    .is_some_and(is_reserved_project_task_id)
        })
        .map(|verdict| (verdict.file, verdict.preds.clone()))
        .collect();
    for (file_index, preds) in recovering {
        let Some((context, reasons)) = project_states.get(file_index) else {
            continue;
        };
        if !reasons.is_empty() {
            continue;
        }
        let Some(file) = files.get(file_index) else {
            continue;
        };
        for (task_index, task) in file.tasks.iter().enumerate() {
            if !context.is_resident(task) || task.status != '?' {
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
            if let Some(existing) = verdicts.iter_mut().find(|verdict| {
                verdict.file == file_index && verdict.task == task_index
            }) {
                match existing.kind {
                    VerdictKind::StillBlockedWaitsOn
                    | VerdictKind::StillBlockedScheduled
                    | VerdictKind::StillBlockedProject
                    | VerdictKind::Recover("project_task") => {}
                    _ => {
                        existing.kind = VerdictKind::Recover("project_member");
                        existing.anchor = None;
                    }
                }
                continue;
            }
            verdicts.push(super::successors::DependentVerdict {
                file: file_index,
                task: task_index,
                preds: preds.clone(),
                anchor: None,
                kind: VerdictKind::Recover("project_member"),
                open_count,
            });
        }
    }
}

/// Whole-line writes for one Blocked → Ready recovery: strip
/// `#prj_is_blocked` (PDT2; a no-op when the tag is absent), then
/// write `[unblocked:: today]` when the WRITE rule applies (UB1;
/// successor links never reach here, and Next/Pending landings,
/// NEW rows, scheduled ticklers, and out-of-scope rows are
/// skipped). `recovered_status` is the rank after recovery.
pub(crate) fn apply_recovery_writes(
    line: &str,
    today: NaiveDate,
    recovered_status: char,
    scope: &UnblockScope,
) -> String {
    let stripped = normalize_prj_is_blocked_tag(line, false);
    if recovered_status != ' ' {
        return stripped;
    }
    mark_unblocked(&stripped, today, recovered_status, scope)
        .unwrap_or(stripped)
}
