//! Pure project pull-in planner (`docs/task-dependencies.md` §12.10).
//!
//! When a close gesture empties a project note N, the open `^prj` task R is
//! pulled in as a successor link right after the closed task's link, becomes
//! Next, and loses its `#hide` tag in the same write — what `bob projects
//! sync` would do on its next run, plus the link. "Empty" is exactly sync's
//! surface predicate ([`ProjectSurface`]), so sync never re-hides a
//! pulled-in `^prj`.
//!
//! Detection runs over the staged post-gesture texts: trigger (a) reads the
//! staged text of C's own notes (no vault walk), trigger (b) reads R's note
//! text for dependent candidates of C. The vault-wide dependents snapshot is
//! only needed when [`pull_in_needs_dependents`] says so. Verdicts follow
//! the Rule steps 1, 2 and 4–9; step 3 needs no separate check under the
//! line model (`^prj` reports only its own reasons, covered by steps 1–2).

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use chrono::NaiveDate;

use super::super::capture_dependency_tasks::has_hide_tag;
use super::super::capture_task_toggle::set_task_line_status;
use super::super::project_dependencies::is_project_task_id;
use super::super::projects::edits::normalize_task_hide_tag;
use super::super::projects::{project_surface, ProjectSurface};
use super::super::task_status_hooks::{
    parse_tasks, FileScan, TaskLine, TasksSettings,
};
use super::super::vault_links::NoteIndex;
use super::successors::{
    is_inbox_note, successor_link_text, successor_new_status, CompletedTask,
    DayView, DependentVerdict, PlacedSuccessor, PredecessorRef,
    ResolvedClosingTarget, SlotAnchor, StillBlockedRow, SuccessorLinkReport,
    SuccessorPlan, UnblockedRow,
};

/// Trigger value every pull-in row reports (§12.10 Reporting).
pub(crate) const PULL_IN_TRIGGER: &str = "project_empty";

/// Trigger value every existing dependency row reports.
pub(crate) const DEPENDENCY_TRIGGER: &str = "dependency";

/// One pull-in candidate R with the C tasks that triggered it.
#[derive(Debug, Clone)]
pub(crate) struct PullInCandidate {
    pub(crate) file: usize,
    pub(crate) task: usize,
    pub(crate) preds: Vec<usize>,
}

/// Whether the vault-wide dependents snapshot must be built for this
/// gesture (§12.10 Gate): some task in C carries `[id::]` (as today), or a
/// trigger-(a) candidate R carries `[dependsOn::]` (Rule steps 1 and 3 need
/// vault-wide openness in that case). Callers run this before deciding
/// whether to build the snapshot.
pub(crate) fn pull_in_needs_dependents(
    completed: &[CompletedTask],
    note_texts: &[(PathBuf, String)],
    tasks_settings: &TasksSettings,
    today: NaiveDate,
) -> bool {
    if completed.iter().any(|task| !task.task_id.is_empty()) {
        return true;
    }
    if note_texts.is_empty() {
        return false;
    }
    let files: Vec<FileScan> = note_texts
        .iter()
        .map(|(relative_path, contents)| FileScan {
            path: relative_path.clone(),
            relative_path: relative_path.clone(),
            contents: contents.clone(),
            tasks: parse_tasks(contents, tasks_settings),
        })
        .collect();
    let surfaces: Vec<Option<ProjectSurface>> = files
        .iter()
        .map(|file| project_surface(&file.relative_path, &file.contents, today))
        .collect();
    let staged: BTreeSet<PathBuf> =
        note_texts.iter().map(|(path, _)| path.clone()).collect();
    detect_pull_ins(completed, &files, &surfaces, &staged)
        .iter()
        .any(|candidate| {
            files[candidate.file].tasks[candidate.task]
                .depends_on
                .iter()
                .any(|dependency| !dependency.is_empty())
        })
}

