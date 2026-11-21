//! Recursive task-requirement ("req") recognizer.
//!
//! A requirement is a Markdown list item that meets all of these:
//!
//! 1. Its body starts with a checkbox (`[c] `).
//! 2. It does not carry the global filter token (`#task`), matched as a
//!    whole token the way [`crate::native::note_tasks`] matches it.
//! 3. Its structural parent — the nearest shallower list item — is a real
//!    `#task` line or another req.
//! 4. It is outside code fences (including fences nested in list items) and
//!    outside quote lines.
//!
//! Following structural parents up the chain reaches exactly one real task,
//! the req's owner. Depth is 1 for a direct child of the owner, plus 1 per
//! req ancestor. Depth is computed, never stored.
//!
//! State is derived, never written: [`EffectiveReqState`] folds the raw
//! [`ReqMark`] with the owner's open/done/cancelled status plus the ancestor
//! rule (a req under a dropped or carried ancestor is [`Excluded`][EffectiveReqState::Excluded]).
//! No Bob writer ever writes req completion; only drop (`[-]`), restore
//! (`[ ]`), and carry (`[>]`) write to a req line.
//!
//! This module is pure: no filesystem, no clock. Later epic phases build the
//! layout module, writers, capture grammar, and close-card selection on top
//! of it. The shared RQ vectors live in `tests/fixtures/task_reqs/`.

use std::collections::{BTreeMap, BTreeSet};

use sha2::{Digest, Sha256};

use super::{
    capture::{
        line_spans, list_item_body, nearest_shallower_list_item_parent,
        parse_managed_task_log_marker,
    },
    capture_drawers, collect_done, markdown,
    note_tasks::{self, NoteTask, NoteTaskSettings},
    task_dependencies, task_relations,
};

/// Safety bound for req nesting. Depth above this is reported with
/// [`ReqNode::depth_exceeded`] and refused by writers; nothing is flattened
/// or dropped silently.
pub(crate) const MAX_REQ_DEPTH: usize = 16;

/// The raw checkbox mark on a req line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReqMark {
    /// `[ ]`: live scope.
    Open,
    /// `[-]`: dropped, with an optional Cancel Log child.
    Dropped,
    /// `[>]`: carried into its own task (`[[#^id|Alias]]` stub).
    Carried,
    /// `[x]` / `[X]`: hand-written or legacy tick. Met for itself only;
    /// never written by Bob.
    LegacyMet,
    /// Any other mark (for example `[?]`, `[*]`): pending and flagged, never
    /// rewritten by Bob.
    Unknown(char),
}

impl ReqMark {
    /// Short stable name used by vectors and later JSON surfaces.
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Dropped => "dropped",
            Self::Carried => "carried",
            Self::LegacyMet => "legacy-met",
            Self::Unknown(_) => "unknown",
        }
    }

    /// The checkbox character that spells this mark (`?` stands in for any
    /// unknown mark).
    pub(crate) fn mark_char(self) -> char {
        match self {
            Self::Open => ' ',
            Self::Dropped => '-',
            Self::Carried => '>',
            Self::LegacyMet => 'x',
            Self::Unknown(_) => '?',
        }
    }
}

/// Why an ancestor excludes a req's scope.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReqExcludeCause {
    Dropped,
    Carried,
}

/// The derived state of a req: raw mark folded with the owner's lifecycle
/// and the ancestor rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EffectiveReqState {
    /// Open req under an open owner: part of the task's live scope.
    Pending,
    /// Open req under a done owner, or a hand-ticked req: met with its task.
    Met,
    /// Open req under a cancelled owner: moot with its task.
    Moot,
    /// Dropped on its own line (`[-]`), under any owner.
    Dropped,
    /// Carried into its own task (`[>]`), under any owner.
    Carried,
    /// A req whose ancestor req was dropped or carried. `by` is the nearest
    /// excluding ancestor's line index; `because` names the root cause.
    /// Excluded reqs are never met. Bytes stay unchanged.
    Excluded { by: usize, because: ReqExcludeCause },
}

impl EffectiveReqState {
    /// Short stable name used by vectors and later JSON surfaces.
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Met => "met",
            Self::Moot => "moot",
            Self::Dropped => "dropped",
            Self::Carried => "carried",
            Self::Excluded { .. } => "excluded",
        }
    }
}

