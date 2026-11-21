//! Carry: a task requirement becomes its own task (`docs/task-requirements.md`
//! §6, `docs/task-split.md` §§1–2).
//!
//! Carry reuses the Task Split engine. This module holds the requirement
//! side of it: which reqs are candidates and which are refused, how the
//! carried branch becomes a canonical task block (the managed back-link in
//! its zone, the NOTES rule through `task_block_layout`), the move map from
//! old descendant lines to new ones, and the [`carry_req`] library entry the
//! close card uses.

use std::path::{Path, PathBuf};

use serde::Serialize;

use super::plan::{
    plan_task_split, SplitError, SplitPlan, SplitRequest, SplitSelection,
    SplitSettings,
};
use crate::native::{
    capture::{line_spans, list_item_body, nearest_shallower_list_item_parent},
    note_tasks::NoteTaskSettings,
    task_block_layout::{
        find_task, insertion_offset, plan_tidy, ChildKind, TaskBlock,
        TidyPolicy,
    },
    task_children::TaskChildKind,
    task_move::VaultSnapshot,
    task_relations,
    task_reqs::{self, EffectiveReqState, ReqExcludeCause, ReqMark, ReqNode},
};

/// The requirement facts a split candidate carries when its line is a req.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ReqCandidate {
    pub(super) depth: usize,
    pub(super) mark: ReqMark,
    pub(super) state: EffectiveReqState,
    /// 1-based sibling ordinals among reqs from the owner down.
    pub(super) path: Vec<usize>,
    /// The structural parent req's line, `None` at depth 1.
    pub(super) parent_req: Option<usize>,
}

impl ReqCandidate {
    pub(super) fn from_node(node: &ReqNode) -> Self {
        Self {
            depth: node.depth,
            mark: node.mark,
            state: node.state,
            path: node.path.clone(),
            parent_req: node.parent,
        }
    }

    /// The close-card row-key suffix (`/req/<path>~<8-hex>`) for this req.
    pub(super) fn row_key_suffix(&self, text: &str) -> String {
        task_reqs::req_row_key("", &self.path, text)
    }
}

/// The requirement half of a split candidate listing (`candidates[].req`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct SplitCandidateReq {
    pub(crate) depth: usize,
    /// The raw checkbox mark (`open`, `dropped`, `carried`, `legacy-met`,
    /// `unknown`).
    pub(crate) mark: &'static str,
    /// The derived state (`pending`, `met`, `moot`, `dropped`, `carried`,
    /// `excluded`).
    pub(crate) state: &'static str,
    /// False when selecting it refuses (dropped, carried, checked, or
    /// excluded by an ancestor).
    pub(crate) carriable: bool,
}

impl From<&ReqCandidate> for SplitCandidateReq {
    fn from(req: &ReqCandidate) -> Self {
        Self {
            depth: req.depth,
            mark: req.mark.name(),
            state: req.state.name(),
            carriable: req_refusal(req).is_none(),
        }
    }
}

/// One line the carry moved: `from` is the 1-based line in the owner's note
/// before the write, `to` the 1-based line in the new task's note after it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub(crate) struct LineMove {
    pub(crate) from: usize,
    pub(crate) to: usize,
}

fn refusal(code: &'static str, message: &str) -> SplitError {
    SplitError {
        code,
        message: message.to_string(),
    }
}

/// Why a selected req cannot be carried: only an open req (`[ ]` or a
/// flagged mark) that no dropped or carried ancestor excludes travels. The
/// owner may already be closed — the post-close card carries under a done
/// task, exactly as it drops there.
pub(super) fn req_refusal(req: &ReqCandidate) -> Option<SplitError> {
    if let EffectiveReqState::Excluded { because, .. } = req.state {
        return Some(refusal(
            "split_req_excluded",
            match because {
                ReqExcludeCause::Dropped => {
                    "Its parent requirement was dropped — restore it first"
                }
                ReqExcludeCause::Carried => {
                    "Its parent requirement was carried into its own task — it goes with it"
                }
            },
        ));
    }
    match req.mark {
        ReqMark::Open | ReqMark::Unknown(_) => None,
        ReqMark::Dropped => Some(refusal(
            "split_not_splittable",
            "Dropped — restore it first",
        )),
        ReqMark::Carried => Some(refusal(
            "split_not_splittable",
            "Carried — open the linked task",
        )),
        ReqMark::LegacyMet => Some(refusal(
            "split_not_splittable",
            "Checked box — split an unchecked note instead",
        )),
    }
}

