//! Task block layout: the one place that answers "where does a new child of
//! kind K go?" (`docs/task-requirements.md` §3).
//!
//! A task's direct children sit in six zones, in order:
//!
//! 1. **Cancel Log** — first wherever a Cancel Log exists.
//! 2. **Managed links** — DEPENDS ON, BLOCKS, SUB-PROJECTS, LINKS, LINKED FROM.
//! 3. **Reqs** — each req moves with its whole subtree.
//! 4. **Authored** — loose notes and nested `#task` subtasks, authored order.
//! 5. **Drawers** — `NOTES` first, then the others in authored order.
//! 6. **Logs** — SCHEDULE, WORK, ROTTEN, in their existing relative order.
//!
//! Order inside a zone is authored order and is never re-sorted. Order is a
//! writer rule plus an explicit `bob task tidy`; it is never a validity rule
//! and never a silent reorder.
//!
//! **The NOTES rule.** A task that owns at least one req (any state, any
//! depth) keeps its notes in a `NOTES` drawer. The gesture that gives a task
//! its first req files the task's loose direct notes into `NOTES` in the same
//! write (the "first-req scoop"); later Bob-written notes file there too.
//! `NOTES` is created only when a note lands in it (`notes_drawer_birth =
//! with_first_note`), so a drawer is nonempty by construction. On a `^prj`
//! owner the home is the project note's `## Notes` section (Title Case when
//! created; an existing `## NOTES` spelling is kept), with column-zero
//! bullets.
//!
//! **Zero-req tasks keep today's placement.** A task with no reqs files a
//! loose note at the tail, before its first managed log, and authored notes
//! and drawers may interleave: capture has always written notes after the
//! drawers, and tidy never reshuffles that history. For such a task only the
//! Cancel Log, managed-link, and log zones are enforced.
//!
//! This module is pure: no filesystem, no clock. The shared RL and RW vectors
//! live in `tests/fixtures/task_reqs/` and are copied byte for byte by the
//! Obsidian twin.

use super::{
    capture::{
        dominant_indent_unit, first_child_indentation, line_spans, LineSpan,
    },
    capture_drawers::{self, Drawer, DrawerResolveOutcome, DrawerResolved},
    note_tasks::{self, NoteTask, NoteTaskSettings},
    task_children::{self, ManagedLogKind, TaskChildKind},
};

mod edit;
mod tidy;
mod writers;

#[cfg(test)]
mod tests;

// The public surface later epic phases import; nothing outside the module
// names every type yet.
#[allow(unused_imports)]
pub(crate) use tidy::{
    canonicalize_new_req_block, plan_tidy, CanonicalReqBlock, TidyPlan,
    TidyPolicy,
};
#[allow(unused_imports)]
pub(crate) use writers::{
    drop_req, insert_reqs, restore_req, toggle_note_req, InsertWrite,
    ReqTarget, ReqWrite, ToggleDirection, ToggleWrite, WriteRefusal,
};

/// Drawer key of the notes drawer a req-owning task keeps.
/// Identity key shared by every spelling of the notes home.
pub(crate) const NOTES_KEY: &str = "NOTES";
/// Title every writer uses when it creates the notes drawer.
pub(crate) const NOTES_TITLE: &str = "Notes";

/// Title Case heading created for a `^prj` task's `## Notes` section.
pub(crate) const NOTES_SECTION_TITLE: &str = "Notes";

/// The global filter default; zone detection only needs it to tell a nested
/// `#task` from a requirement, and drawer slots need neither.
const DEFAULT_FILTER: &str = "#task";

/// A child zone, in canonical order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Zone {
    CancelLog,
    ManagedLinks,
    Reqs,
    Authored,
    Drawers,
    Logs,
}