/// One recognized requirement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReqNode {
    /// 0-based line index of the req's head line.
    pub(crate) line_index: usize,
    /// 1 for a direct child of the owner, plus 1 per req ancestor.
    pub(crate) depth: usize,
    /// Structural parent line: the owner task line for depth 1, else the
    /// parent req's line.
    pub(crate) parent: Option<usize>,
    /// 0-based line index of the owning `#task` line.
    pub(crate) owner_line: usize,
    /// Raw checkbox mark.
    pub(crate) mark: ReqMark,
    /// Derived state.
    pub(crate) state: EffectiveReqState,
    /// Clean text: checkbox and trailing `^anchor` removed, trimmed.
    pub(crate) text: String,
    /// Trailing `^anchor` when the head line carries one.
    pub(crate) anchor: Option<String>,
    /// Last 0-based line index of the req's subtree (descendant reqs plus
    /// attached notes), inclusive.
    pub(crate) subtree_end: usize,
    /// Contiguous 0-based line ranges of the req's directly attached notes
    /// (descendant req subtrees excluded), as `(start, end)` inclusive.
    pub(crate) note_ranges: Vec<(usize, usize)>,
    /// 1-based sibling ordinals among reqs from the owner down
    /// (for example `2.1.3`).
    pub(crate) path: Vec<usize>,
    /// True when [`depth`][Self::depth] exceeds [`MAX_REQ_DEPTH`].
    pub(crate) depth_exceeded: bool,
}

/// Owner lifecycle as seen by the state model.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OwnerStatus {
    Open,
    Done,
    Cancelled,
}

impl OwnerStatus {
    fn of(task: &NoteTask) -> Self {
        use super::task_status_hooks::TaskStatusType;
        match task.status_type {
            TaskStatusType::Done => Self::Done,
            TaskStatusType::Cancelled => Self::Cancelled,
            _ => Self::Open,
        }
    }
}

/// Parse a req checkbox head: `[` + one mark char + `]` + whitespace.
/// Returns the mark and the text after the marker.
fn parse_req_marker(body: &str) -> Option<(ReqMark, &str)> {
    let after_open = body.strip_prefix('[')?;
    let mut chars = after_open.chars();
    let mark_char = chars.next()?;
    let after_mark = &after_open[mark_char.len_utf8()..];
    let after_close = after_mark.strip_prefix(']')?;
    let mark = match mark_char {
        ' ' => ReqMark::Open,
        '-' => ReqMark::Dropped,
        '>' => ReqMark::Carried,
        'x' | 'X' => ReqMark::LegacyMet,
        other => ReqMark::Unknown(other),
    };
    if after_close.is_empty() {
        return Some((mark, ""));
    }
    if !after_close
        .chars()
        .next()
        .is_some_and(|next| next == ' ' || next == '\t')
    {
        return None;
    }
    Some((mark, after_close.trim_start_matches([' ', '\t'])))
}

/// True when `rest` (text after the checkbox) carries the global filter as a
/// whole whitespace-separated token.
fn carries_global_filter(rest: &str, global_filter: &str) -> bool {
    !global_filter.is_empty()
        && rest.split_whitespace().any(|token| token == global_filter)
}

/// Line-shape check: a checkbox list item without the global filter token.
/// Position (parent chain, fences, quotes) is decided by the forest builder.
fn req_head_mark(line: &str, global_filter: &str) -> Option<(ReqMark, String)> {
    let body = list_item_body(line)?;
    let (mark, rest) = parse_req_marker(body)?;
    if carries_global_filter(rest, global_filter) {
        return None;
    }
    Some((mark, rest.to_string()))
}

/// Single-line requirement-candidate check for [`crate::native::task_children`]:
/// a checkbox list item that does not carry the global filter token.
pub(crate) fn checkbox_mark_without_filter(
    line: &str,
    global_filter: &str,
) -> Option<ReqMark> {
    req_head_mark(line, global_filter).map(|(mark, _)| mark)
}

