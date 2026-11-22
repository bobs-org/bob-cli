//! Tidy planning and the NOTES filing both tidy and the req writers share.

use super::{
    super::{
        capture_drawers::{self, Drawer},
        note_tasks::{NoteTask, NoteTaskSettings},
        task_reqs,
    },
    desired_order,
    edit::{
        insert_lines, physical_lines, pull_ranges, rebased,
        restore_final_newline, with_ending, Pulled,
    },
    ensure_notes_drawer, find_task, TaskBlock, NOTES_TITLE,
};

/// What `plan_tidy` may assume about the vault.
#[derive(Debug, Clone, Copy)]
pub(crate) struct TidyPolicy<'a> {
    pub(crate) settings: &'a NoteTaskSettings,
}

/// One planned repair of a task block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TidyPlan {
    /// The whole note after the repair.
    pub(crate) updated: String,
    /// Whole subtrees that changed place (the smallest set whose removal
    /// leaves the rest in order).
    pub(crate) moves: usize,
    /// Loose notes filed into `NOTES`.
    pub(crate) notes_filed: usize,
    /// `NOTES` was created to hold them.
    pub(crate) notes_created: bool,
}

/// Plan the canonical order for one open task block: move whole subtrees
/// into their zones (authored order survives inside a zone) and, for a task
/// that owns reqs, file its loose notes into `NOTES`. Returns `None` when
/// the block is already canonical, and for a task that is not open: history
/// is never rewritten.
///
/// Callers apply one task at a time and rescan, because a repair shifts the
/// lines (and byte extents) of everything below it.
pub(crate) fn plan_tidy(
    contents: &str,
    task: &NoteTask,
    policy: &TidyPolicy<'_>,
) -> Option<TidyPlan> {
    if !task.status_type.is_open() {
        return None;
    }
    let settings = policy.settings;
    let mut updated = contents.to_string();
    let mut notes_filed = 0;
    let mut notes_created = false;
    let block = TaskBlock::read(contents, task, &settings.global_filter);
    if block.owns_reqs()
        && let Some(scoop) =
            scoop_loose_notes(&updated, task.line_index, settings)
    {
        notes_filed = scoop.filed;
        notes_created = scoop.created;
        updated = scoop.updated;
    }
    let refreshed = find_task(&updated, task.line_index, settings)?;
    let mut moves = 0;
    if let Some((reordered, count)) = reorder(&updated, &refreshed, settings) {
        updated = reordered;
        moves = count;
    }
    let updated = restore_final_newline(contents, updated);
    (updated != contents).then_some(TidyPlan {
        updated,
        moves,
        notes_filed,
        notes_created,
    })
}

/// A newly written task block after its reqs and notes have been placed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CanonicalReqBlock {
    /// The standalone block, canonical when the task is open.
    pub(crate) updated: String,
    /// Owner display text (`NoteTask::description`).
    pub(crate) owner: String,
    /// Requirement lines written under this owner, any depth.
    pub(crate) added: usize,
    /// Loose notes scooped into `NOTES`.
    pub(crate) notes_filed: usize,
    /// `NOTES` was created to hold them.
    pub(crate) notes_created: bool,
}

/// Canonicalize a standalone newly-written task block that owns
/// requirements: reqs sit in the reqs zone, loose notes file into `NOTES`
/// (created only when a note lands), other drawers follow, logs last.
///
/// Returns `None` when the block's first line is not an open-or-closed
/// task or the task owns no reqs, so a zero-req capture stays
/// byte-identical. A task that is not open keeps authored order and still
/// reports the reqs it already holds.
pub(crate) fn canonicalize_new_req_block(
    block: &str,
    settings: &NoteTaskSettings,
) -> Option<CanonicalReqBlock> {
    let task = find_task(block, 0, settings)?;
    let read = TaskBlock::read(block, &task, &settings.global_filter);
    if !read.owns_reqs() {
        return None;
    }
    let (updated, notes_filed, notes_created) = if task.status_type.is_open() {
        match plan_tidy(block, &task, &TidyPolicy { settings }) {
            Some(plan) => (plan.updated, plan.notes_filed, plan.notes_created),
            None => (block.to_string(), 0, false),
        }
    } else {
        (block.to_string(), 0, false)
    };
    let task = find_task(&updated, 0, settings)?;
    let added = task_reqs::req_forest(&updated, &task, settings).len();
    (added > 0).then_some(CanonicalReqBlock {
        updated,
        owner: task.description.clone(),
        added,
        notes_filed,
        notes_created,
    })
}