/// Find every pull-in candidate R over staged files with their surfaces.
/// Trigger (a) needs the note's staged text from the explicit `staged_notes`
/// lookup (C's own notes); trigger (b) reads R's note text from any staged
/// file. One candidate per R: both triggers' predecessors union into
/// `preds(R)`.
pub(crate) fn detect_pull_ins(
    completed: &[CompletedTask],
    files: &[FileScan],
    surfaces: &[Option<ProjectSurface>],
    staged_notes: &BTreeSet<PathBuf>,
) -> Vec<PullInCandidate> {
    let completed_ids: BTreeSet<&str> = completed
        .iter()
        .map(|task| task.task_id.as_str())
        .filter(|id| !id.is_empty())
        .collect();
    let mut candidates: BTreeMap<(usize, usize), BTreeSet<usize>> =
        BTreeMap::new();
    for (file_index, file) in files.iter().enumerate() {
        let Some(surface) = surfaces
            .get(file_index)
            .and_then(|surface| surface.as_ref())
        else {
            continue;
        };
        if !surface.is_empty() {
            continue;
        }
        let Some(task_index) = open_prj_task_index(file, surface) else {
            continue;
        };
        // R closed by this gesture reads closed in the staged text, so the
        // surface check above already excludes it. Exclude an explicit R
        // close as well, whatever the staged text claims.
        if completed.iter().any(|task| {
            task.relative_path == file.relative_path && task.block_id == "prj"
        }) {
            continue;
        }
        let task = &file.tasks[task_index];
        let mut preds = BTreeSet::new();
        // Trigger (a): C closed a task in N other than R.
        if staged_notes.contains(&file.relative_path) {
            preds.extend(completed.iter().enumerate().filter_map(
                |(position, task)| {
                    (task.relative_path == file.relative_path)
                        .then_some(position)
                },
            ));
        }
        // Trigger (b): R's Depends-On names a task in C.
        if !completed_ids.is_empty() {
            preds.extend(completed.iter().enumerate().filter_map(
                |(position, completed_task)| {
                    (!completed_task.task_id.is_empty()
                        && task.depends_on.contains(&completed_task.task_id))
                    .then_some(position)
                },
            ));
        }
        if !preds.is_empty() {
            candidates.insert((file_index, task_index), preds);
        }
    }
    candidates
        .into_iter()
        .map(|((file, task), preds)| PullInCandidate {
            file,
            task,
            preds: preds.into_iter().collect(),
        })
        .collect()
}

/// Index of the open `^prj` task in a staged file: the hook task on R's
/// surface line when it parses as an open `^prj`, else the unique open
/// `^prj` task when the two parses disagree on an exotic line, else none.
/// The surface line is the planner's R identity: it edits exactly it.
fn open_prj_task_index(
    file: &FileScan,
    surface: &ProjectSurface,
) -> Option<usize> {
    let mut on_line = None;
    let mut count = 0;
    let mut last = None;
    for (index, task) in file.tasks.iter().enumerate() {
        if !(is_project_task_id(task.block_id.as_deref())
            && task.status_recognized
            && task.status_type.is_open())
        {
            continue;
        }
        count += 1;
        last = Some(index);
        if task.line_index + 1 == surface.prj_line_number {
            on_line = Some(index);
        }
    }
    on_line.or(if count == 1 { last } else { None })
}

/// Run the pull-in Rule for one candidate (§12.10 steps 1, 2, 4–9; step 2
/// uses SCHED(N)). Step 3 needs no separate check under the line model.
/// Returns the verdict kind plus the link anchor for a step-9 pull-in.
pub(crate) fn classify_pull_in(
    surface: &ProjectSurface,
    open_count: usize,
    live_before: bool,
    anchor: Option<SlotAnchor>,
    link_unblocked: bool,
    today: NaiveDate,
) -> (super::successors::VerdictKind, Option<SlotAnchor>) {
    use super::successors::VerdictKind;
    // Step 1: R has open prerequisites after the gesture.
    if open_count > 0 {
        return (VerdictKind::StillBlockedWaitsOn, None);
    }
    // Step 2: SCHED(N) is after today.
    if surface.sched_date.is_some_and(|date| date > today) {
        return (VerdictKind::StillBlockedScheduled, None);
    }
    // Step 4 removes `#hide`; steps 5–9 pick the row shape.
    if live_before {
        // Step 5: R had a live link before the gesture.
        return (VerdictKind::Recover("already_planned"), None);
    }
    let Some(anchor) = anchor else {
        // Step 6: no task in preds(R) has an anchor.
        return (VerdictKind::Recover("not_planned_today"), None);
    };
    if !link_unblocked {
        // Step 8: linking disabled.
        return (VerdictKind::Recover("disabled"), None);
    }
    // Step 9: pull R in at the earliest anchor among preds(R).
    (VerdictKind::Successor, Some(anchor))
}

/// Recover rank for a pull-in recover verdict: `?` becomes `*` when R keeps
/// a live link in the post-gesture day text, else ` `; other statuses are
/// unchanged (a Pending `^prj` is never demoted).
pub(crate) fn pull_in_recover_landed(previous: char, live: bool) -> char {
    if previous != '?' {
        return previous;
    }
    if live {
        '*'
    } else {
        ' '
    }
}