/// Lines that can never be a req or a req parent by shape: drawer titles and
/// managed link/log lines.
fn is_structural_container(line: &str) -> bool {
    if let Some(body) = list_item_body(line) {
        let title = body.trim();
        if capture_drawers::is_drawer_title(title) {
            return true;
        }
    }
    if task_relations::parse::is_subprojects_line(line)
        || task_dependencies::is_managed_link_line(line)
        || task_dependencies::blocks::is_cancel_log(line)
        || parse_managed_task_log_marker(line).is_some()
    {
        return true;
    }
    false
}

/// Clean req text: strip the trailing `^anchor` (reused as the carry ID) and
/// trim.
fn clean_req_text(rest: &str) -> (String, Option<String>) {
    let trimmed = rest.trim_end();
    if let Some(anchor) = collect_done::trailing_block_id_in_line(trimmed) {
        let suffix = format!("^{anchor}");
        let without =
            trimmed.strip_suffix(&suffix).unwrap_or(trimmed).trim_end();
        return (without.to_string(), Some(anchor));
    }
    (trimmed.to_string(), None)
}

/// Base (ancestor-free) derived state from raw mark and owner lifecycle.
fn base_state(mark: ReqMark, owner: OwnerStatus) -> EffectiveReqState {
    match mark {
        ReqMark::Open => match owner {
            OwnerStatus::Open => EffectiveReqState::Pending,
            OwnerStatus::Done => EffectiveReqState::Met,
            OwnerStatus::Cancelled => EffectiveReqState::Moot,
        },
        ReqMark::Dropped => EffectiveReqState::Dropped,
        ReqMark::Carried => EffectiveReqState::Carried,
        // Hand-ticked: met for itself in every owner lifecycle. Pending
        // descendants stay pending because Met never excludes.
        ReqMark::LegacyMet => EffectiveReqState::Met,
        ReqMark::Unknown(_) => match owner {
            OwnerStatus::Open => EffectiveReqState::Pending,
            OwnerStatus::Done => EffectiveReqState::Met,
            OwnerStatus::Cancelled => EffectiveReqState::Moot,
        },
    }
}

/// Fold the ancestor rule over a base state: a req under a dropped or
/// carried ancestor is excluded by the nearest such ancestor. A req
/// dropped or carried on its own keeps that state.
fn apply_ancestor_rule(
    base: EffectiveReqState,
    parent_state: Option<EffectiveReqState>,
    parent_line: Option<usize>,
) -> EffectiveReqState {
    if matches!(
        base,
        EffectiveReqState::Dropped | EffectiveReqState::Carried
    ) {
        return base;
    }
    let Some(parent) = parent_state else {
        return base;
    };
    match parent {
        EffectiveReqState::Dropped => EffectiveReqState::Excluded {
            by: parent_line.expect("req parent line"),
            because: ReqExcludeCause::Dropped,
        },
        EffectiveReqState::Carried => EffectiveReqState::Excluded {
            by: parent_line.expect("req parent line"),
            because: ReqExcludeCause::Carried,
        },
        EffectiveReqState::Excluded { by: _, because } => {
            EffectiveReqState::Excluded {
                by: parent_line.expect("req parent line"),
                because,
            }
        }
        _ => base,
    }
}

/// Stable row identity bound to the owner block's preimage: sibling ordinals
/// from the owner plus the sha256 of the clean text, so duplicate labels in
/// different branches never share a key.
pub(crate) fn req_row_key(
    owner_key: &str,
    path: &[usize],
    clean_text: &str,
) -> String {
    let dotted = path
        .iter()
        .map(usize::to_string)
        .collect::<Vec<_>>()
        .join(".");
    let digest = Sha256::digest(clean_text.as_bytes());
    let hex = hex::encode(digest);
    format!("{owner_key}/req/{dotted}~{}", &hex[..8])
}

/// Build the req forest for one owner task.
pub(crate) fn req_forest(
    contents: &str,
    task: &NoteTask,
    settings: &NoteTaskSettings,
) -> Vec<ReqNode> {
    forest_for_note(contents, settings)
        .into_iter()
        .filter(|node| node.owner_line == task.line_index)
        .collect()
}