/// `“{ancestor}” already takes this branch …`: selecting a req together with
/// one of its descendants is refused, because the descendant travels with
/// the ancestor.
pub(super) fn overlap_refusal(ancestor: &str, descendant: &str) -> SplitError {
    SplitError {
        code: "split_overlap",
        message: format!(
            "“{ancestor}” already takes this branch — carry it alone and “{descendant}” goes with it"
        ),
    }
}

/// The carried branch as a canonical task block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct CarriedBlock {
    /// The block's lines, each ending in a newline.
    pub(super) lines: Vec<String>,
    /// `(old, new)` 0-based offsets inside the block: `old` counts from the
    /// req head (the new task line), `new` from the new task line after the
    /// back-link and the NOTES rule placed everything.
    pub(super) moves: Vec<(usize, usize)>,
}

/// Build the new task block for a carried req.
///
/// `task_line` is the new task line and `children` the req's rebased
/// subtree (its direct children are the task's direct children now). The
/// back-link lands in the managed-link zone (`task_block_layout`), and the
/// block is laid out the way `bob task tidy` would: when the branch brought
/// descendant reqs, the req's direct notes file into `NOTES` (created only
/// when a note lands in it), and every child sits in its zone with authored
/// order kept inside each zone.
pub(super) fn carried_block(
    task_line: &str,
    children: &[String],
    back_link: Option<&str>,
    fallback_indent: &str,
    settings: &NoteTaskSettings,
) -> CarriedBlock {
    let mut scratch = with_newline(task_line);
    for child in children {
        scratch.push_str(&with_newline(child));
    }
    let original = scratch.clone();
    if let Some(link) = back_link
        && let Some(task) = find_task(&scratch, 0, settings)
    {
        let slot =
            insertion_offset(&scratch, &task, ChildKind::ManagedLink, settings);
        let has_children =
            !TaskBlock::read(&scratch, &task, &settings.global_filter)
                .children
                .is_empty();
        let indent = if has_children {
            slot.indent.as_str()
        } else {
            fallback_indent
        };
        let line = task_relations::format::format_linked_from_line(
            indent,
            &[(
                link.to_string(),
                Some(task_relations::MENTION_LABEL.to_string()),
            )],
        );
        scratch.insert_str(slot.offset, &with_newline(&line));
    }
    if let Some(task) = find_task(&scratch, 0, settings)
        && let Some(tidy) = plan_tidy(&scratch, &task, &TidyPolicy { settings })
    {
        scratch = tidy.updated;
    }
    CarriedBlock {
        moves: move_offsets(&original, &scratch, settings),
        lines: scratch.split_inclusive('\n').map(str::to_string).collect(),
    }
}

fn with_newline(line: &str) -> String {
    if line.ends_with('\n') {
        line.to_string()
    } else {
        format!("{line}\n")
    }
}

/// Groups of nonblank lines that travel as one through the back-link
/// insertion, the NOTES scoop, and the zone reorder: each direct child
/// subtree of the block's task, except that a drawer splits into its header
/// and one group per entry (scooped notes land beside existing entries).
fn block_units(contents: &str, settings: &NoteTaskSettings) -> Vec<Vec<usize>> {
    let Some(task) = find_task(contents, 0, settings) else {
        return Vec::new();
    };
    let lines = line_spans(contents);
    let block = TaskBlock::read(contents, &task, &settings.global_filter);
    let nonblank = |from: usize, to: usize| -> Vec<usize> {
        (from..=to)
            .filter(|index| !lines[*index].text.trim().is_empty())
            .collect()
    };
    let mut units = Vec::new();
    for child in &block.children {
        if !matches!(child.kind, TaskChildKind::Drawer { .. }) {
            units.push(nonblank(child.line, child.end));
            continue;
        }
        let mut starts = vec![child.line];
        starts.extend((child.line + 1..=child.end).filter(|&index| {
            list_item_body(lines[index].text).is_some()
                && nearest_shallower_list_item_parent(&lines, index)
                    == Some(child.line)
        }));
        for (position, start) in starts.iter().enumerate() {
            let stop =
                starts.get(position + 1).map_or(child.end, |next| next - 1);
            units.push(nonblank(*start, stop));
        }
    }
    units
}

