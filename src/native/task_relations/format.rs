//! Canonical LINKS / LINKED FROM formatters, sort, and insert slots
//! (`docs/task-relations.md` §§3.1, 4).

use std::collections::BTreeSet;

use super::super::task_dependencies::{
    blocks::{
        cmp_blocks_links, direct_child_lines, is_blocks_line, is_cancel_log,
        leading_whitespace, subtree_end,
    },
    parse::is_dependency_line,
};
use super::parse::{is_subprojects_line, links_children_of};
use super::{KNOT_EMOJI, LINKED_FROM_LABEL, LINKS_LABEL, RELATION_SEPARATOR};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RelationInsertSlot {
    pub(crate) line_index: usize,
    pub(crate) indent: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RelationSlotKind {
    Links,
    LinkedFrom,
}

/// Render one canonical LINKS line. Entries stay in authored order.
pub(crate) fn format_links_line(
    indent: &str,
    entries: &[(String, Option<String>)],
) -> String {
    let body = entries
        .iter()
        .map(|(link, label)| match label {
            Some(label) if !label.is_empty() => format!("{label} {link}"),
            _ => link.clone(),
        })
        .collect::<Vec<_>>()
        .join(RELATION_SEPARATOR);
    format!("{indent}- {KNOT_EMOJI} **{LINKS_LABEL}:** {body}")
}

/// Render one canonical LINKED FROM line. Entries are sorted as BK21.
pub(crate) fn format_linked_from_line(
    indent: &str,
    entries: &[(String, Option<String>)],
) -> String {
    let mut ordered = entries.to_vec();
    sort_linked_from_entries(&mut ordered);
    let body = ordered
        .iter()
        .map(|(link, label)| match label {
            Some(label) if !label.is_empty() => {
                format!("{link} ({label})")
            }
            _ => link.clone(),
        })
        .collect::<Vec<_>>()
        .join(RELATION_SEPARATOR);
    format!("{indent}- {KNOT_EMOJI} **{LINKED_FROM_LABEL}:** {body}")
}

pub(crate) fn sort_linked_from_entries(
    entries: &mut [(String, Option<String>)],
) {
    entries.sort_by(|left, right| {
        cmp_blocks_links(&left.0, &right.0).then_with(|| left.1.cmp(&right.1))
    });
}

/// Position for a missing LINKS or LINKED FROM child (Rule 3).
pub(crate) fn relation_insert_slot(
    lines: &[&str],
    fenced: &BTreeSet<usize>,
    task_line: usize,
    kind: RelationSlotKind,
) -> RelationInsertSlot {
    let owner = lines.get(task_line).copied().unwrap_or("");
    let parent_indent = leading_whitespace(owner).to_string();
    let fallback_indent = format!("{parent_indent}\t");
    let children = direct_child_lines(lines, fenced, task_line);
    let child_indent = children
        .first()
        .map(|index| leading_whitespace(lines[*index]).to_string())
        .unwrap_or(fallback_indent);

    if kind == RelationSlotKind::LinkedFrom
        && let Some(child) = links_children_of(lines, fenced, task_line).first()
    {
        return RelationInsertSlot {
            line_index: subtree_end(lines, child.line_index),
            indent: child_indent,
        };
    }

    let mut last_anchor = None;
    for &index in &children {
        let line = lines[index];
        if is_cancel_log(line)
            || is_dependency_line(line)
            || is_blocks_line(line)
            || is_subprojects_line(line)
        {
            last_anchor = Some(index);
        }
    }
    if let Some(anchor) = last_anchor {
        return RelationInsertSlot {
            line_index: subtree_end(lines, anchor),
            indent: child_indent,
        };
    }

    if let Some(&first) = children.first() {
        return RelationInsertSlot {
            line_index: first,
            indent: child_indent,
        };
    }

    RelationInsertSlot {
        line_index: task_line + 1,
        indent: child_indent,
    }
}

#[allow(dead_code)]
pub(crate) fn linked_from_insert_slot(
    lines: &[&str],
    fenced: &BTreeSet<usize>,
    task_line: usize,
) -> RelationInsertSlot {
    relation_insert_slot(lines, fenced, task_line, RelationSlotKind::LinkedFrom)
}

#[allow(dead_code)]
pub(crate) fn links_insert_slot(
    lines: &[&str],
    fenced: &BTreeSet<usize>,
    task_line: usize,
) -> RelationInsertSlot {
    relation_insert_slot(lines, fenced, task_line, RelationSlotKind::Links)
}
