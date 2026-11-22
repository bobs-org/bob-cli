//! Pure requirement line writers: drop, restore, the Ctrl+] note/req toggle,
//! and first-requirement insertion (`docs/task-requirements.md` §2, §3, §5).
//!
//! Each writer takes the whole note, a [`ReqTarget`] naming the line and the
//! exact text it expects there (the preimage), and the vault settings. It
//! returns the whole updated note, or a [`WriteRefusal`] with a stable code
//! and the copy a client shows. No writer ever writes req completion: the
//! only req edits are drop (`[ ]` to `[-]` plus a Cancel Log), restore
//! (`[-]` to `[ ]`), and the toggle between a note and a req.

use chrono::NaiveDate;

use super::{
    super::{
        capture::{
            document_line_ending, leading_spaces_or_tabs_len, line_spans,
            list_item_body, nearest_shallower_list_item_parent,
        },
        capture_drawers,
        note_tasks::{NoteTask, NoteTaskSettings},
        task_children::{self, TaskChildKind},
        task_dependencies::blocks::is_cancel_log,
        task_reqs::{
            self, EffectiveReqState, ReqExcludeCause, ReqMark, ReqNode,
            MAX_REQ_DEPTH,
        },
    },
    edit::{
        bare, insert_lines, physical_lines, pull_ranges, rebased,
        restore_final_newline, with_ending,
    },
    find_task, insertion_offset,
    tidy::{file_into_notes, scoop_loose_notes},
    ChildKind, TaskBlock,
};

/// Written when a task that already keeps a Cancel Log is dropped with no
/// reason: a kept history has no gaps.
const NO_REASON_GIVEN: &str = "🤷 no reason given";
const CANCEL_LOG_MARKER: &str = "❌ **CANCEL LOG**";

/// A refused write: a stable `code` and the copy a client shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WriteRefusal {
    pub(crate) code: &'static str,
    pub(crate) message: String,
}

impl WriteRefusal {
    fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

type Written<T> = Result<T, WriteRefusal>;

/// A line a writer acts on, with the exact text it expects to find there. A
/// moved or edited line refuses as `req_stale` instead of writing elsewhere.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ReqTarget<'a> {
    /// 0-based line index.
    pub(crate) line_index: usize,
    /// The line's text without its ending.
    pub(crate) preimage: &'a str,
}

/// What a drop did to the Cancel Log.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LogOutcome {
    /// No log written (empty reason, none kept).
    None,
    /// A new log became the req's first child.
    Created,
    /// An entry was prepended under the req's existing log.
    Prepended,
}

/// The result of a drop or restore.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReqWrite {
    pub(crate) updated: String,
    /// 0-based head line of the req (it does not move).
    pub(crate) line_index: usize,
    pub(crate) log: LogOutcome,
    /// Descendant reqs that went with it.
    pub(crate) descendants: usize,
}

fn stale(contents: &str, target: ReqTarget<'_>) -> Written<()> {
    let lines = line_spans(contents);
    match lines.get(target.line_index) {
        Some(line) if line.text == target.preimage => Ok(()),
        _ => Err(WriteRefusal::new(
            "req_stale",
            "The line changed — try again",
        )),
    }
}

fn owner_text(
    contents: &str,
    node: &ReqNode,
    settings: &NoteTaskSettings,
) -> String {
    find_task(contents, node.owner_line, settings)
        .map_or_else(|| "the task".to_string(), |task| task.description)
}

fn locate_req(
    contents: &str,
    target: ReqTarget<'_>,
    settings: &NoteTaskSettings,
) -> Written<(ReqNode, Vec<ReqNode>)> {
    stale(contents, target)?;
    let forest = task_reqs::note_req_forest(contents, settings);
    let node = forest
        .iter()
        .find(|node| node.line_index == target.line_index)
        .cloned()
        .ok_or_else(|| {
            WriteRefusal::new("not_a_req", "This line is not a requirement")
        })?;
    Ok((node, forest))
}

