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
//! text for dependent candidates of C, and trigger (c) resolves each closed
//! `^prj`'s `parent:` to N and reads N's staged text with K's marker entry
//! overridden closed (one resolution plus the caller's one read of N, only
//! when C contains a closed `^prj` whose note has `parent:`). The vault-wide
//! dependents snapshot is only needed when [`inspect_pull_ins`] says so.
//! Verdicts follow the Rule steps 1–9; step 3 reads a tagged R as an
//! inherited reason through the tag proxy (`docs/task-dependencies.md`
//! §12.10 Rule, §13.3).

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use chrono::NaiveDate;

use super::super::capture_task_toggle::set_task_line_status;
use super::super::project_dependencies::{
    is_project_task_id, resolve_project_parent,
};
use super::super::projects::edits::normalize_task_hide_tag;
use super::super::projects::{
    project_surface, project_surface_with_closed_child, ProjectSurface,
};
use super::super::task_status_hooks::{
    parse_tasks, FileScan, TaskLine, TasksSettings,
};
use super::super::vault_links::NoteIndex;
use super::successors::{
    is_inbox_note, raw_task_line, successor_link_text, successor_new_status,
    CompletedTask, DayView, DependentVerdict, PlacedSuccessor, PredecessorRef,
    ResolvedClosingTarget, SlotAnchor, StillBlockedRow, SuccessorLinkReport,
    SuccessorPlan, UnblockedRow,
};

/// Trigger value every pull-in row reports (§12.10 Reporting).
pub(crate) const PULL_IN_TRIGGER: &str = "project_empty";

/// Trigger value every existing dependency row reports.
pub(crate) const DEPENDENCY_TRIGGER: &str = "dependency";

/// The capture-side cost gate discovered from the staged close texts.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct PullInGate {
    pub(crate) has_candidates: bool,
    pub(crate) needs_dependents: bool,
    /// Resolved parent notes of closed `^prj` tasks in C that are not in
    /// the staged texts yet (`docs/task-dependencies.md` §12.10 trigger
    /// (c)). The caller loads each through its staged planner, appends it
    /// to the staged note texts, and re-inspects. Empty unless C closed a
    /// `^prj` whose note has a resolvable `parent:`, so every other close
    /// pays nothing.
    pub(crate) parent_notes: Vec<PathBuf>,
}

/// One pull-in candidate R with the C tasks that triggered it.
#[derive(Debug, Clone)]
pub(crate) struct PullInCandidate {
    pub(crate) file: usize,
    pub(crate) task: usize,
    pub(crate) preds: Vec<usize>,
}

/// Detect trigger-(a) candidates from staged C-note texts and decide
/// whether the vault-wide dependents snapshot is required (§12.10 Gate):
/// some task in C carries `[id::]`, a candidate R carries `[dependsOn::]`,
/// or a candidate R is `[?]` while some open resident of N carries
/// `[dependsOn::]` (the §12.2 PROJECT RECOVERY cascade needs vault-wide
/// openness). Trigger (b) necessarily has an `[id::]` predecessor, so it is
/// covered by the same gate even though it is discovered in that snapshot.
/// Byte prefilter for trigger (c) loading (`docs/task-dependencies.md`
/// §12.10 Trigger): true when C closed a `^prj` whose staged text mentions
/// `parent:`. Resolving the parent still needs the vault index, but this
/// check costs no reads and no catalog walk, so ordinary closes never pay
/// for trigger (c). A prose or body `parent:` only over-reports (the
/// resolver still says no); a real scalar `parent:` always contains the
/// substring, so nothing is missed.
pub(crate) fn closed_prj_may_have_parent(
    completed: &[CompletedTask],
    note_texts: &[(PathBuf, String)],
) -> bool {
    completed.iter().any(|task| {
        is_project_task_id(Some(task.block_id.as_str()))
            && note_texts.iter().any(|(path, contents)| {
                path == &task.relative_path && contents.contains("parent:")
            })
    })
}