/// Reorder the children into canonical order, returning the new note and
/// the number of subtrees that moved.
fn reorder(
    contents: &str,
    task: &NoteTask,
    settings: &NoteTaskSettings,
) -> Option<(String, usize)> {
    let block = TaskBlock::read(contents, task, &settings.global_filter);
    let order = desired_order(&block);
    if order.iter().enumerate().all(|(slot, &index)| slot == index) {
        return None;
    }
    // `position[i]` is where the i-th current child belongs; the children on
    // a longest increasing run stay put, every other one moves.
    let mut position = vec![0; order.len()];
    for (slot, &index) in order.iter().enumerate() {
        position[index] = slot;
    }
    let moves = order.len() - longest_increasing_run(&position);

    let physical = physical_lines(contents);
    let ending = super::super::capture::document_line_ending(contents);
    let first_child = block.children.first()?.line;
    let mut out = String::with_capacity(contents.len());
    for line in &physical[..first_child] {
        out.push_str(line);
    }
    for &index in &order {
        let child = &block.children[index];
        for line in &physical[child.line..=child.end] {
            out.push_str(&with_ending(line, ending));
        }
    }
    for line in &physical[block.last_line + 1..] {
        out.push_str(line);
    }
    Some((out, moves))
}

fn longest_increasing_run(values: &[usize]) -> usize {
    let mut best = vec![1usize; values.len()];
    let mut longest = 0;
    for index in 0..values.len() {
        for earlier in 0..index {
            if values[earlier] < values[index] {
                best[index] = best[index].max(best[earlier] + 1);
            }
        }
        longest = longest.max(best[index]);
    }
    longest
}

/// The loose direct notes of a req-owning task, moved into `NOTES`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Scoop {
    pub(super) updated: String,
    pub(super) filed: usize,
    pub(super) created: bool,
}

/// File every loose direct note of the task at `task_line` into its `NOTES`
/// drawer (the first-req scoop). Only the task's own notes move, never notes
/// attached to reqs, and nested `#task` subtasks stay where they are.
/// `NOTES` is created only when at least one note moves.
pub(super) fn scoop_loose_notes(
    contents: &str,
    task_line: usize,
    settings: &NoteTaskSettings,
) -> Option<Scoop> {
    let task = find_task(contents, task_line, settings)?;
    let block = TaskBlock::read(contents, &task, &settings.global_filter);
    let lines = super::super::capture::line_spans(contents);
    let ranges: Vec<(usize, usize)> = block
        .children
        .iter()
        .filter(|child| child.is_loose_note())
        .map(|child| (child.line, child.content_end(&lines)))
        .collect();
    if ranges.is_empty() {
        return None;
    }
    let (without, pulled) = pull_ranges(contents, &ranges);
    let filed = file_into_notes(&without, task_line, &pulled, settings)?;
    Some(Scoop {
        updated: filed.updated,
        filed: pulled.len(),
        created: filed.created,
    })
}

/// Lines filed into a `NOTES` drawer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Filed {
    pub(super) updated: String,
    /// The drawer was created by this write.
    pub(super) created: bool,
    /// 0-based line where the first filed line landed.
    pub(super) first_line: usize,
}

/// Append `pulled` ranges to the end of the task's `NOTES` drawer entries
/// (before the drawer's own managed logs), creating the drawer first among
/// the drawers when the task has none.
pub(super) fn file_into_notes(
    contents: &str,
    task_line: usize,
    pulled: &[Pulled],
    settings: &NoteTaskSettings,
) -> Option<Filed> {
    let task = find_task(contents, task_line, settings)?;
    let existing = capture_drawers::drawers(contents, &task)
        .into_iter()
        .find(|drawer| drawer.key == NOTES_TITLE);
    let (staged, drawer, created): (String, Drawer, bool) = match existing {
        Some(drawer) => (contents.to_string(), drawer, false),
        None => ensure_notes_drawer(contents, &task).ok()?,
    };
    let insertion = capture_drawers::drawer_insertion(&staged, &drawer);
    let entries: Vec<String> = pulled
        .iter()
        .flat_map(|range| rebased(range, &insertion.indentation))
        .collect();
    let before = &staged[..insertion.offset];
    let first_line = before.matches('\n').count()
        + usize::from(!before.is_empty() && !before.ends_with('\n'));
    Some(Filed {
        updated: insert_lines(&staged, insertion.offset, &entries),
        created,
        first_line,
    })
}