/// Build the req forest for every task in the note.
fn forest_for_note(
    contents: &str,
    settings: &NoteTaskSettings,
) -> Vec<ReqNode> {
    let lines = line_spans(contents);
    if lines.is_empty() {
        return Vec::new();
    }
    // The list-body fence scan that `classify_direct_children` uses: a fence
    // nested in a list item starts after the list marker, so feed the item
    // body to the fence scanner.
    let logical: Vec<&str> = lines
        .iter()
        .map(|line| list_item_body(line.text).unwrap_or(line.text))
        .collect();
    let fenced = markdown::fenced_lines(&logical, 0..logical.len());
    let frontmatter_end = markdown::strictly_closed_frontmatter_end(&logical);

    let scan = note_tasks::scan(contents, settings);
    let tasks: BTreeMap<usize, &NoteTask> = scan
        .tasks()
        .iter()
        .map(|task| (task.line_index, task))
        .collect();

    // Owner line for every line inside a task block, following structural
    // parents up to the nearest enclosing task.
    let mut nodes: Vec<ReqNode> = Vec::new();
    let mut node_by_line: BTreeMap<usize, usize> = BTreeMap::new();

    // Owner extents, innermost first, so nested subtasks win their own reqs.
    let mut owners: Vec<&NoteTask> = tasks.values().copied().collect();
    owners.sort_by_key(|task| {
        (std::cmp::Reverse(task.line_index), task.block_end)
    });

    for owner in owners {
        let status = OwnerStatus::of(owner);
        let end = owner_block_end(&tasks, &lines, owner.line_index);
        for (line_index, line) in lines
            .iter()
            .enumerate()
            .take(end + 1)
            .skip(owner.line_index + 1)
        {
            if frontmatter_end.is_some_and(|end| line_index <= end)
                || fenced.contains(&line_index)
                || line.text.trim().is_empty()
                || list_item_body(line.text).is_none()
            {
                continue;
            }
            // A nested task starts its own ownership boundary.
            if tasks.contains_key(&line_index) {
                continue;
            }
            let Some((mark, rest)) =
                req_head_mark(line.text, &settings.global_filter)
            else {
                continue;
            };
            if is_structural_container(line.text) {
                continue;
            }
            let parent_line =
                nearest_shallower_list_item_parent(&lines, line_index);
            let Some(parent_line) = parent_line else {
                continue;
            };
            if fenced.contains(&parent_line)
                || is_structural_container(lines[parent_line].text)
            {
                continue;
            }
            let (depth, parent) = if parent_line == owner.line_index {
                (1, None)
            } else if let Some(parent_node) = node_by_line.get(&parent_line) {
                let parent_node = &nodes[*parent_node];
                if parent_node.owner_line != owner.line_index {
                    continue;
                }
                (parent_node.depth + 1, Some(parent_line))
            } else {
                // Under a note, a drawer entry, a managed line, or any other
                // non-req bullet: an ordinary checklist, never a req.
                continue;
            };
            let (text, anchor) = clean_req_text(&rest);
            let base = base_state(mark, status);
            let parent_state =
                parent.map(|line| nodes[node_by_line[&line]].state);
            let state = apply_ancestor_rule(base, parent_state, parent);
            node_by_line.insert(line_index, nodes.len());
            nodes.push(ReqNode {
                line_index,
                depth,
                parent,
                owner_line: owner.line_index,
                mark,
                state,
                text,
                anchor,
                subtree_end: line_index,
                note_ranges: Vec::new(),
                path: Vec::new(),
                depth_exceeded: depth > MAX_REQ_DEPTH,
            });
        }
    }

    // Sibling ordinals among reqs sharing one owner and structural parent.
    let mut sibling_ordinal: BTreeMap<(usize, usize), usize> = BTreeMap::new();
    let mut parent_key: BTreeMap<(usize, usize), Vec<usize>> = BTreeMap::new();
    let mut order: Vec<usize> = (0..nodes.len()).collect();
    order.sort_by_key(|index| {
        (nodes[*index].owner_line, nodes[*index].line_index)
    });
    for index in order {
        let owner = nodes[index].owner_line;
        let head = nodes[index].line_index;
        let key = (owner, nodes[index].parent.unwrap_or(usize::MAX));
        let ordinal = sibling_ordinal.entry(key).or_insert(0);
        *ordinal += 1;
        let mut path = parent_key.get(&key).cloned().unwrap_or_default();
        path.push(*ordinal);
        parent_key.insert((owner, head), path.clone());
        nodes[index].path = path;
    }

    // Subtree ends and direct note ranges. A line belongs to the nearest req
    // whose ancestor chain reaches it without crossing another req or a
    // nested task; descendant req subtrees are excluded from note ranges.
    let mut chains: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for node in &nodes {
        let mut chain = vec![node.line_index];
        let mut cursor = node.parent;
        while let Some(line) = cursor {
            chain.push(line);
            cursor = nodes
                .get(node_by_line[&line])
                .and_then(|parent| parent.parent);
        }
        chains.insert(node.line_index, chain);
    }
    // First pass: subtree ends, so the note pass can exclude descendant
    // req subtrees regardless of document order.
    let mut all_notes: Vec<Vec<usize>> = Vec::with_capacity(nodes.len());
    for index in 0..nodes.len() {
        let line_index = nodes[index].line_index;
        let owner_line = nodes[index].owner_line;
        let mut end = line_index;
        let mut note_lines: Vec<usize> = Vec::new();
        let owner_end = owner_block_end(&tasks, &lines, owner_line);
        let head_indent = indent_len(lines[line_index].text);
        let mut cursor = line_index + 1;
        while cursor <= owner_end {
            let text = lines[cursor].text;
            let is_list = list_item_body(text).is_some();
            if is_list && indent_len(text) <= head_indent {
                break;
            }
            if tasks.contains_key(&cursor) {
                cursor = skip_task_block(&tasks, &lines, cursor);
                continue;
            }
            if fenced.contains(&cursor) {
                cursor += 1;
                continue;
            }
            if chain_reaches(&lines, &tasks, &fenced, cursor, line_index) {
                end = cursor;
                // Managed lines, drawers, and their nested content travel
                // with the req but are never its notes.
                if !node_by_line.contains_key(&cursor)
                    && is_list
                    && !is_structural_container(text)
                    && !chain_crosses_container(
                        &lines, &fenced, cursor, line_index,
                    )
                    && req_head_mark(text, &settings.global_filter).is_none()
                {
                    note_lines.push(cursor);
                }
            }
            cursor += 1;
        }
        nodes[index].subtree_end = end;
        all_notes.push(note_lines);
    }

    // Second pass: direct note ranges.
    for index in 0..nodes.len() {
        let line_index = nodes[index].line_index;
        let owner_line = nodes[index].owner_line;
        let note_lines = std::mem::take(&mut all_notes[index]);
        // Keep only direct notes: drop lines inside a descendant req's
        // subtree.
        let child_subtrees: Vec<(usize, usize)> = nodes
            .iter()
            .filter(|other| {
                other.owner_line == owner_line
                    && other.line_index != line_index
                    && chains
                        .get(&other.line_index)
                        .is_some_and(|chain| chain.contains(&line_index))
            })
            .map(|other| (other.line_index, other.subtree_end))
            .collect();
        let direct: Vec<usize> = note_lines
            .into_iter()
            .filter(|line| {
                !child_subtrees
                    .iter()
                    .any(|(start, end)| *line >= *start && *line <= *end)
            })
            .collect();
        nodes[index].note_ranges = contiguous_ranges(&direct);
    }

    nodes.sort_by_key(|node| (node.owner_line, node.line_index));
    nodes
}

