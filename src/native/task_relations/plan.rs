//! LINKS and LINKED FROM edit planners (`docs/task-relations.md` §§4, 7).

use std::collections::{BTreeMap, BTreeSet};

use super::super::task_dependencies::blocks::{
    leading_whitespace, BlocksEdit, BlocksLinePlan, BlocksLineReport,
    BlocksUpdateKind,
};
use super::format::{
    format_linked_from_line, format_links_line, relation_insert_slot,
    sort_linked_from_entries, RelationSlotKind,
};
use super::label::{normalize_relation_label, LabelError};
use super::parse::{linked_from_children_of, links_children_of};
use super::{LinkedFromLine, LinksLine, RelationEntry};

pub(crate) type RelationEdit = BlocksEdit;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct LinksEdit {
    pub add: Vec<(String, Option<String>)>,
    pub remove: BTreeSet<String>,
    pub relabel: BTreeMap<String, Option<String>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LinksLineReport {
    pub added: Vec<String>,
    pub relabeled: Vec<String>,
    pub unchanged: Vec<String>,
    pub removed: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum LinksLinePlan {
    Noop,
    Malformed,
    InvalidLabel(LabelError),
    Edits {
        edits: Vec<RelationEdit>,
        report: LinksLineReport,
    },
}

pub(crate) fn plan_links_edit(
    lines: &[&str],
    fenced: &BTreeSet<usize>,
    owner_line: usize,
    edit: &LinksEdit,
) -> LinksLinePlan {
    let children = links_children_of(lines, fenced, owner_line);
    if children
        .iter()
        .any(|child| matches!(child.parsed, LinksLine::Malformed))
    {
        return LinksLinePlan::Malformed;
    }

    let mut add = Vec::new();
    for (link, label) in &edit.add {
        match normalize_optional_label(label.as_deref()) {
            Ok(label) => add.push((link.clone(), label)),
            Err(error) => return LinksLinePlan::InvalidLabel(error),
        }
    }
    let mut relabel = BTreeMap::new();
    for (identity, label) in &edit.relabel {
        match normalize_optional_label(label.as_deref()) {
            Ok(label) => {
                relabel.insert(identity.clone(), label);
            }
            Err(error) => return LinksLinePlan::InvalidLabel(error),
        }
    }

    let mut entries: Vec<(String, Option<String>)> = Vec::new();
    let mut seen = BTreeSet::new();
    let mut existing_indexes = Vec::new();
    for child in &children {
        existing_indexes.push(child.line_index);
        match &child.parsed {
            LinksLine::Accepted {
                entries: parsed, ..
            } => {
                for entry in parsed {
                    let identity = entry.canonical_text();
                    if seen.insert(identity.clone()) {
                        entries.push((identity, entry.label.clone()));
                    }
                }
            }
            LinksLine::Empty | LinksLine::Malformed | LinksLine::NotALine => {}
        }
    }
    let original = entries.clone();

    entries.retain(|(identity, _)| !edit.remove.contains(identity));
    for (identity, label) in &relabel {
        if let Some(entry) = entries
            .iter_mut()
            .find(|(existing, _)| existing == identity)
        {
            entry.1 = label.clone();
        }
    }

    let mut added = Vec::new();
    let mut relabeled = Vec::new();
    let mut unchanged = Vec::new();
    for (link, label) in add {
        if let Some(entry) =
            entries.iter_mut().find(|(existing, _)| existing == &link)
        {
            if entry.1 == label {
                unchanged.push(link);
            } else {
                entry.1 = label;
                relabeled.push(link);
            }
        } else {
            entries.push((link.clone(), label));
            added.push(link);
        }
    }
    for (identity, label) in &relabel {
        if original.iter().any(|(existing, previous)| {
            existing == identity && previous != label
        }) && !relabeled.contains(identity)
        {
            relabeled.push(identity.clone());
        }
    }
    let removed = original
        .iter()
        .filter(|(identity, _)| {
            !entries.iter().any(|(existing, _)| existing == identity)
        })
        .map(|(identity, _)| identity.clone())
        .collect::<Vec<_>>();

    let report = LinksLineReport {
        added,
        relabeled,
        unchanged,
        removed,
    };

    if entries.is_empty() {
        if existing_indexes.is_empty() {
            return LinksLinePlan::Noop;
        }
        return LinksLinePlan::Edits {
            edits: existing_indexes
                .iter()
                .map(|&line_index| RelationEdit::Remove { line_index })
                .collect(),
            report,
        };
    }

    let Some(&first_index) = existing_indexes.first() else {
        let slot = relation_insert_slot(
            lines,
            fenced,
            owner_line,
            RelationSlotKind::Links,
        );
        return LinksLinePlan::Edits {
            edits: vec![RelationEdit::Insert {
                line_index: slot.line_index,
                text: format_links_line(&slot.indent, &entries),
            }],
            report,
        };
    };

    let formatted =
        format_links_line(leading_whitespace(lines[first_index]), &entries);
    let extras = &existing_indexes[1..];
    if extras.is_empty() && lines[first_index] == formatted {
        return LinksLinePlan::Noop;
    }
    let mut edits = vec![RelationEdit::Replace {
        line_index: first_index,
        text: formatted,
    }];
    edits.extend(
        extras
            .iter()
            .map(|&line_index| RelationEdit::Remove { line_index }),
    );
    LinksLinePlan::Edits { edits, report }
}

/// Desired LINKED FROM entries after a gesture delta.
pub(crate) fn linked_from_desired_after_delta<F>(
    existing: &[RelationEntry],
    mut resolve: F,
    add: &BTreeMap<String, (String, Option<String>)>,
    remove: &BTreeSet<String>,
) -> Vec<(String, Option<String>)>
where
    F: FnMut(&RelationEntry) -> Option<(String, String)>,
{
    let mut desired = BTreeMap::new();
    for entry in existing {
        if let Some((identity, canonical_text)) = resolve(entry)
            && !remove.contains(&identity)
        {
            desired.insert(identity, (canonical_text, entry.label.clone()));
        }
    }
    for (identity, (canonical_text, label)) in add {
        if !remove.contains(identity) {
            desired.insert(
                identity.clone(),
                (canonical_text.clone(), label.clone()),
            );
        }
    }
    let mut entries = desired.into_values().collect::<Vec<_>>();
    sort_linked_from_entries(&mut entries);
    entries
}

pub(crate) fn plan_linked_from_line(
    lines: &[&str],
    fenced: &BTreeSet<usize>,
    owner_line: usize,
    desired: &BTreeSet<(String, Option<String>)>,
) -> BlocksLinePlan {
    let children = linked_from_children_of(lines, fenced, owner_line);
    if children
        .iter()
        .any(|child| matches!(child.parsed, LinkedFromLine::Malformed))
    {
        return BlocksLinePlan::Malformed;
    }

    let mut desired_entries = desired.iter().cloned().collect::<Vec<_>>();
    sort_linked_from_entries(&mut desired_entries);
    let desired_set = desired_entries.iter().cloned().collect::<BTreeSet<_>>();

    let existing_lines = children
        .iter()
        .filter_map(|child| match &child.parsed {
            LinkedFromLine::Accepted { entries, canonical } => Some((
                child.line_index,
                entries
                    .iter()
                    .map(|entry| (entry.canonical_text(), entry.label.clone()))
                    .collect::<Vec<_>>(),
                *canonical,
            )),
            LinkedFromLine::Empty => {
                Some((child.line_index, Vec::new(), false))
            }
            LinkedFromLine::Malformed | LinkedFromLine::NotALine => None,
        })
        .collect::<Vec<_>>();

    let mut current_set = BTreeSet::new();
    for (_, entries, _) in &existing_lines {
        current_set.extend(entries.iter().cloned());
    }
    let added = desired_set
        .difference(&current_set)
        .map(|(link, _)| link.clone())
        .collect::<Vec<_>>();
    let removed = current_set
        .difference(&desired_set)
        .map(|(link, _)| link.clone())
        .collect::<Vec<_>>();

    if desired_entries.is_empty() {
        if existing_lines.is_empty() {
            return BlocksLinePlan::Noop;
        }
        let edits = existing_lines
            .iter()
            .map(|(line_index, _, _)| BlocksEdit::Remove {
                line_index: *line_index,
            })
            .collect();
        return BlocksLinePlan::Edits {
            edits,
            report: BlocksLineReport {
                kind: if existing_lines.len() > 1 {
                    BlocksUpdateKind::Deduplicated
                } else {
                    BlocksUpdateKind::Removed
                },
                added,
                removed,
            },
        };
    }

    let Some((first_index, first_entries, first_canonical)) =
        existing_lines.first()
    else {
        let slot = relation_insert_slot(
            lines,
            fenced,
            owner_line,
            RelationSlotKind::LinkedFrom,
        );
        return BlocksLinePlan::Edits {
            edits: vec![BlocksEdit::Insert {
                line_index: slot.line_index,
                text: format_linked_from_line(&slot.indent, &desired_entries),
            }],
            report: BlocksLineReport {
                kind: BlocksUpdateKind::Inserted,
                added,
                removed,
            },
        };
    };

    let first_set = first_entries.iter().cloned().collect::<BTreeSet<_>>();
    let exact_entries = first_set == desired_set
        && first_entries.len() == desired_entries.len();
    let has_duplicates =
        existing_lines.len() > 1 || first_entries.len() != first_set.len();
    let canonical_line = format_linked_from_line(
        leading_whitespace(lines[*first_index]),
        &desired_entries,
    );
    let mut edits = Vec::new();
    let kind = if has_duplicates {
        edits.push(BlocksEdit::Replace {
            line_index: *first_index,
            text: canonical_line,
        });
        edits.extend(existing_lines.iter().skip(1).map(
            |(line_index, _, _)| BlocksEdit::Remove {
                line_index: *line_index,
            },
        ));
        BlocksUpdateKind::Deduplicated
    } else if !exact_entries {
        edits.push(BlocksEdit::Replace {
            line_index: *first_index,
            text: canonical_line,
        });
        BlocksUpdateKind::Rewritten
    } else if !first_canonical {
        edits.push(BlocksEdit::Replace {
            line_index: *first_index,
            text: canonical_line,
        });
        BlocksUpdateKind::Canonicalized
    } else {
        return BlocksLinePlan::Noop;
    };

    BlocksLinePlan::Edits {
        edits,
        report: BlocksLineReport {
            kind,
            added,
            removed,
        },
    }
}

fn normalize_optional_label(
    label: Option<&str>,
) -> Result<Option<String>, LabelError> {
    match label {
        None => Ok(None),
        Some(raw) => Ok(Some(normalize_relation_label(raw)?)),
    }
}