pub(crate) fn inspect_pull_ins(
    completed: &[CompletedTask],
    note_texts: &[(PathBuf, String)],
    tasks_settings: &TasksSettings,
    today: NaiveDate,
    index: Option<&NoteIndex>,
) -> PullInGate {
    let has_completed_ids =
        completed.iter().any(|task| !task.task_id.is_empty());
    if note_texts.is_empty() {
        return PullInGate {
            has_candidates: false,
            needs_dependents: has_completed_ids,
            parent_notes: closed_prj_parent_notes(completed, note_texts, index),
        };
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
    let candidates =
        detect_pull_ins(completed, &files, &surfaces, &staged, index, today);
    let needs_candidate_dependencies = candidates.iter().any(|candidate| {
        files[candidate.file].tasks[candidate.task]
            .depends_on
            .iter()
            .any(|dependency| !dependency.is_empty())
    });
    let needs_stale_blocked_cascade = candidates.iter().any(|candidate| {
        let file = &files[candidate.file];
        if file.tasks[candidate.task].status != '?' {
            return false;
        }
        file.tasks.iter().enumerate().any(|(index, task)| {
            index != candidate.task
                && task.status_recognized
                && task.status_type.is_open()
                && task
                    .depends_on
                    .iter()
                    .any(|dependency| !dependency.is_empty())
        })
    });
    PullInGate {
        has_candidates: !candidates.is_empty(),
        needs_dependents: has_completed_ids
            || needs_candidate_dependencies
            || needs_stale_blocked_cascade,
        parent_notes: closed_prj_parent_notes(completed, note_texts, index),
    }
}

/// Resolved parent notes for trigger (c) loading: for each closed `^prj`
/// in C whose note text resolves a `parent:`, the parent path unless its
/// text is already staged. Unresolvable, ambiguous, list-form, and self
/// parents yield nothing (PP31); a loaded note that is not a recognised
/// project simply yields no candidate at detection.
fn closed_prj_parent_notes(
    completed: &[CompletedTask],
    note_texts: &[(PathBuf, String)],
    index: Option<&NoteIndex>,
) -> Vec<PathBuf> {
    let Some(index) = index else {
        return Vec::new();
    };
    let staged: BTreeSet<&PathBuf> =
        note_texts.iter().map(|(path, _)| path).collect();
    let mut parents = Vec::new();
    for task in completed {
        if !is_project_task_id(Some(task.block_id.as_str())) {
            continue;
        }
        let Some((_, contents)) = note_texts
            .iter()
            .find(|(path, _)| path == &task.relative_path)
        else {
            continue;
        };
        let Some(parent) =
            resolve_project_parent(&task.relative_path, contents, index)
        else {
            continue;
        };
        if !staged.contains(&parent) && !parents.contains(&parent) {
            parents.push(parent);
        }
    }
    parents
}

/// Find every pull-in candidate R over staged files with their surfaces.
/// Trigger (a) needs the note's staged text from the explicit `staged_notes`
/// lookup (C's own notes); trigger (b) reads R's note text from any staged
/// file; trigger (c) resolves each closed `^prj`'s `parent:` to N and reads
/// N with K's marker entry overridden closed. One candidate per R: all
/// triggers' predecessors union into `preds(R)`.
pub(crate) fn detect_pull_ins(
    completed: &[CompletedTask],
    files: &[FileScan],
    surfaces: &[Option<ProjectSurface>],
    staged_notes: &BTreeSet<PathBuf>,
    index: Option<&NoteIndex>,
    today: NaiveDate,
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
    // Trigger (c): C contains K's `^prj` with `parent(K)` = N, R ∉ C, and
    // EMPTY(N) with N's marker entry for K read as closed. `preds(R)` is
    // K's `^prj`, anchored by its slot, the closing entry, or inherit.
    // N's staged text arrives via the caller's parent-note load; without
    // an index (or without N staged) there is no candidate and every
    // other close is unchanged.
    if let Some(index) = index {
        for (position, closed) in completed.iter().enumerate() {
            if !is_project_task_id(Some(closed.block_id.as_str())) {
                continue;
            }
            let Some(k_file) = files
                .iter()
                .position(|file| file.relative_path == closed.relative_path)
            else {
                continue;
            };
            let Some(parent) = resolve_project_parent(
                &files[k_file].relative_path,
                &files[k_file].contents,
                index,
            ) else {
                continue;
            };
            let Some(n_file) =
                files.iter().position(|file| file.relative_path == parent)
            else {
                continue;
            };
            // R ∉ C: N's own `^prj` was not closed by this gesture.
            if completed.iter().any(|task| {
                task.relative_path == parent
                    && is_project_task_id(Some(task.block_id.as_str()))
            }) {
                continue;
            }
            let closed_child = files[k_file].relative_path.clone();
            let surface = project_surface_with_closed_child(
                &parent,
                &files[n_file].contents,
                today,
                &closed_child,
            );
            let Some(surface) = surface else {
                continue;
            };
            if !surface.is_empty() {
                continue;
            }
            let Some(task_index) =
                open_prj_task_index(&files[n_file], &surface)
            else {
                continue;
            };
            candidates
                .entry((n_file, task_index))
                .or_default()
                .insert(position);
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

/// Pull-in classification after steps 1–5 (`docs/task-dependencies.md`
/// §12.10). Destination (implicit vs earliest_anchor) is attached by
/// [`super::successors::plan_successors`].
#[derive(Debug, Clone)]
pub(crate) enum PullInClass {
    StillBlockedWaitsOn,
    StillBlockedScheduled,
    StillBlockedProject,
    Recover(&'static str),
    /// Offerable: `anchor` is `None` when preds(R) have no §12.3 slot.
    Offer {
        anchor: Option<SlotAnchor>,
    },
}

/// Run the pull-in Rule for one candidate (§12.10 steps 1–5, then the
/// shared kill-switch / implicit-destination / offer steps). Step 3
/// reads a tagged R as an inherited reason through the tag proxy, so a
/// pull-in never offers a Blocked `^prj`.
pub(crate) fn classify_pull_in(
    surface: &ProjectSurface,
    open_count: usize,
    live_before: bool,
    anchor: Option<SlotAnchor>,
    link_unblocked: bool,
    today: NaiveDate,
    inherited: Option<PathBuf>,
) -> PullInClass {
    // Step 1: R has open prerequisites after the gesture.
    if open_count > 0 {
        return PullInClass::StillBlockedWaitsOn;
    }
    // Step 2: SCHED(N) is after today.
    if surface.sched_date.is_some_and(|date| date > today) {
        return PullInClass::StillBlockedScheduled;
    }
    // Step 3: R carries `#prj_is_blocked` (N inherits). R stays
    // byte-identical with `still_blocked{reason: project}`; this
    // applies to every trigger. The row builder reports the holder
    // from the close states, which carry the same tag read.
    if inherited.is_some() {
        return PullInClass::StillBlockedProject;
    }
    // Step 4 removes `#hide`; steps 5–9 pick the row shape.
    if live_before {
        // Step 5: R had a live link before the gesture.
        return PullInClass::Recover("already_planned");
    }
    if !link_unblocked {
        // Kill switch before implicit resolution (SO19).
        return PullInClass::Recover("disabled");
    }
    PullInClass::Offer { anchor }
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

/// Whether a pull-in line write removed `#hide`: sync's own tag boundaries
/// (`normalize_task_hide_tag`, PP22/PP23), so `#hide,`, `#hide.` and
/// `(#hide)` count while `#hidden` and `#hideaway` do not.
pub(crate) fn pull_in_hide_removed(raw: &str) -> bool {
    normalize_task_hide_tag(raw, false, true) != raw
}

fn is_hide_left_boundary(character: char) -> bool {
    character.is_whitespace() || matches!(character, '(' | '[' | '{')
}

fn is_hide_right_boundary(character: char) -> bool {
    character.is_whitespace()
        || matches!(
            character,
            ']' | ')' | '}' | ':' | '.' | ',' | ';' | '!' | '?'
        )
}

fn token_carries_hide(token: &str) -> bool {
    let bytes = token.as_bytes();
    let mut offset = 0;
    while offset + 5 <= bytes.len() {
        let Some(relative) = token[offset..].find("#hide") else {
            break;
        };
        let index = offset + relative;
        let end = index + 5;
        let before = token[..index].chars().next_back();
        let after = token[end..].chars().next();
        if before.is_none_or(is_hide_left_boundary)
            && after.is_none_or(is_hide_right_boundary)
        {
            return true;
        }
        offset = index + 1;
    }
    false
}

/// Row display text for a project row: the post-write description with
/// `#prj`, `#hide` and `#prj_is_blocked` dropped (display only), using
/// sync's tag boundaries for `#hide` so `#hide,`, `#hide.` and `(#hide)`
/// drop while `#hidden` and `#hideaway` never do (PP36).
pub(crate) fn pull_in_display_text(description: &str) -> String {
    description
        .split_whitespace()
        .filter(|token| {
            *token != "#prj"
                && *token != "#prj_is_blocked"
                && !token_carries_hide(token)
        })
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
    pub project_states: &'a super::project_close::ProjectCloseStates,
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
        project_states,
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
        let entry_name = verdict
            .offer
            .as_ref()
            .map(|offer| offer.destination.entry_name.clone())
            .unwrap_or_else(|| match closing_target.as_ref() {
                Some(resolved) => resolved.name.clone(),
                None => {
                    let anchor = verdict
                        .anchor
                        .as_ref()
                        .expect("successor has an anchor");
                    day.entries
                        .get(anchor.entry_index)
                        .map(|entry| entry.name.clone())
                        .unwrap_or_default()
                }
            });
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
                    || (verdict
                        .offer
                        .as_ref()
                        .is_none_or(|offer| !offer.placement_routed)
                        && closing_target
                            .as_ref()
                            .is_some_and(|resolved| resolved.entry_created)),
                next_up: placed_link.next_up,
                block_id_created: minted_id.is_some(),
            }),
            not_linked: None,
            trigger,
            hide_removed,
            offer: verdict.offer.as_ref().map(super::successors::offer_report),
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
                    project: project.or_else(|| verdict.muted_project.clone()),
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
                    project: project.or_else(|| verdict.muted_project.clone()),
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
                let state = project_states.get(verdict.file);
                let scheduled = state
                    .and_then(|state| state.reasons.future_scheduled)
                    .map(|date| date.format("%Y-%m-%d").to_string());
                // A resident of a cascade-blocked project names the
                // holder (PDC4/PP32); otherwise the project note itself.
                let project = state
                    .and_then(|state| state.inherited.clone())
                    .or_else(|| Some(file.relative_path.clone()));
                plan.still_blocked.push(StillBlockedRow {
                    note_path: file.relative_path.clone(),
                    block_id,
                    line,
                    text: still_blocked_text(task, verdict),
                    status_symbol: task.status,
                    reason: "project",
                    waits_on: 0,
                    scheduled,
                    project,
                    trigger: if verdict.pull_in {
                        PULL_IN_TRIGGER
                    } else {
                        DEPENDENCY_TRIGGER
                    },
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
                let live = task.block_id.as_ref().is_some_and(|id| {
                    post_live
                        .live
                        .contains_key(&(file.relative_path.clone(), id.clone()))
                });
                let status_symbol = if task.status == '?' {
                    if live {
                        '*'
                    } else {
                        ' '
                    }
                } else {
                    task.status
                };
                if status_symbol == task.status && verdict.offer.is_none() {
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
                    offer: verdict
                        .offer
                        .as_ref()
                        .map(super::successors::offer_report),
                });
            }
            super::successors::VerdictKind::Successor
            | super::successors::VerdictKind::Offer => {}
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
    if status_symbol == task.status && !hide_removed && verdict.offer.is_none()
    {
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
        offer: verdict.offer.as_ref().map(super::successors::offer_report),
    });
}