/// Last line index inside `task`'s block (`block_end` is a byte offset).
fn block_last_line(
    lines: &[super::capture::LineSpan<'_>],
    task: &NoteTask,
) -> usize {
    lines
        .iter()
        .position(|line| line.end > task.block_end)
        .map(|index| index.saturating_sub(1))
        .unwrap_or_else(|| lines.len().saturating_sub(1))
}

fn owner_block_end(
    tasks: &BTreeMap<usize, &NoteTask>,
    lines: &[super::capture::LineSpan<'_>],
    owner_line: usize,
) -> usize {
    tasks
        .get(&owner_line)
        .map(|task| block_last_line(lines, task))
        .unwrap_or_else(|| lines.len().saturating_sub(1))
}

fn skip_task_block(
    tasks: &BTreeMap<usize, &NoteTask>,
    lines: &[super::capture::LineSpan<'_>],
    line_index: usize,
) -> usize {
    tasks
        .get(&line_index)
        .map(|task| block_last_line(lines, task) + 1)
        .unwrap_or(line_index + 1)
}

fn indent_len(text: &str) -> usize {
    text.bytes()
        .take_while(|byte| *byte == b' ' || *byte == b'\t')
        .count()
}

/// True when `line_index`'s structural chain reaches `target` without
/// crossing a task boundary or a fence.
fn chain_reaches(
    lines: &[super::capture::LineSpan<'_>],
    tasks: &BTreeMap<usize, &NoteTask>,
    fenced: &BTreeSet<usize>,
    mut line_index: usize,
    target: usize,
) -> bool {
    loop {
        if line_index == target {
            return true;
        }
        if tasks.contains_key(&line_index) || fenced.contains(&line_index) {
            return false;
        }
        match nearest_shallower_list_item_parent(lines, line_index) {
            Some(parent) => line_index = parent,
            None => return false,
        }
    }
}

/// True when the structural chain from `line_index` up to (excluding)
/// `target` crosses a drawer title or managed line.
fn chain_crosses_container(
    lines: &[super::capture::LineSpan<'_>],
    fenced: &BTreeSet<usize>,
    mut line_index: usize,
    target: usize,
) -> bool {
    loop {
        if line_index == target {
            return false;
        }
        if fenced.contains(&line_index) {
            return false;
        }
        match nearest_shallower_list_item_parent(lines, line_index) {
            Some(parent) => {
                if parent != target
                    && is_structural_container(lines[parent].text)
                {
                    return true;
                }
                line_index = parent;
            }
            None => return false,
        }
    }
}

fn contiguous_ranges(sorted: &[usize]) -> Vec<(usize, usize)> {
    let mut ranges = Vec::new();
    let mut start: Option<usize> = None;
    let mut prev = 0;
    for line in sorted {
        match start {
            None => {
                start = Some(*line);
                prev = *line;
            }
            Some(first) => {
                if *line == prev + 1 {
                    prev = *line;
                } else {
                    ranges.push((first, prev));
                    start = Some(*line);
                    prev = *line;
                }
            }
        }
    }
    if let Some(first) = start {
        ranges.push((first, prev));
    }
    ranges
}

/// The owning task line for `line_index`: the line itself when it is a task,
/// else the owner of the req chain above it, else `None`.
pub(crate) fn owner_of_line(
    contents: &str,
    line_index: usize,
    settings: &NoteTaskSettings,
) -> Option<usize> {
    let scan = note_tasks::scan(contents, settings);
    if scan
        .tasks()
        .iter()
        .any(|task| task.line_index == line_index)
    {
        return Some(line_index);
    }
    forest_for_note(contents, settings)
        .into_iter()
        .find(|node| node.line_index == line_index)
        .map(|node| node.owner_line)
}

/// True when `line_index` is a recognized requirement head line.
pub(crate) fn is_req_line(
    contents: &str,
    line_index: usize,
    settings: &NoteTaskSettings,
) -> bool {
    forest_for_note(contents, settings)
        .iter()
        .any(|node| node.line_index == line_index)
}

/// Every recognized requirement in the note, ordered by owner then document
/// position. Writers that need more than one lookup (the node, its
/// descendants, its siblings) build the forest once through this entry.
pub(crate) fn note_req_forest(
    contents: &str,
    settings: &NoteTaskSettings,
) -> Vec<ReqNode> {
    forest_for_note(contents, settings)
}

/// The hash run drawn in place of a req's checkbox: `⌗⌗` at depth 1, one
/// more hash per level, capped at four so it fills the four cells of `[ ] `.
pub(crate) fn req_run(depth: usize) -> &'static str {
    match depth {
        0 | 1 => "⌗⌗",
        2 => "⌗⌗⌗",
        _ => "⌗⌗⌗⌗",
    }
}

/// 1-based sibling-ordinal path (`2.1.3`) for a req node.
pub(crate) fn req_path(node: &ReqNode) -> String {
    node.path
        .iter()
        .map(usize::to_string)
        .collect::<Vec<_>>()
        .join(".")
}

#[cfg(test)]
mod tests;
