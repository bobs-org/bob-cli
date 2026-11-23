//! Shared classification for direct task children.

use std::collections::BTreeMap;

use super::{
    capture::{
        line_spans, list_item_body, nearest_shallower_list_item_parent,
        parse_managed_task_log_marker, ManagedTaskLogKind,
    },
    markdown,
    note_tasks::{self, NoteTask, NoteTaskSettings},
    task_dependencies, task_relations, task_reqs,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ManagedLinkKind {
    DependsOn,
    Blocks,
    Links,
    LinkedFrom,
    SubProjects,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ManagedLogKind {
    Schedule,
    Work,
    Rotten,
    Cancel,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TaskChildKind {
    Authored,
    Drawer {
        title: String,
    },
    ManagedLink(ManagedLinkKind),
    ManagedLog(ManagedLogKind),
    NestedTask {
        open: bool,
    },
    /// A checkbox child without the global filter: a requirement candidate.
    /// Structural position (parent chain, fences) is decided by
    /// [`task_reqs`][crate::native::task_reqs]; this single-line check only
    /// separates checkbox children from plain authored notes.
    Requirement {
        mark: task_reqs::ReqMark,
    },
}

impl TaskChildKind {
    pub(crate) fn is_managed(&self) -> bool {
        matches!(self, Self::ManagedLink(_) | Self::ManagedLog(_))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(not(test), allow(dead_code))] // Shared by the task merge phase.
pub(crate) struct ClassifiedTaskChild {
    pub(crate) line_index: usize,
    pub(crate) kind: TaskChildKind,
}

/// Classify one list-item line. `nested_task_open` comes from Bob's task
/// scanner when the caller has a note context; `None` means this caller only
/// needs the managed-line and drawer predicates. The default global filter
/// (`#task`) separates requirement checkboxes from nested tasks.
pub(crate) fn classify_task_child(
    line: &str,
    nested_task_open: Option<bool>,
) -> TaskChildKind {
    classify_task_child_with_filter(line, nested_task_open, "#task")
}

/// [`classify_task_child`] with an explicit global filter, so recursive
/// consumers can reuse the same whole-token match [`task_reqs`][crate::native::task_reqs]
/// uses for requirement recognition.
pub(crate) fn classify_task_child_with_filter(
    line: &str,
    nested_task_open: Option<bool>,
    global_filter: &str,
) -> TaskChildKind {
    if task_relations::parse::is_subprojects_line(line) {
        return TaskChildKind::ManagedLink(ManagedLinkKind::SubProjects);
    }

    if task_dependencies::is_managed_link_line(line) {
        let kind = if task_dependencies::is_dependency_line(line) {
            ManagedLinkKind::DependsOn
        } else if task_dependencies::blocks::is_blocks_line(line) {
            ManagedLinkKind::Blocks
        } else if task_relations::is_links_line(line) {
            ManagedLinkKind::Links
        } else {
            ManagedLinkKind::LinkedFrom
        };
        return TaskChildKind::ManagedLink(kind);
    }

    if task_dependencies::blocks::is_cancel_log(line) {
        return TaskChildKind::ManagedLog(ManagedLogKind::Cancel);
    }

    if let Some(kind) = parse_managed_task_log_marker(line) {
        return TaskChildKind::ManagedLog(match kind {
            ManagedTaskLogKind::Schedule => ManagedLogKind::Schedule,
            ManagedTaskLogKind::Work => ManagedLogKind::Work,
            ManagedTaskLogKind::Rotten => ManagedLogKind::Rotten,
        });
    }

    if let Some(open) = nested_task_open {
        if carries_task_filter(line, global_filter) {
            return TaskChildKind::NestedTask { open };
        }
    } else if carries_task_filter(line, global_filter) {
        // Without a note context the caller cannot tell open from closed;
        // the line is still a task boundary, never a requirement.
        return TaskChildKind::Authored;
    }

    if let Some(mark) =
        task_reqs::checkbox_mark_without_filter(line, global_filter)
    {
        return TaskChildKind::Requirement { mark };
    }

    if let Some(body) = list_item_body(line)
        && let Some((title, _)) =
            super::capture_drawers::drawer_body_title(body)
        && !super::capture_drawers::is_reserved_drawer_key(
            &super::capture_drawers::drawer_key(title),
        )
        && !super::capture_drawers::is_structural_h2_key(
            &super::capture_drawers::drawer_key(title),
        )
    {
        return TaskChildKind::Drawer {
            title: title.to_string(),
        };
    }

    TaskChildKind::Authored
}

/// True when the checkbox body carries the task filter as a whole token.
fn carries_task_filter(line: &str, global_filter: &str) -> bool {
    list_item_body(line).is_some_and(|body| {
        !global_filter.is_empty()
            && body.split_whitespace().any(|token| token == global_filter)
    })
}

/// Classify list-item direct children of `parent`, preserving source order.
/// Nested task openness is derived using the same note settings as other Bob
/// task readers.
#[cfg_attr(not(test), allow(dead_code))] // Shared by the task merge phase.
pub(crate) fn classify_direct_children(
    contents: &str,
    parent: &NoteTask,
    settings: &NoteTaskSettings,
) -> Vec<ClassifiedTaskChild> {
    let tasks_by_line: BTreeMap<usize, bool> =
        note_tasks::scan(contents, settings)
            .tasks()
            .iter()
            .map(|task| (task.line_index, task.status_type.is_open()))
            .collect();
    classify_direct_children_with(
        contents,
        parent,
        &settings.global_filter,
        |line_index| tasks_by_line.get(&line_index).copied(),
    )
}

/// [`classify_direct_children`] without the whole-note task scan: a direct
/// child that carries the global filter is a [`TaskChildKind::NestedTask`]
/// (reported open, since only its boundary matters to layout callers), every
/// other child classifies exactly as in the scanning form. The block-layout
/// module uses this so a drawer slot can be found with no vault settings.
pub(crate) fn classify_direct_children_by_filter(
    contents: &str,
    parent: &NoteTask,
    global_filter: &str,
) -> Vec<ClassifiedTaskChild> {
    classify_direct_children_with(contents, parent, global_filter, |_| {
        Some(true)
    })
}

fn classify_direct_children_with(
    contents: &str,
    parent: &NoteTask,
    global_filter: &str,
    nested_task_open: impl Fn(usize) -> Option<bool>,
) -> Vec<ClassifiedTaskChild> {
    let lines = line_spans(contents);
    if parent.line_index >= lines.len() {
        return Vec::new();
    }
    // A fenced block nested inside a list item starts after the list marker
    // (`- ```lang`), so feed the item body to the fence scanner. This keeps
    // list-shaped examples inside such fences out of the parent’s children.
    let logical: Vec<&str> = lines
        .iter()
        .map(|line| list_item_body(line.text).unwrap_or(line.text))
        .collect();
    let fenced = markdown::fenced_lines(&logical, 0..logical.len());

    lines
        .iter()
        .enumerate()
        .skip(parent.line_index + 1)
        .take_while(|(_, line)| line.end <= parent.block_end)
        .filter_map(|(line_index, line)| {
            if fenced.contains(&line_index)
                || line.text.trim().is_empty()
                || list_item_body(line.text).is_none()
                || nearest_shallower_list_item_parent(&lines, line_index)
                    != Some(parent.line_index)
            {
                return None;
            }
            let classified = classify_task_child_with_filter(
                line.text,
                nested_task_open(line_index),
                global_filter,
            );
            let kind = if let Some(header) =
                super::capture_drawers::recognize_drawer_header(
                    contents, parent, line_index,
                ) {
                TaskChildKind::Drawer {
                    title: header.title,
                }
            } else if matches!(classified, TaskChildKind::Drawer { .. }) {
                TaskChildKind::Authored
            } else {
                classified
            };
            Some(ClassifiedTaskChild { line_index, kind })
        })
        .collect()
}

#[cfg(test)]
mod tests;