/// The kind of child a writer wants to add.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ChildKind {
    /// The `❌ **CANCEL LOG**` marker: the task's first child.
    CancelLog,
    /// A managed link line: the end of the managed-link zone (the per-kind
    /// writers keep their finer order inside the zone).
    ManagedLink,
    /// A requirement: the end of the reqs zone.
    Req,
    /// A loose authored child (a note or a nested `#task`). On a task that
    /// owns reqs it lands at the end of the authored zone; on a task with no
    /// reqs it keeps today's tail slot, before the first managed log.
    Authored,
    /// A new drawer header other than `NOTES`: before the first managed log.
    Drawer,
    /// A new `NOTES` header: first among the drawers.
    NotesDrawer,
    /// A new managed log: the end of the block.
    Log,
}

impl ChildKind {
    /// Short stable name used by vectors.
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::CancelLog => "cancel-log",
            Self::ManagedLink => "managed-link",
            Self::Req => "req",
            Self::Authored => "authored",
            Self::Drawer => "drawer",
            Self::NotesDrawer => "notes-drawer",
            Self::Log => "log",
        }
    }

    /// Parse a vector's kind name.
    #[cfg(test)]
    pub(crate) fn parse(name: &str) -> Option<Self> {
        [
            Self::CancelLog,
            Self::ManagedLink,
            Self::Req,
            Self::Authored,
            Self::Drawer,
            Self::NotesDrawer,
            Self::Log,
        ]
        .into_iter()
        .find(|kind| kind.name() == name)
    }
}

/// An insertion point: insert new lines before `line_index`
/// (`line_index == line count` appends), at the given child indentation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Slot {
    pub(crate) line_index: usize,
    pub(crate) offset: usize,
    pub(crate) indent: String,
}

/// One direct child of a task, with its extent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BlockChild {
    /// 0-based head line.
    pub(crate) line: usize,
    /// 0-based last line of the child's extent, inclusive: everything up to
    /// the next sibling, so trailing blank lines travel with their child.
    pub(crate) end: usize,
    pub(crate) kind: TaskChildKind,
    pub(crate) zone: Zone,
}

impl BlockChild {
    /// The last nonblank line of the child's extent.
    pub(crate) fn content_end(&self, lines: &[LineSpan<'_>]) -> usize {
        let mut end = self.end;
        while end > self.line && lines[end].text.trim().is_empty() {
            end -= 1;
        }
        end
    }

    /// A plain authored note: not a nested `#task`.
    pub(crate) fn is_loose_note(&self) -> bool {
        matches!(self.kind, TaskChildKind::Authored)
    }

    pub(crate) fn is_notes_drawer(&self) -> bool {
        matches!(
            &self.kind,
            TaskChildKind::Drawer { title }
                if capture_drawers::drawer_key(title) == NOTES_KEY
        )
    }
}

/// A task's direct children in document order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TaskBlock {
    pub(crate) task_line: usize,
    /// 0-based last line of the task block, inclusive.
    pub(crate) last_line: usize,
    pub(crate) children: Vec<BlockChild>,
}

impl TaskBlock {
    pub(crate) fn read(
        contents: &str,
        task: &NoteTask,
        global_filter: &str,
    ) -> Self {
        let lines = line_spans(contents);
        let last_line = block_last_line(&lines, task);
        let classified = task_children::classify_direct_children_by_filter(
            contents,
            task,
            global_filter,
        );
        let children = classified
            .iter()
            .enumerate()
            .map(|(index, child)| BlockChild {
                line: child.line_index,
                end: classified
                    .get(index + 1)
                    .map_or(last_line, |next| next.line_index - 1),
                zone: zone_of(&child.kind),
                kind: child.kind.clone(),
            })
            .collect();
        Self {
            task_line: task.line_index,
            last_line,
            children,
        }
    }

    /// True when the task owns at least one req (any state, any depth: a
    /// deeper req always hangs off a depth-1 req).
    pub(crate) fn owns_reqs(&self) -> bool {
        self.children.iter().any(|child| child.zone == Zone::Reqs)
    }