fn descendant_count(forest: &[ReqNode], node: &ReqNode) -> usize {
    forest
        .iter()
        .filter(|other| {
            other.owner_line == node.owner_line
                && other.line_index > node.line_index
                && other.line_index <= node.subtree_end
        })
        .count()
}

/// `line` with its checkbox mark replaced (`None` drops the checkbox
/// entirely), keeping everything else byte for byte.
fn rewrite_checkbox(line: &str, mark: Option<char>) -> Option<String> {
    let body = list_item_body(line)?;
    let prefix = &line[..line.len() - body.len()];
    let rest = body.strip_prefix('[')?;
    let mark_char = rest.chars().next()?;
    let after = rest[mark_char.len_utf8()..].strip_prefix(']')?;
    Some(match mark {
        Some(mark) => format!("{prefix}[{mark}]{after}"),
        None => format!("{prefix}{}", after.trim_start_matches([' ', '\t'])),
    })
}

/// Replace the text of one physical line, keeping its own ending.
fn replace_line_text(physical: &str, text: &str) -> String {
    format!("{text}{}", &physical[bare(physical).len()..])
}

/// One nesting unit for children of `line_index`: the req's own indent
/// relative to its structural parent, else a tab.
fn nesting_unit(
    lines: &[super::super::capture::LineSpan<'_>],
    line_index: usize,
) -> String {
    let own = leading_spaces_or_tabs_len(lines[line_index].text);
    nearest_shallower_list_item_parent(lines, line_index)
        .map(|parent| leading_spaces_or_tabs_len(lines[parent].text))
        .filter(|parent| *parent < own)
        .map_or_else(
            || "\t".to_string(),
            |parent| lines[line_index].text[parent..own].to_string(),
        )
}

/// Direct list-item children of a req, in document order.
fn direct_children(
    lines: &[super::super::capture::LineSpan<'_>],
    node: &ReqNode,
) -> Vec<usize> {
    (node.line_index + 1..=node.subtree_end)
        .filter(|&index| {
            list_item_body(lines[index].text).is_some()
                && nearest_shallower_list_item_parent(lines, index)
                    == Some(node.line_index)
        })
        .collect()
}

/// Drop a pending req: `[ ]` becomes `[-]` and the reason lands in a Cancel
/// Log as the req's first child, written the way a task cancel writes it
/// (`*YYYY-MM-DD* — reason`, newest first). An empty reason writes no log
/// unless the req already keeps one; then it gets
/// `*YYYY-MM-DD* — 🤷 no reason given`. The owner may already be closed: the
/// post-close card drops reqs under a done task.
pub(crate) fn drop_req(
    contents: &str,
    target: ReqTarget<'_>,
    date: NaiveDate,
    reason: &str,
    settings: &NoteTaskSettings,
) -> Written<ReqWrite> {
    let (node, forest) = locate_req(contents, target, settings)?;
    if let EffectiveReqState::Excluded { because, .. } = node.state {
        return Err(match because {
            ReqExcludeCause::Dropped => WriteRefusal::new(
                "req_excluded",
                "Its parent requirement was dropped — restore it first",
            ),
            ReqExcludeCause::Carried => WriteRefusal::new(
                "req_excluded",
                "Its parent requirement was carried into its own task — it goes with it",
            ),
        });
    }
    match node.mark {
        ReqMark::Open | ReqMark::Unknown(_) => {}
        ReqMark::Dropped => {
            return Err(WriteRefusal::new("req_dropped", "Already dropped"));
        }
        ReqMark::Carried => {
            return Err(WriteRefusal::new(
                "req_carried",
                "Carried — open the linked task",
            ));
        }
        ReqMark::LegacyMet => {
            return Err(WriteRefusal::new("req_met", "Already met"));
        }
    }

    let lines = line_spans(contents);
    let physical = physical_lines(contents);
    let ending = document_line_ending(contents);
    let head = node.line_index;
    let dropped = rewrite_checkbox(lines[head].text, Some('-'))
        .ok_or_else(|| WriteRefusal::new("not_a_req", "Not a requirement"))?;

    let children = direct_children(&lines, &node);
    let log = children
        .iter()
        .copied()
        .find(|&index| is_cancel_log(lines[index].text));
    let reason = reason.split_whitespace().collect::<Vec<_>>().join(" ");
    let entry = if !reason.is_empty() {
        Some(reason)
    } else if log.is_some() {
        Some(NO_REASON_GIVEN.to_string())
    } else {
        None
    };

    let unit = nesting_unit(&lines, head);
    let mut out: Vec<String> =
        physical.iter().map(|line| (*line).to_string()).collect();
    out[head] = replace_line_text(physical[head], &dropped);
    let outcome = match (log, entry) {
        (Some(log), Some(entry)) => {
            let marker = lines[log].text;
            let marker_indent = &marker[..leading_spaces_or_tabs_len(marker)];
            let entry_indent = lines
                .get(log + 1)
                .filter(|next| {
                    leading_spaces_or_tabs_len(next.text) > marker_indent.len()
                })
                .map_or_else(
                    || format!("{marker_indent}{unit}"),
                    |next| {
                        next.text[..leading_spaces_or_tabs_len(next.text)]
                            .to_string()
                    },
                );
            out[log] = with_ending(&out[log], ending);
            out.insert(
                log + 1,
                format!("{entry_indent}- *{date}* — {entry}{ending}"),
            );
            LogOutcome::Prepended
        }
        (None, Some(entry)) => {
            let head_indent = &lines[head].text
                [..leading_spaces_or_tabs_len(lines[head].text)];
            let header_indent = children.first().map_or_else(
                || format!("{head_indent}{unit}"),
                |&first| {
                    lines[first].text
                        [..leading_spaces_or_tabs_len(lines[first].text)]
                        .to_string()
                },
            );
            let entry_indent = format!("{header_indent}{unit}");
            out[head] = with_ending(&out[head], ending);
            out.insert(
                head + 1,
                format!("{entry_indent}- *{date}* — {entry}{ending}"),
            );
            out.insert(
                head + 1,
                format!("{header_indent}- {CANCEL_LOG_MARKER}{ending}"),
            );
            LogOutcome::Created
        }
        (_, None) => LogOutcome::None,
    };

    Ok(ReqWrite {
        updated: restore_final_newline(contents, out.concat()),
        line_index: head,
        log: outcome,
        descendants: descendant_count(&forest, &node),
    })
}

/// Restore a dropped req: `[-]` becomes `[ ]` and the Cancel Log stays, the
/// way reopening a task keeps its log.
pub(crate) fn restore_req(
    contents: &str,
    target: ReqTarget<'_>,
    settings: &NoteTaskSettings,
) -> Written<ReqWrite> {
    let (node, forest) = locate_req(contents, target, settings)?;
    match node.mark {
        ReqMark::Dropped => {}
        ReqMark::Carried => {
            return Err(WriteRefusal::new(
                "req_carried",
                "Carried — open the linked task",
            ));
        }
        ReqMark::LegacyMet => {
            return Err(WriteRefusal::new("req_met", "Already met"));
        }
        ReqMark::Open | ReqMark::Unknown(_) => {
            return Err(WriteRefusal::new(
                "req_not_dropped",
                format!(
                    "Requirements are met when “{}” closes — Ctrl+Enter on the task",
                    owner_text(contents, &node, settings)
                ),
            ));
        }
    }
    let lines = line_spans(contents);
    let physical = physical_lines(contents);
    let restored = rewrite_checkbox(lines[node.line_index].text, Some(' '))
        .ok_or_else(|| WriteRefusal::new("not_a_req", "Not a requirement"))?;
    let mut out: Vec<String> =
        physical.iter().map(|line| (*line).to_string()).collect();
    out[node.line_index] =
        replace_line_text(physical[node.line_index], &restored);
    Ok(ReqWrite {
        updated: restore_final_newline(contents, out.concat()),
        line_index: node.line_index,
        log: LogOutcome::None,
        descendants: descendant_count(&forest, &node),
    })
}

/// Which way Ctrl+] turned a line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ToggleDirection {
    NoteToReq,
    ReqToNote,
}

/// The result of a Ctrl+] toggle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ToggleWrite {
    pub(crate) updated: String,
    /// 0-based head line of the toggled item after the write.
    pub(crate) line_index: usize,
    pub(crate) direction: ToggleDirection,
    /// The req depth (after a note became a req, before a req became a note).
    pub(crate) depth: usize,
    /// Loose notes the first-req scoop filed under `NOTES`.
    pub(crate) scooped: usize,
    /// `NOTES` was created by this write.
    pub(crate) notes_created: bool,
}

