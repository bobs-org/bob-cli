//! Shared classification for direct task children.

use std::collections::BTreeMap;

use super::{
    capture::{
        line_spans, list_item_body, nearest_shallower_list_item_parent,
        parse_managed_task_log_marker, ManagedTaskLogKind,
    },
    markdown,
    note_tasks::{self, NoteTask, NoteTaskSettings},
    task_dependencies, task_relations,
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
    Drawer { title: String },
    ManagedLink(ManagedLinkKind),
    ManagedLog(ManagedLogKind),
    NestedTask { open: bool },
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
/// needs the managed-line and drawer predicates.
pub(crate) fn classify_task_child(
    line: &str,
    nested_task_open: Option<bool>,
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
        return TaskChildKind::NestedTask { open };
    }

    if let Some(body) = list_item_body(line) {
        let title = body.trim();
        if super::capture_drawers::is_drawer_title(title) {
            return TaskChildKind::Drawer {
                title: title.to_string(),
            };
        }
    }

    TaskChildKind::Authored
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
    let tasks_by_line: BTreeMap<usize, bool> =
        note_tasks::scan(contents, settings)
            .tasks()
            .iter()
            .map(|task| (task.line_index, task.status_type.is_open()))
            .collect();

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
            Some(ClassifiedTaskChild {
                line_index,
                kind: classify_task_child(
                    line.text,
                    tasks_by_line.get(&line_index).copied(),
                ),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests;