/// Status write for a pull-in recover verdict: step 5 (`already_planned`)
/// raises ` ` and `?` to `*` like a link; every other recover verdict takes
/// the recover rank.
pub(crate) fn pull_in_recover_status(
    reason: &'static str,
    previous: char,
    live: bool,
) -> char {
    if reason == "already_planned" {
        return super::successors::successor_new_status(previous);
    }
    pull_in_recover_landed(previous, live)
}

/// Preview of a pull-in line write: the status swap, then the step-4
/// `#hide` removal through sync's own normalizer (PP22 pins the bytes).
/// Callers compute the recover-rank scope off this preview so the hidden
/// WRITE skip never applies to a pulled-in R.
pub(crate) fn pull_in_preview_line(raw: &str, landed: char) -> String {
    let updated = set_task_line_status(raw, landed).unwrap_or(raw.to_string());
    normalize_task_hide_tag(&updated, false, true)
}

/// Whether a pull-in line write removed `#hide`.
pub(crate) fn pull_in_hide_removed(raw: &str) -> bool {
    has_hide_tag(raw)
}

/// Row display text for a project row: the post-write description with
/// `#prj` and `#hide` dropped token-bounded (display only). `#hidden`,
/// `#hideaway` and `#prj_is_blocked` are never touched.
pub(crate) fn pull_in_display_text(description: &str) -> String {
    description
        .split_whitespace()
        .filter(|token| *token != "#prj" && *token != "#hide")
        .collect::<Vec<_>>()
        .join(" ")
}

/// Predecessor refs for one verdict's predecessors.
fn predecessor_refs(
    completed: &[CompletedTask],
    preds: &[usize],
) -> Vec<PredecessorRef> {
    completed
        .iter()
        .enumerate()
        .filter(|(position, _)| preds.contains(position))
        .map(|(_, task)| PredecessorRef {
            note_path: task.relative_path.clone(),
            block_id: task.block_id.clone(),
            text: task.text.clone(),
        })
        .collect()
}

/// Report line for a row about a task living in the day file itself.
fn day_line(
    day_task_lines: &BTreeMap<String, usize>,
    day_relative: &Path,
    note_path: &PathBuf,
    block_id: &str,
    fallback: usize,
) -> usize {
    if *note_path == day_relative && !block_id.is_empty() {
        day_task_lines.get(block_id).copied().unwrap_or(fallback)
    } else {
        fallback
    }
}

/// Everything [`emit_rows`] needs from the planner that its own detection
/// does not compute.
pub(crate) struct RowEmit<'a> {
    pub files: &'a [FileScan],
    pub verdicts: &'a [DependentVerdict],
    pub successor_positions: &'a [usize],
    pub completed: &'a [CompletedTask],
    pub surfaces: &'a [Option<ProjectSurface>],
    pub day: &'a DayView,
    pub closing_target: &'a Option<ResolvedClosingTarget>,
    pub minted: &'a BTreeMap<(PathBuf, usize), String>,
    pub placed: Vec<PlacedSuccessor>,
    pub post_live: &'a DayView,
    pub day_task_lines: &'a BTreeMap<String, usize>,
    pub day_relative: &'a Path,
    pub index: &'a NoteIndex,
}