/// Ctrl+] on a line.
///
/// - **Note to req.** At depth 1 the line and its subtree move to the end of
///   the reqs zone, and the first req of a task runs the first-req scoop in
///   the same write. Deeper, the toggle happens in place. A note inside a
///   drawer, under another note, or under a managed line refuses with the
///   capture copy (`req_in_drawer`, `req_parent`).
/// - **Req to note.** A leaf converts in place at depth 2 or more; at depth 1
///   the line and its subtree move into `NOTES`. A req with descendant reqs
///   refuses: carry it or toggle them first.
pub(crate) fn toggle_note_req(
    contents: &str,
    target: ReqTarget<'_>,
    settings: &NoteTaskSettings,
) -> Written<ToggleWrite> {
    stale(contents, target)?;
    let forest = task_reqs::note_req_forest(contents, settings);
    match forest
        .iter()
        .find(|node| node.line_index == target.line_index)
    {
        Some(node) => req_to_note(contents, node, &forest, settings),
        None => note_to_req(contents, target, &forest, settings),
    }
}

fn req_to_note(
    contents: &str,
    node: &ReqNode,
    forest: &[ReqNode],
    settings: &NoteTaskSettings,
) -> Written<ToggleWrite> {
    match node.mark {
        ReqMark::Dropped => {
            return Err(WriteRefusal::new(
                "req_dropped",
                "Dropped — restore it first",
            ));
        }
        ReqMark::Carried => {
            return Err(WriteRefusal::new(
                "req_carried",
                "Carried — open the linked task",
            ));
        }
        ReqMark::Open | ReqMark::Unknown(_) | ReqMark::LegacyMet => {}
    }
    let nested = descendant_count(forest, node);
    if nested > 0 {
        return Err(WriteRefusal::new(
            "req_has_nested",
            format!(
                "“{}” has {nested} nested requirement{} — carry it or toggle them first",
                node.text,
                if nested == 1 { "" } else { "s" },
            ),
        ));
    }
    let lines = line_spans(contents);
    let physical = physical_lines(contents);
    let note = rewrite_checkbox(lines[node.line_index].text, None)
        .ok_or_else(|| WriteRefusal::new("not_a_req", "Not a requirement"))?;
    if node.depth >= 2 {
        let mut out: Vec<String> =
            physical.iter().map(|line| (*line).to_string()).collect();
        out[node.line_index] =
            replace_line_text(physical[node.line_index], &note);
        return Ok(ToggleWrite {
            updated: restore_final_newline(contents, out.concat()),
            line_index: node.line_index,
            direction: ToggleDirection::ReqToNote,
            depth: node.depth,
            scooped: 0,
            notes_created: false,
        });
    }
    let (without, mut pulled) =
        pull_ranges(contents, &[(node.line_index, node.subtree_end)]);
    pulled[0].lines[0] = replace_line_text(&pulled[0].lines[0], &note);
    let filed = file_into_notes(&without, node.owner_line, &pulled, settings)
        .ok_or_else(|| {
        WriteRefusal::new("not_a_task", "No task owns this requirement")
    })?;
    Ok(ToggleWrite {
        updated: restore_final_newline(contents, filed.updated),
        line_index: filed.first_line,
        direction: ToggleDirection::ReqToNote,
        depth: node.depth,
        scooped: 0,
        notes_created: filed.created,
    })
}

