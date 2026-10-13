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

use super::super::capture_dependency_tasks::{
    has_hide_tag, has_prj_is_blocked_tag,
};
use super::super::freshness::placement::{mark_unblocked, UnblockScope};
use super::super::project_dependencies::{
    blocking_open_dependency_ids, filter_internal_project_ids,
    is_project_task_id, project_block_reasons, resolve_project_parent,
    ProjectBlockReasons, ProjectContext,
};
use super::super::projects::edits::normalize_prj_is_blocked_tag;
use super::super::task_status_hooks::{
    canonical_daily_date, FileScan, TaskDependencyState, TaskLine,
};
use super::super::vault_links::NoteIndex;
use super::is_recurring_task_line;

/// Per-file project close state in `files` order: the context plus
/// remaining own reasons after this gesture's completions, plus the
/// inherited holder when the `^prj` line carries `#prj_is_blocked`
/// (the tag proxy, `docs/task-dependencies.md` §13.3). `inherited` is
/// `parent(K)` resolved through the vault index, falling back to the
/// note's own path when the parent is unresolvable, so a tagged
/// project always reads as still blocked on close paths (PDC4/PDC5).
#[derive(Debug, Clone)]
pub(crate) struct ProjectCloseState {
    pub(crate) context: ProjectContext,
    pub(crate) reasons: ProjectBlockReasons,
    pub(crate) inherited: Option<PathBuf>,
}

/// Per-file [`ProjectCloseState`]s in `files` order.
pub(crate) type ProjectCloseStates = Vec<ProjectCloseState>;

/// Build [`ProjectCloseStates`] for staged files. Openness comes from
/// the staged post-close snapshot, so a prerequisite this gesture
/// completed already reads closed.
pub(crate) fn project_states_for_files(
    files: &[FileScan],
    states: &BTreeMap<(usize, usize), TaskDependencyState>,
    today: NaiveDate,
    index: &NoteIndex,
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
            let inherited = context
                .project_task_index()
                .and_then(|task_index| {
                    file.tasks.get(task_index).map(|task| {
                        file.contents
                            .lines()
                            .nth(task.line_index)
                            .unwrap_or_default()
                    })
                })
                .filter(|raw| has_prj_is_blocked_tag(raw))
                .map(|_| {
                    resolve_project_parent(
                        &file.relative_path,
                        &file.contents,
                        index,
                    )
                    .unwrap_or_else(|| file.relative_path.clone())
                });
            ProjectCloseState {
                context,
                reasons,
                inherited,
            }
        })
        .collect()
}

/// Whether `task` is a resident whose project still has a reason
/// after this gesture (PDC1): its own reasons are gone but the
/// project's remain, so it stays `[?]` with `still_blocked{reason:
/// project}`. A resident of a tagged `^prj` project stays Blocked
/// through the inherited holder even when the own reasons are gone
/// (PDC4), naming `parent(K)`. `^prj` itself is never a resident and
/// never matches.
pub(crate) fn project_still_blocks(
    file_index: usize,
    task: &TaskLine,
    project_states: &ProjectCloseStates,
) -> Option<PathBuf> {
    let state = project_states.get(file_index)?;
    if state.context.is_resident(task)
        && (!state.reasons.is_empty() || state.inherited.is_some())
    {
        Some(
            state
                .inherited
                .clone()
                .unwrap_or_else(|| state.context.relative_path.clone()),
        )
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
    let mut files_out = Vec::new();
    for verdict in verdicts {
        if !recovering_prj_verdict(verdict, files) {
            continue;
        }
        let Some(state) = project_states.get(verdict.file) else {
            continue;
        };
        // An inherited (tagged) project never recovers its residents on
        // close paths: descendants keep their tags until reconcile
        // (`close_recovers_subprojects = no`, PDC5/PDC6).
        if state.reasons.is_empty() && state.inherited.is_none() {
            files_out.push(verdict.file);
        }
    }
    files_out.sort_unstable();
    files_out.dedup();
    files_out
}

/// Whether one verdict's `^prj` recovery cascades to its residents: any
/// dependency `Recover`, or a §12.10 pull-in verdict (linked or not) on a
/// `[?]` R that leaves Blocked. A linked pull-in is a `Successor` verdict,
/// which only pull-ins ever produce for `^prj`.
fn recovering_prj_verdict(
    verdict: &super::successors::DependentVerdict,
    files: &[FileScan],
) -> bool {
    use super::successors::VerdictKind;
    let Some(task) = files
        .get(verdict.file)
        .and_then(|file| file.tasks.get(verdict.task))
    else {
        return false;
    };
    if !task
        .block_id
        .as_deref()
        .is_some_and(|id| is_project_task_id(Some(id)))
    {
        return false;
    }
    match verdict.kind {
        VerdictKind::Recover(_) if !verdict.pull_in => true,
        VerdictKind::Recover(_) | VerdictKind::Successor => {
            verdict.pull_in && task.status == '?'
        }
        _ => false,
    }
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
    let recovering: Vec<(usize, Vec<usize>)> = verdicts
        .iter()
        .filter(|verdict| recovering_prj_verdict(verdict, files))
        .map(|verdict| (verdict.file, verdict.preds.clone()))
        .collect();
    for (file_index, preds) in recovering {
        let Some(state) = project_states.get(file_index) else {
            continue;
        };
        // An inherited (tagged) project recovers nothing on close paths
        // (PDC5): its residents keep `[?]` + tag until reconcile.
        if !state.reasons.is_empty() || state.inherited.is_some() {
            continue;
        }
        let context = &state.context;
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
                pull_in: false,
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