/// Build the row lists: linked successors always, recovers whose status
/// changed (pull-ins: or whose `#hide` was removed), still-blocked
/// dependents always. `placed` aligns 1:1 with `successor_positions`.
pub(crate) fn emit_rows(plan: &mut SuccessorPlan, ctx: RowEmit<'_>) {
    let RowEmit {
        files,
        verdicts,
        successor_positions,
        completed,
        surfaces,
        day,
        closing_target,
        minted,
        placed,
        post_live,
        day_task_lines,
        day_relative,
        index,
    } = ctx;
    let mut placed_by_order = placed.into_iter();
    for position in successor_positions {
        let verdict = &verdicts[*position];
        let file = &files[verdict.file];
        let task = &file.tasks[verdict.task];
        let placed_link = placed_by_order.next().expect("placed link");
        let minted_id = minted
            .get(&(file.relative_path.clone(), task.line_index))
            .cloned();
        let block_id = task
            .block_id
            .clone()
            .or(minted_id.clone())
            .unwrap_or_default();
        let entry_name = match closing_target.as_ref() {
            Some(resolved) => resolved.name.clone(),
            None => {
                let anchor =
                    verdict.anchor.as_ref().expect("successor has an anchor");
                day.entries
                    .get(anchor.entry_index)
                    .map(|entry| entry.name.clone())
                    .unwrap_or_default()
            }
        };
        let block_link = successor_link_text(
            &file.relative_path,
            &block_id,
            day_relative,
            index,
        );
        let line = day_line(
            day_task_lines,
            day_relative,
            &file.relative_path,
            &block_id,
            task.line_index + 1,
        );
        let (trigger, hide_removed, text) =
            successor_row_fields(file, task, verdict);
        plan.unblocked.push(UnblockedRow {
            note_path: file.relative_path.clone(),
            block_id,
            line,
            text,
            previous_status_symbol: task.status,
            status_symbol: successor_new_status(task.status),
            inbox: is_inbox_note(&file.relative_path, &file.contents, index),
            unblocked_by: predecessor_refs(completed, &verdict.preds),
            link: Some(SuccessorLinkReport {
                entry_name,
                entry_line: placed_link.entry_line,
                line: placed_link.line,
                block_link,
                entry_created: placed_link.entry_created
                    || closing_target
                        .as_ref()
                        .is_some_and(|resolved| resolved.entry_created),
                next_up: placed_link.next_up,
                block_id_created: minted_id.is_some(),
            }),
            not_linked: None,
            trigger,
            hide_removed,
        });
    }
    for verdict in verdicts.iter() {
        let file = &files[verdict.file];
        let task = &file.tasks[verdict.task];
        match verdict.kind {
            super::successors::VerdictKind::StillBlockedWaitsOn => {
                let block_id = task.block_id.clone().unwrap_or_default();
                let line = day_line(
                    day_task_lines,
                    day_relative,
                    &file.relative_path,
                    &block_id,
                    task.line_index + 1,
                );
                let (trigger, project) = waits_row_fields(verdict, file);
                plan.still_blocked.push(StillBlockedRow {
                    note_path: file.relative_path.clone(),
                    block_id,
                    line,
                    text: still_blocked_text(task, verdict),
                    status_symbol: task.status,
                    reason: "waits_on",
                    waits_on: verdict.open_count,
                    scheduled: None,
                    project,
                    trigger,
                });
            }
            super::successors::VerdictKind::StillBlockedScheduled => {
                let block_id = task.block_id.clone().unwrap_or_default();
                let line = day_line(
                    day_task_lines,
                    day_relative,
                    &file.relative_path,
                    &block_id,
                    task.line_index + 1,
                );
                let (trigger, project, scheduled) =
                    scheduled_row_fields(surfaces, verdict, file, task);
                plan.still_blocked.push(StillBlockedRow {
                    note_path: file.relative_path.clone(),
                    block_id,
                    line,
                    text: still_blocked_text(task, verdict),
                    status_symbol: task.status,
                    reason: "scheduled",
                    waits_on: 0,
                    scheduled,
                    project,
                    trigger,
                });
            }
            super::successors::VerdictKind::StillBlockedProject => {
                let block_id = task.block_id.clone().unwrap_or_default();
                let line = day_line(
                    day_task_lines,
                    day_relative,
                    &file.relative_path,
                    &block_id,
                    task.line_index + 1,
                );
                plan.still_blocked.push(StillBlockedRow {
                    note_path: file.relative_path.clone(),
                    block_id,
                    line,
                    text: task.description.clone(),
                    status_symbol: task.status,
                    reason: "project",
                    waits_on: 0,
                    scheduled: None,
                    project: Some(file.relative_path.clone()),
                    trigger: DEPENDENCY_TRIGGER,
                });
            }
            super::successors::VerdictKind::Recover(reason) => {
                if verdict.pull_in {
                    emit_pull_in_recover(
                        plan,
                        completed,
                        post_live,
                        day_task_lines,
                        day_relative,
                        index,
                        verdict,
                        file,
                        task,
                        reason,
                    );
                    continue;
                }
                if task.status != '?' {
                    continue;
                }
                let live = task.block_id.as_ref().is_some_and(|id| {
                    post_live
                        .live
                        .contains_key(&(file.relative_path.clone(), id.clone()))
                });
                let status_symbol = if live { '*' } else { ' ' };
                if status_symbol == task.status {
                    continue;
                }
                let block_id = task.block_id.clone().unwrap_or_default();
                let line = day_line(
                    day_task_lines,
                    day_relative,
                    &file.relative_path,
                    &block_id,
                    task.line_index + 1,
                );
                plan.unblocked.push(UnblockedRow {
                    note_path: file.relative_path.clone(),
                    block_id,
                    line,
                    text: task.description.clone(),
                    previous_status_symbol: task.status,
                    status_symbol,
                    inbox: is_inbox_note(
                        &file.relative_path,
                        &file.contents,
                        index,
                    ),
                    unblocked_by: predecessor_refs(completed, &verdict.preds),
                    link: None,
                    not_linked: Some(reason),
                    trigger: DEPENDENCY_TRIGGER,
                    hide_removed: false,
                });
            }
            super::successors::VerdictKind::Successor => {}
        }
    }
}