    fn last_in(&self, accept: impl Fn(Zone) -> bool) -> Option<&BlockChild> {
        self.children.iter().rev().find(|child| accept(child.zone))
    }

    fn first_in(&self, zone: Zone) -> Option<&BlockChild> {
        self.children.iter().find(|child| child.zone == zone)
    }
}

/// The zone a classified child belongs to.
pub(crate) fn zone_of(kind: &TaskChildKind) -> Zone {
    match kind {
        TaskChildKind::ManagedLog(ManagedLogKind::Cancel) => Zone::CancelLog,
        TaskChildKind::ManagedLink(_) => Zone::ManagedLinks,
        TaskChildKind::Requirement { .. } => Zone::Reqs,
        TaskChildKind::Authored | TaskChildKind::NestedTask { .. } => {
            Zone::Authored
        }
        TaskChildKind::Drawer { .. } => Zone::Drawers,
        TaskChildKind::ManagedLog(_) => Zone::Logs,
    }
}

/// Last line index inside `task`'s block (`block_end` is a byte offset).
pub(crate) fn block_last_line(
    lines: &[LineSpan<'_>],
    task: &NoteTask,
) -> usize {
    lines
        .iter()
        .position(|line| line.end >= task.block_end)
        .unwrap_or_else(|| lines.len().saturating_sub(1))
        .max(task.line_index)
}

/// Byte offset of the start of `line_index`; the line count maps to the end
/// of the note.
pub(crate) fn line_offset(lines: &[LineSpan<'_>], line_index: usize) -> usize {
    match line_index {
        0 => 0,
        index => lines
            .get(index - 1)
            .or_else(|| lines.last())
            .map_or(0, |line| line.end),
    }
}

/// The indentation a new direct child of `task` takes: the first existing
/// child's, else the note's dominant indent unit, else a tab.
pub(crate) fn child_indent(lines: &[LineSpan<'_>], task: &NoteTask) -> String {
    first_child_indentation(
        lines,
        task.line_index,
        task.block_end,
        &task.indentation,
    )
    .or_else(|| {
        dominant_indent_unit(lines)
            .map(|unit| format!("{}{}", task.indentation, unit))
    })
    .unwrap_or_else(|| format!("{}\t", task.indentation))
}

/// Where a new child of `kind` goes under `task`.
pub(crate) fn insertion_offset(
    contents: &str,
    task: &NoteTask,
    kind: ChildKind,
    settings: &NoteTaskSettings,
) -> Slot {
    let lines = line_spans(contents);
    let block = TaskBlock::read(contents, task, &settings.global_filter);
    let line_index = slot_line(&lines, &block, kind);
    Slot {
        line_index,
        offset: line_offset(&lines, line_index),
        indent: child_indent(&lines, task),
    }
}

fn slot_line(
    lines: &[LineSpan<'_>],
    block: &TaskBlock,
    kind: ChildKind,
) -> usize {
    let after = |child: &BlockChild| child.content_end(lines) + 1;
    let managed_end = || {
        block
            .last_in(|zone| zone <= Zone::ManagedLinks)
            .map(after)
            .or_else(|| block.children.first().map(|child| child.line))
            .unwrap_or(block.last_line + 1)
    };
    let tail = || {
        block
            .first_in(Zone::Logs)
            .map_or(block.last_line + 1, |log| log.line)
    };
    match kind {
        ChildKind::CancelLog => block.task_line + 1,
        ChildKind::ManagedLink => managed_end(),
        ChildKind::Req => block
            .last_in(|zone| zone == Zone::Reqs)
            .map(after)
            .unwrap_or_else(managed_end),
        ChildKind::Authored if block.owns_reqs() => block
            .last_in(|zone| zone <= Zone::Authored)
            .map(after)
            .unwrap_or_else(managed_end),
        ChildKind::Authored | ChildKind::Drawer => tail(),
        ChildKind::NotesDrawer => block
            .first_in(Zone::Drawers)
            .map_or_else(tail, |drawer| drawer.line),
        ChildKind::Log => block.last_line + 1,
    }
}

/// Byte offset where a new drawer header lands: a created `NOTES` lands
/// first among the drawers, every other drawer keeps today's position
/// (before the first managed log, else the end of the block). It needs no
/// vault settings, so `capture_drawers::ensure_drawer` routes through it.
pub(crate) fn drawer_header_offset(
    contents: &str,
    task: &NoteTask,
    notes_first: bool,
) -> usize {
    let lines = line_spans(contents);
    let block = TaskBlock::read(contents, task, DEFAULT_FILTER);
    let kind = if notes_first {
        ChildKind::NotesDrawer
    } else {
        ChildKind::Drawer
    };
    line_offset(&lines, slot_line(&lines, &block, kind))
}

/// The resolution that creates a task-child `NOTES` drawer.
fn create_notes() -> DrawerResolved {
    DrawerResolved {
        outcome: DrawerResolveOutcome::Create {
            title: NOTES_TITLE.to_string(),
            key: NOTES_KEY.to_string(),
            slug: "notes".to_string(),
        },
        created: true,
        warning: None,
    }
}

/// The resolution that creates a project note's Title Case `## Notes` section.
fn create_prj_notes() -> DrawerResolved {
    DrawerResolved {
        outcome: DrawerResolveOutcome::Create {
            title: NOTES_SECTION_TITLE.to_string(),
            key: NOTES_KEY.to_string(),
            slug: "notes".to_string(),
        },
        created: true,
        warning: None,
    }
}

/// Create the task's `NOTES` drawer (first among the drawers) and return the
/// note with it, the drawer, and `true`. Callers use this only when
/// [`note_home`] answered [`NoteHome::NewNotes`]. On a `^prj` owner this is
/// the note's Title Case `## Notes` section, not a task-child bullet.
pub(crate) fn ensure_notes_drawer(
    contents: &str,
    task: &NoteTask,
) -> Result<(String, Drawer, bool), String> {
    let resolved = if capture_drawers::is_prj_parent(contents, task) {
        create_prj_notes()
    } else {
        create_notes()
    };
    capture_drawers::ensure_drawer(contents, task, &resolved)
}

/// The project note's `## Notes` (or `## NOTES`) H2 section, if any.
/// Mirrors `taskBlockProjectNotesDrawer`.
pub(crate) fn project_notes_drawer(contents: &str) -> Option<Drawer> {
    capture_drawers::project_h2_drawers(contents)
        .into_iter()
        .find(|drawer| drawer.key == NOTES_KEY)
}

/// The NOTES home of `task`: the project `## Notes` section on a `^prj`
/// owner, else the task-child `NOTES` drawer.
pub(crate) fn notes_drawer(contents: &str, task: &NoteTask) -> Option<Drawer> {
    if capture_drawers::is_prj_parent(contents, task) {
        project_notes_drawer(contents)
    } else {
        capture_drawers::drawers(contents, task)
            .into_iter()
            .find(|drawer| drawer.key == NOTES_KEY)
    }
}

/// Title written when NOTES is created for `task`: Title Case `Notes` on a
/// `^prj` owner, else the bold drawer title `Notes`.
pub(crate) fn notes_create_title(
    contents: &str,
    task: &NoteTask,
) -> &'static str {
    if capture_drawers::is_prj_parent(contents, task) {
        NOTES_SECTION_TITLE
    } else {
        NOTES_TITLE
    }
}

/// Where a Bob-written loose note on `task` files, following the NOTES rule.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum NoteHome {
    /// A task with no reqs: a loose child at today's tail slot.
    Loose(Slot),
    /// A req-owning task that already keeps a `NOTES` drawer.
    Notes(Drawer),
    /// A req-owning task with no `NOTES` drawer: create it, then file in it.
    NewNotes,
}

pub(crate) fn note_home(
    contents: &str,
    task: &NoteTask,
    settings: &NoteTaskSettings,
) -> NoteHome {
    let block = TaskBlock::read(contents, task, &settings.global_filter);
    if !block.owns_reqs() {
        return NoteHome::Loose(insertion_offset(
            contents,
            task,
            ChildKind::Authored,
            settings,
        ));
    }
    notes_drawer(contents, task).map_or(NoteHome::NewNotes, NoteHome::Notes)
}

/// Why a child is out of place.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ViolationKind {
    /// The child sits after a child of a later zone.
    OutOfZone { zone: Zone },
    /// `NOTES` is not the first drawer.
    NotesNotFirst,
    /// A req-owning task keeps a loose note outside `NOTES`.
    NoteOutsideNotes,
}

impl ViolationKind {
    /// Short stable name used by vectors.
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::OutOfZone { .. } => "out-of-zone",
            Self::NotesNotFirst => "notes-not-first",
            Self::NoteOutsideNotes => "note-outside-notes",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Violation {
    /// 0-based head line of the displaced child.
    pub(crate) line_index: usize,
    pub(crate) kind: ViolationKind,
}

/// Sort key of a child: its zone rank, plus a sub-rank that puts `NOTES`
/// first among the drawers of a req-owning task. On a task with no reqs the
/// authored and drawer zones share one rank (see the module docs).
fn sort_key(child: &BlockChild, owns_reqs: bool) -> (u8, u8) {
    let rank = match child.zone {
        Zone::CancelLog => 0,
        Zone::ManagedLinks => 1,
        Zone::Reqs => 2,
        Zone::Authored => 3,
        Zone::Drawers if owns_reqs => 4,
        Zone::Drawers => 3,
        Zone::Logs => 5,
    };
    let sub = u8::from(
        owns_reqs && child.zone == Zone::Drawers && !child.is_notes_drawer(),
    );
    (rank, sub)
}

/// Child positions (into [`TaskBlock::children`]) in canonical order: a
/// stable sort, so authored order survives inside each zone.
pub(crate) fn desired_order(block: &TaskBlock) -> Vec<usize> {
    let owns_reqs = block.owns_reqs();
    let mut order: Vec<usize> = (0..block.children.len()).collect();
    order.sort_by_key(|&index| sort_key(&block.children[index], owns_reqs));
    order
}

/// Every way the block drifts from the canonical layout.
pub(crate) fn layout_violations(
    contents: &str,
    task: &NoteTask,
    settings: &NoteTaskSettings,
) -> Vec<Violation> {
    let block = TaskBlock::read(contents, task, &settings.global_filter);
    let owns_reqs = block.owns_reqs();
    let mut violations = Vec::new();
    let mut highest: Option<(u8, u8)> = None;
    for child in &block.children {
        let key = sort_key(child, owns_reqs);
        if highest.is_some_and(|highest| key < highest) {
            let kind = if child.zone == Zone::Drawers && child.is_notes_drawer()
            {
                ViolationKind::NotesNotFirst
            } else {
                ViolationKind::OutOfZone { zone: child.zone }
            };
            violations.push(Violation {
                line_index: child.line,
                kind,
            });
        }
        highest = Some(highest.map_or(key, |highest| highest.max(key)));
        if owns_reqs && child.zone == Zone::Authored && child.is_loose_note() {
            violations.push(Violation {
                line_index: child.line,
                kind: ViolationKind::NoteOutsideNotes,
            });
        }
    }
    violations
}

/// The scanned task at `line_index`, for callers that rescan after a write.
pub(crate) fn find_task(
    contents: &str,
    line_index: usize,
    settings: &NoteTaskSettings,
) -> Option<NoteTask> {
    note_tasks::scan(contents, settings)
        .tasks()
        .iter()
        .find(|task| task.line_index == line_index)
        .cloned()
}