fn note_to_req(
    contents: &str,
    target: ReqTarget<'_>,
    forest: &[ReqNode],
    settings: &NoteTaskSettings,
) -> Written<ToggleWrite> {
    let lines = line_spans(contents);
    let text = lines[target.line_index].text;
    let kind = task_children::classify_task_child_with_filter(
        text,
        Some(true),
        &settings.global_filter,
    );
    let refuse_parent =
        || WriteRefusal::new("req_parent", task_reqs::copy::REQ_PARENT);
    let Some(body) = list_item_body(text) else {
        return Err(WriteRefusal::new(
            "not_a_note",
            "Only a note can become a requirement",
        ));
    };
    match kind {
        TaskChildKind::Authored => {}
        // A checkbox that is no req by structure is an ordinary checklist.
        TaskChildKind::Requirement { .. } => return Err(refuse_parent()),
        _ => {
            return Err(WriteRefusal::new(
                "not_a_note",
                "Only a note can become a requirement",
            ));
        }
    }

    let parent = nearest_shallower_list_item_parent(&lines, target.line_index)
        .ok_or_else(refuse_parent)?;
    let (owner_line, depth) = if let Some(owner) =
        find_task(contents, parent, settings)
    {
        (owner.line_index, 1)
    } else if let Some(req) =
        forest.iter().find(|node| node.line_index == parent)
    {
        (req.owner_line, req.depth + 1)
    } else {
        let parent_body = list_item_body(lines[parent].text).unwrap_or("");
        return Err(if capture_drawers::is_drawer_title(parent_body.trim()) {
            WriteRefusal::new("req_in_drawer", task_reqs::copy::REQ_IN_DRAWER)
        } else {
            refuse_parent()
        });
    };
    if depth > MAX_REQ_DEPTH {
        return Err(WriteRefusal::new("req_depth", task_reqs::copy::REQ_DEPTH));
    }

    let prefix = &text[..text.len() - body.len()];
    let converted = format!("{prefix}[ ] {body}");
    let physical = physical_lines(contents);
    if depth >= 2 {
        let mut out: Vec<String> =
            physical.iter().map(|line| (*line).to_string()).collect();
        out[target.line_index] =
            replace_line_text(physical[target.line_index], &converted);
        return Ok(ToggleWrite {
            updated: restore_final_newline(contents, out.concat()),
            line_index: target.line_index,
            direction: ToggleDirection::NoteToReq,
            depth,
            scooped: 0,
            notes_created: false,
        });
    }

    // Depth 1: the note and its subtree move to the end of the reqs zone.
    let owner =
        find_task(contents, owner_line, settings).ok_or_else(refuse_parent)?;
    let block = TaskBlock::read(contents, &owner, &settings.global_filter);
    let had_reqs = block.owns_reqs();
    let child = block
        .children
        .iter()
        .find(|child| child.line == target.line_index)
        .ok_or_else(refuse_parent)?;
    let (without, mut pulled) =
        pull_ranges(contents, &[(child.line, child.content_end(&lines))]);
    pulled[0].lines[0] = replace_line_text(&pulled[0].lines[0], &converted);
    let owner =
        find_task(&without, owner_line, settings).ok_or_else(refuse_parent)?;
    let slot = insertion_offset(&without, &owner, ChildKind::Req, settings);
    let mut updated =
        insert_lines(&without, slot.offset, &rebased(&pulled[0], &slot.indent));
    let mut scooped = 0;
    let mut notes_created = false;
    if !had_reqs
        && let Some(scoop) = scoop_loose_notes(&updated, owner_line, settings)
    {
        scooped = scoop.filed;
        notes_created = scoop.created;
        updated = scoop.updated;
    }
    let line_index = last_depth_one_req(&updated, owner_line, settings)
        .unwrap_or(slot.line_index);
    Ok(ToggleWrite {
        updated: restore_final_newline(contents, updated),
        line_index,
        direction: ToggleDirection::NoteToReq,
        depth,
        scooped,
        notes_created,
    })
}