/// `trigger`, `hide_removed` and display text for one linked successor row.
fn successor_row_fields(
    file: &FileScan,
    task: &TaskLine,
    verdict: &DependentVerdict,
) -> (&'static str, bool, String) {
    if verdict.pull_in {
        let raw = raw_task_line(&file.contents, task.line_index);
        (
            PULL_IN_TRIGGER,
            pull_in_hide_removed(&raw),
            pull_in_display_text(&task.description),
        )
    } else {
        (DEPENDENCY_TRIGGER, false, task.description.clone())
    }
}

/// `trigger` and `project` for a `waits_on` still-blocked row. A pull-in
/// row names its project note; an existing row says `dependency`.
fn waits_row_fields(
    verdict: &DependentVerdict,
    file: &FileScan,
) -> (&'static str, Option<PathBuf>) {
    if verdict.pull_in {
        (PULL_IN_TRIGGER, Some(file.relative_path.clone()))
    } else {
        (DEPENDENCY_TRIGGER, None)
    }
}

/// Display text for a still-blocked row: pull-ins drop `#prj`/`#hide`.
fn still_blocked_text(task: &TaskLine, verdict: &DependentVerdict) -> String {
    if verdict.pull_in {
        pull_in_display_text(&task.description)
    } else {
        task.description.clone()
    }
}

/// `trigger`, `project` and `scheduled` for a `scheduled` still-blocked
/// row. A pull-in row reports SCHED(N) and its project note (§12.5); an
/// existing row reports the task's own date.
fn scheduled_row_fields(
    surfaces: &[Option<ProjectSurface>],
    verdict: &DependentVerdict,
    file: &FileScan,
    task: &TaskLine,
) -> (&'static str, Option<PathBuf>, Option<String>) {
    if verdict.pull_in {
        let scheduled = surfaces
            .get(verdict.file)
            .and_then(|surface| surface.as_ref())
            .and_then(|surface| surface.sched_raw.clone());
        (PULL_IN_TRIGGER, Some(file.relative_path.clone()), scheduled)
    } else {
        (
            DEPENDENCY_TRIGGER,
            None,
            task.scheduled
                .map(|date| date.format("%Y-%m-%d").to_string()),
        )
    }
}

/// One pull-in recover row (§12.10 ROWS): emitted when the gesture changed
/// R's status, removed `#hide`, or linked R. A surfaced row removed `#hide`
/// without linking or changing status.
#[allow(clippy::too_many_arguments)]
fn emit_pull_in_recover(
    plan: &mut SuccessorPlan,
    completed: &[CompletedTask],
    post_live: &DayView,
    day_task_lines: &BTreeMap<String, usize>,
    day_relative: &Path,
    index: &NoteIndex,
    verdict: &DependentVerdict,
    file: &FileScan,
    task: &TaskLine,
    reason: &'static str,
) {
    let live = task.block_id.as_ref().is_some_and(|id| {
        post_live
            .live
            .contains_key(&(file.relative_path.clone(), id.clone()))
    });
    let status_symbol = pull_in_recover_status(reason, task.status, live);
    let hide_removed =
        pull_in_hide_removed(&raw_task_line(&file.contents, task.line_index));
    if status_symbol == task.status && !hide_removed {
        return;
    }
    let block_id = task.block_id.clone().unwrap_or_default();
    let line = day_line(
        day_task_lines,
        day_relative,
        &file.relative_path,
        &block_id,
        task.line_index + 1,
    );
    plan.unblocked.push(UnblockedRow {
        note_path: file.relative_path.clone(),
        block_id,
        line,
        text: pull_in_display_text(&task.description),
        previous_status_symbol: task.status,
        status_symbol,
        inbox: is_inbox_note(&file.relative_path, &file.contents, index),
        unblocked_by: predecessor_refs(completed, &verdict.preds),
        link: None,
        not_linked: Some(reason),
        trigger: PULL_IN_TRIGGER,
        hide_removed,
    });
}

/// Raw staged line for one task, without its line ending.
fn raw_task_line(contents: &str, line_index: usize) -> String {
    contents
        .lines()
        .nth(line_index)
        .unwrap_or_default()
        .trim_end_matches('\r')
        .to_string()
}