/// Pair each nonblank line of `before`'s child units with its line in
/// `after`. Units keep their text (only leading indentation is rebased), and
/// identical units always share a zone, so the first unclaimed unit with the
/// same text is the one that moved there.
fn move_offsets(
    before: &str,
    after: &str,
    settings: &NoteTaskSettings,
) -> Vec<(usize, usize)> {
    let old_lines = line_spans(before);
    let new_lines = line_spans(after);
    let old_units = block_units(before, settings);
    let new_units = block_units(after, settings);
    let text = |lines: &[crate::native::capture::LineSpan<'_>],
                unit: &[usize]| {
        unit.iter()
            .map(|index| lines[*index].text.trim().to_string())
            .collect::<Vec<_>>()
    };
    let new_texts: Vec<Vec<String>> = new_units
        .iter()
        .map(|unit| text(&new_lines, unit))
        .collect();
    let mut claimed = vec![false; new_units.len()];
    let mut moves = Vec::new();
    for unit in &old_units {
        let wanted = text(&old_lines, unit);
        let Some(found) = (0..new_units.len())
            .find(|index| !claimed[*index] && new_texts[*index] == wanted)
        else {
            continue;
        };
        claimed[found] = true;
        moves
            .extend(unit.iter().copied().zip(new_units[found].iter().copied()));
    }
    moves.sort_unstable();
    moves
}

/// A requirement named for [`carry_req`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) enum ReqRef<'a> {
    /// A close-card row key, `<owner key>/req/<path>~<8-hex>`; the owner
    /// key part is the caller's and only the req suffix is matched.
    RowKey(&'a str),
    /// A 1-based line and the `note_tasks::line_digest` of its text.
    LineDigest { line: usize, digest: &'a str },
}

/// What [`carry_req`] wrote.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) struct CarryOutcome {
    /// The whole split plan: postimages, repaired files, stub, placement.
    pub(crate) plan: SplitPlan,
    /// The created task's note, 1-based line, and block ID.
    pub(crate) path: String,
    pub(crate) line: usize,
    pub(crate) block_id: String,
    /// Old descendant lines (owner's note, before) to new ones (`path`,
    /// after); the close card's jump landing follows a pinned row through
    /// it when an ancestor's carry moved that row.
    pub(crate) moves: Vec<LineMove>,
}

/// Carry one requirement of the task at `owner_line` (1-based) in
/// `owner_path` into its own task. Refuses exactly as `bob task split`
/// does, plus `not_a_req` when the named line is no requirement and
/// `split_stale_ref` when the row key or digest no longer matches.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn carry_req(
    snapshot: &VaultSnapshot,
    owner_path: &Path,
    owner_line: usize,
    target: ReqRef<'_>,
    today: &str,
    settings: &SplitSettings,
    task_settings: &NoteTaskSettings,
) -> Result<CarryOutcome, SplitError> {
    let selection = match target {
        ReqRef::RowKey(key) => SplitSelection::ReqKey(key.to_string()),
        ReqRef::LineDigest { line, digest } => {
            SplitSelection::BulletRef(format!("{line}:{digest}"))
        }
    };
    let request = SplitRequest {
        owner_path: PathBuf::from(owner_path),
        owner_line,
        selection,
    };
    let plan =
        plan_task_split(snapshot, &request, today, settings, task_settings)?;
    let task = plan.tasks.first().cloned().ok_or_else(|| {
        refusal("split_no_selection", "no splittable bullets selected")
    })?;
    if task.req_depth.is_none() {
        return Err(refusal("not_a_req", "This line is not a requirement"));
    }
    Ok(CarryOutcome {
        path: task.path,
        line: task.line,
        block_id: task.block_id,
        moves: task.moves,
        plan,
    })
}