/// The last depth-1 req of the owner: where a req inserted at the end of the
/// reqs zone ends up, whatever the first-req scoop moved around it.
fn last_depth_one_req(
    contents: &str,
    owner_line: usize,
    settings: &NoteTaskSettings,
) -> Option<usize> {
    task_reqs::note_req_forest(contents, settings)
        .iter()
        .filter(|node| node.owner_line == owner_line && node.depth == 1)
        .map(|node| node.line_index)
        .max()
}

/// The result of inserting reqs under a task.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct InsertWrite {
    pub(crate) updated: String,
    /// 0-based line of the first inserted req after the write.
    pub(crate) first_line: usize,
    /// Lines the reqs occupy (head lines plus their nested lines).
    pub(crate) line_count: usize,
    /// Requirements the write added at every depth: each block's head plus
    /// the reqs nested under it.
    pub(crate) added: usize,
    /// Loose notes the first-req scoop filed under `NOTES`.
    pub(crate) scooped: usize,
    /// `NOTES` was created by this write.
    pub(crate) notes_created: bool,
}

/// Insert req blocks at the end of the task's reqs zone. Each block is the
/// text of one depth-1 req and everything nested under it, written with
/// relative indentation: its first line is an open `- [ ] text` with no
/// indent, and the task's child indentation is added to every line.
///
/// A task that owned no req before runs the first-req scoop in the same
/// write: its loose direct notes move into `NOTES`, which is created only
/// when at least one note moves (`notes_drawer_birth = with_first_note`).
pub(crate) fn insert_reqs(
    contents: &str,
    target: ReqTarget<'_>,
    blocks: &[String],
    settings: &NoteTaskSettings,
) -> Written<InsertWrite> {
    stale(contents, target)?;
    let task: NoteTask = find_task(contents, target.line_index, settings)
        .ok_or_else(|| {
            WriteRefusal::new("not_a_task", "This line is not a task")
        })?;
    if blocks.is_empty() {
        return Err(WriteRefusal::new("req_empty", "Nothing to insert"));
    }
    for block in blocks {
        let first = block.lines().next().unwrap_or("");
        let open = !first.starts_with([' ', '\t'])
            && task_reqs::checkbox_mark_without_filter(
                first,
                &settings.global_filter,
            ) == Some(ReqMark::Open);
        if !open {
            return Err(WriteRefusal::new(
                "req_state",
                task_reqs::copy::REQ_STATE,
            ));
        }
    }

    let had_reqs =
        TaskBlock::read(contents, &task, &settings.global_filter).owns_reqs();
    let slot = insertion_offset(contents, &task, ChildKind::Req, settings);
    let lines: Vec<String> = blocks
        .iter()
        .flat_map(|block| block.split('\n'))
        .map(|line| {
            if line.trim().is_empty() {
                String::new()
            } else {
                format!("{}{line}", slot.indent)
            }
        })
        .collect();
    let line_count = lines.len();
    let mut updated = insert_lines(contents, slot.offset, &lines);
    let mut scooped = 0;
    let mut notes_created = false;
    if !had_reqs
        && let Some(scoop) =
            scoop_loose_notes(&updated, task.line_index, settings)
    {
        scooped = scoop.filed;
        notes_created = scoop.created;
        updated = scoop.updated;
    }
    let forest = task_reqs::note_req_forest(&updated, settings);
    let deepest = forest
        .iter()
        .any(|node| node.owner_line == task.line_index && node.depth_exceeded);
    if deepest {
        return Err(WriteRefusal::new("req_depth", task_reqs::copy::REQ_DEPTH));
    }
    let owned_before = task_reqs::note_req_forest(contents, settings)
        .iter()
        .filter(|node| node.owner_line == task.line_index)
        .count();
    let owned_after = forest
        .iter()
        .filter(|node| node.owner_line == task.line_index)
        .count();
    let added = owned_after.saturating_sub(owned_before);
    let top_level: Vec<usize> = {
        let mut heads: Vec<usize> = forest
            .iter()
            .filter(|node| {
                node.owner_line == task.line_index && node.depth == 1
            })
            .map(|node| node.line_index)
            .collect();
        heads.sort_unstable();
        heads
    };
    let first_line = top_level
        .len()
        .checked_sub(blocks.len())
        .and_then(|from| top_level.get(from).copied())
        .unwrap_or(slot.line_index);
    Ok(InsertWrite {
        updated: restore_final_newline(contents, updated),
        first_line,
        line_count,
        added,
        scooped,
        notes_created,
    })
}
