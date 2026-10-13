//! Parser, formatter, and shared edit planner for generated BLOCKS lines.
//!
//! A BLOCKS line is a projection of a task's dependents, never an input to
//! the dependency graph. The parser intentionally uses the shared block-link
//! scanner so aliases, embeds, and struck links are understood without
//! splitting on separators that may occur inside an alias.

use std::collections::{BTreeMap, BTreeSet};

use super::super::task_status_hooks::{
    after_list_marker, leading_indentation_width, nearest_parent_list_item,
};
use super::block_link_spans;

pub(crate) const BLOCKS_EMOJI: &str = "🧱";
pub(crate) const BLOCKS_LABEL: &str = "BLOCKS";
pub(crate) const BLOCKS_SEPARATOR: &str = " • ";

const TOLERATED_EMOJI: &[&str] = &["🧱", "🚧", "⛓️", "⛓"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum BlocksLine {
    Accepted {
        links: Vec<BlocksLink>,
        canonical: bool,
    },
    Empty,
    Malformed,
    NotALine,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BlocksLink {
    pub(crate) target: String,
    pub(crate) block_id: String,
    pub(crate) embedded: bool,
    pub(crate) struck: bool,
    pub(crate) aliased: bool,
}

impl BlocksLink {
    fn canonical_text(&self) -> String {
        if self.target.is_empty() {
            format!("[[#^{}]]", self.block_id)
        } else {
            format!("[[{}#^{}]]", self.target, self.block_id)
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BlocksChild {
    pub(crate) line_index: usize,
    pub(crate) parsed: BlocksLine,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum BlocksEdit {
    Insert { line_index: usize, text: String },
    Replace { line_index: usize, text: String },
    Remove { line_index: usize },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BlocksUpdateKind {
    Inserted,
    Rewritten,
    Canonicalized,
    Removed,
    Deduplicated,
}

impl BlocksUpdateKind {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Inserted => "inserted",
            Self::Rewritten => "rewritten",
            Self::Canonicalized => "canonicalized",
            Self::Removed => "removed",
            Self::Deduplicated => "deduplicated",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BlocksLineReport {
    pub(crate) kind: BlocksUpdateKind,
    pub(crate) added: Vec<String>,
    pub(crate) removed: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum BlocksLinePlan {
    Noop,
    Malformed,
    Edits {
        edits: Vec<BlocksEdit>,
        report: BlocksLineReport,
    },
}

pub(crate) fn is_blocks_line(line: &str) -> bool {
    !matches!(parse_blocks_line(line), BlocksLine::NotALine)
}

pub(crate) fn parse_blocks_line(line: &str) -> BlocksLine {
    let indent = line
        .bytes()
        .take_while(|byte| matches!(byte, b' ' | b'\t'))
        .count();
    let Some(marker_end) = after_list_marker(line, indent) else {
        return BlocksLine::NotALine;
    };
    let mut rest = line[marker_end..].trim_start();

    let mut canonical_emoji = false;
    for emoji in TOLERATED_EMOJI {
        if let Some(after) = rest.strip_prefix(emoji) {
            canonical_emoji = *emoji == BLOCKS_EMOJI;
            rest = after.trim_start();
            break;
        }
    }

    let Some(body) = rest.strip_prefix("**") else {
        return BlocksLine::NotALine;
    };
    let Some(close) = body.find("**") else {
        return BlocksLine::NotALine;
    };
    let label_raw = &body[..close];
    let label = label_raw.strip_suffix(':').unwrap_or(label_raw);
    if label != BLOCKS_LABEL && label != "BLOCKING" {
        return BlocksLine::NotALine;
    }
    let canonical_label = label_raw == "BLOCKS:";
    let mut after = body[close + 2..].to_string();
    if let Some(stripped) = after.strip_prefix(':') {
        after = stripped.to_string();
    }
    if !after.is_empty() && !after.starts_with([' ', '\t']) {
        return BlocksLine::Malformed;
    }
    let label_end = line.len() - after.len();

    let spans = block_link_spans(line);
    let mut ranges = spans
        .iter()
        .map(|link| (link.full_start, link.full_end))
        .collect::<Vec<_>>();
    ranges.sort_unstable();
    let mut residue = String::new();
    let mut cursor = label_end;
    for (start, end) in &ranges {
        if *start < label_end {
            continue;
        }
        residue.push_str(&line[cursor..*start]);
        cursor = (*end).max(cursor);
    }
    residue.push_str(&line[cursor..]);
    if residue.contains("[[") || residue.contains("]]") {
        return BlocksLine::Malformed;
    }
    if !residue.chars().all(|character| {
        character.is_whitespace() || matches!(character, '•' | '·' | ',')
    }) {
        return BlocksLine::Malformed;
    }

    if spans.is_empty() {
        return if after.trim().is_empty() {
            BlocksLine::Empty
        } else {
            BlocksLine::Malformed
        };
    }

    let canonical_separators = canonical_gaps(line, label_end, &ranges);
    let links = spans
        .into_iter()
        .map(|link| BlocksLink {
            target: link.target,
            block_id: link.block_id,
            embedded: link.embedded,
            struck: link.struck,
            aliased: link.aliased,
        })
        .collect::<Vec<_>>();
    let mut ordered = links
        .iter()
        .map(BlocksLink::canonical_text)
        .collect::<Vec<_>>();
    let original_order = ordered.clone();
    sort_blocks_links(&mut ordered);
    let canonical_order = ordered == original_order;
    let unique = ordered.windows(2).all(|pair| pair[0] != pair[1]);
    let canonical_links = links
        .iter()
        .all(|link| !link.embedded && !link.struck && !link.aliased);

    BlocksLine::Accepted {
        links,
        canonical: canonical_emoji
            && canonical_label
            && canonical_separators
            && canonical_links
            && canonical_order
            && unique,
    }
}

fn canonical_gaps(
    line: &str,
    label_end: usize,
    ranges: &[(usize, usize)],
) -> bool {
    let relevant = ranges
        .iter()
        .filter(|(start, _)| *start >= label_end)
        .collect::<Vec<_>>();
    if relevant.is_empty() || &line[label_end..relevant[0].0] != " " {
        return false;
    }
    for pair in relevant.windows(2) {
        if &line[pair[0].1..pair[1].0] != BLOCKS_SEPARATOR {
            return false;
        }
    }
    line[relevant[relevant.len() - 1].1..].is_empty()
}

/// True for a task's direct-child BLOCKS lines, including malformed ones.
/// Fenced lines, descendants of child bullets, and non-task owners are
/// excluded. The returned children retain document order.
pub(crate) fn blocks_children_of(
    lines: &[&str],
    fenced: &BTreeSet<usize>,
    task_line: usize,
) -> Vec<BlocksChild> {
    let Some(owner) = lines.get(task_line) else {
        return Vec::new();
    };
    if !has_task_tag(owner) {
        return Vec::new();
    }
    let source_indent = leading_indentation_width(owner);
    let mut children = Vec::new();
    for line_index in task_line + 1..lines.len() {
        let line = lines[line_index];
        if line.trim().is_empty() || fenced.contains(&line_index) {
            continue;
        }
        if leading_indentation_width(line) <= source_indent {
            break;
        }
        if nearest_parent_list_item(lines, line_index) != Some(task_line) {
            continue;
        }
        let parsed = parse_blocks_line(line);
        if !matches!(parsed, BlocksLine::NotALine) {
            children.push(BlocksChild { line_index, parsed });
        }
    }
    children
}

fn has_task_tag(line: &str) -> bool {
    line.split_whitespace().any(|token| token == "#task")
}

pub(crate) fn sort_blocks_links(links: &mut [String]) {
    links.sort_by(|left, right| {
        ascii_fold(left)
            .cmp(&ascii_fold(right))
            .then_with(|| left.cmp(right))
    });
}

fn ascii_fold(value: &str) -> Vec<u8> {
    value
        .bytes()
        .map(|byte| byte.to_ascii_lowercase())
        .collect()
}

pub(crate) fn format_blocks_line(indent: &str, links: &[String]) -> String {
    format!(
        "{indent}- {BLOCKS_EMOJI} **{BLOCKS_LABEL}:** {}",
        links.join(BLOCKS_SEPARATOR)
    )
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BlocksInsertSlot {
    pub(crate) line_index: usize,
    pub(crate) indent: String,
}

/// Position for a missing BLOCKS child: after the Depends-On subtree,
/// otherwise in the first-child slot (second after a first-child Cancel
/// Log). The indent reuses an existing direct child's whitespace when
/// possible, falling back to one tab below the owner.
pub(crate) fn blocks_insert_slot(
    lines: &[&str],
    fenced: &BTreeSet<usize>,
    task_line: usize,
) -> BlocksInsertSlot {
    let owner = lines.get(task_line).copied().unwrap_or("");
    let parent_indent = leading_whitespace(owner).to_string();
    let fallback_indent = format!("{parent_indent}\t");
    let first_child = direct_child_lines(lines, fenced, task_line)
        .into_iter()
        .next();
    let child_indent = first_child
        .map(|index| leading_whitespace(lines[index]).to_string())
        .unwrap_or(fallback_indent);

    if let Some(dep) =
        super::parse::dependency_child_of(lines, fenced, task_line)
    {
        return BlocksInsertSlot {
            line_index: subtree_end(lines, dep.line_index),
            indent: child_indent,
        };
    }

    if let Some(first) = first_child {
        if is_cancel_log(lines[first]) {
            return BlocksInsertSlot {
                line_index: subtree_end(lines, first),
                indent: child_indent,
            };
        }
        return BlocksInsertSlot {
            line_index: first,
            indent: child_indent,
        };
    }

    BlocksInsertSlot {
        line_index: task_line + 1,
        indent: child_indent,
    }
}

fn direct_child_lines(
    lines: &[&str],
    fenced: &BTreeSet<usize>,
    task_line: usize,
) -> Vec<usize> {
    let Some(owner) = lines.get(task_line) else {
        return Vec::new();
    };
    let source_indent = leading_indentation_width(owner);
    (task_line + 1..lines.len())
        .take_while(|index| {
            lines[*index].trim().is_empty()
                || leading_indentation_width(lines[*index]) > source_indent
        })
        .filter(|index| {
            !fenced.contains(index)
                && nearest_parent_list_item(lines, *index) == Some(task_line)
        })
        .collect()
}

fn subtree_end(lines: &[&str], root: usize) -> usize {
    let root_indent = leading_indentation_width(lines[root]);
    let mut index = root + 1;
    while index < lines.len() {
        let line = lines[index];
        if line.trim().is_empty() {
            let next_child = lines[index + 1..]
                .iter()
                .find(|candidate| !candidate.trim().is_empty());
            if next_child.is_some_and(|candidate| {
                leading_indentation_width(candidate) > root_indent
            }) {
                index += 1;
                continue;
            }
            break;
        }
        if leading_indentation_width(line) <= root_indent {
            break;
        }
        index += 1;
    }
    index
}

fn is_cancel_log(line: &str) -> bool {
    let indent = line
        .bytes()
        .take_while(|byte| matches!(byte, b' ' | b'\t'))
        .count();
    let Some(marker_end) = after_list_marker(line, indent) else {
        return false;
    };
    let body = line[marker_end..].trim_start();
    body.starts_with('❌') && body.contains("CANCEL LOG")
}

fn leading_whitespace(line: &str) -> &str {
    let end = line
        .bytes()
        .take_while(|byte| matches!(byte, b' ' | b'\t'))
        .count();
    &line[..end]
}

/// Desired entries after a gesture delta. `resolve` maps each parsed link
/// to `(stable identity, canonical link text)`; additions carry the same
/// mapping, and removals name stable identities.
#[allow(dead_code)]
pub(crate) fn blocks_desired_after_delta<F>(
    existing: &[BlocksLink],
    mut resolve: F,
    add: &BTreeMap<String, String>,
    remove: &BTreeSet<String>,
) -> Vec<String>
where
    F: FnMut(&BlocksLink) -> Option<(String, String)>,
{
    let mut desired = BTreeMap::new();
    for link in existing {
        if let Some((identity, canonical_text)) = resolve(link)
            && !remove.contains(&identity)
        {
            desired.insert(identity, canonical_text);
        }
    }
    for (identity, canonical_text) in add {
        if !remove.contains(identity) {
            desired.insert(identity.clone(), canonical_text.clone());
        }
    }
    let mut links = desired.into_values().collect::<Vec<_>>();
    sort_blocks_links(&mut links);
    links
}

pub(crate) fn plan_blocks_line(
    lines: &[&str],
    fenced: &BTreeSet<usize>,
    owner_line: usize,
    desired: &BTreeSet<String>,
) -> BlocksLinePlan {
    let children = blocks_children_of(lines, fenced, owner_line);
    if children
        .iter()
        .any(|child| matches!(child.parsed, BlocksLine::Malformed))
    {
        return BlocksLinePlan::Malformed;
    }

    let mut desired_links = desired.iter().cloned().collect::<Vec<_>>();
    sort_blocks_links(&mut desired_links);
    let desired_set = desired_links.iter().cloned().collect::<BTreeSet<_>>();

    let existing_lines = children
        .iter()
        .filter_map(|child| match &child.parsed {
            BlocksLine::Accepted { links, canonical } => Some((
                child.line_index,
                links
                    .iter()
                    .map(BlocksLink::canonical_text)
                    .collect::<Vec<_>>(),
                *canonical,
            )),
            BlocksLine::Empty => Some((child.line_index, Vec::new(), false)),
            BlocksLine::Malformed | BlocksLine::NotALine => None,
        })
        .collect::<Vec<_>>();

    let mut current_set = BTreeSet::new();
    for (_, links, _) in &existing_lines {
        current_set.extend(links.iter().cloned());
    }
    let added = desired_set
        .difference(&current_set)
        .cloned()
        .collect::<Vec<_>>();
    let removed = current_set
        .difference(&desired_set)
        .cloned()
        .collect::<Vec<_>>();

    if desired_links.is_empty() {
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

    let Some((first_index, first_links, first_canonical)) =
        existing_lines.first()
    else {
        let slot = blocks_insert_slot(lines, fenced, owner_line);
        return BlocksLinePlan::Edits {
            edits: vec![BlocksEdit::Insert {
                line_index: slot.line_index,
                text: format_blocks_line(&slot.indent, &desired_links),
            }],
            report: BlocksLineReport {
                kind: BlocksUpdateKind::Inserted,
                added,
                removed,
            },
        };
    };

    let first_set = first_links.iter().cloned().collect::<BTreeSet<_>>();
    let exact_entries =
        first_set == desired_set && first_links.len() == desired_links.len();
    let has_duplicates =
        existing_lines.len() > 1 || first_links.len() != first_set.len();
    let canonical_line = format_blocks_line(
        leading_whitespace(lines[*first_index]),
        &desired_links,
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

#[cfg(test)]
#[path = "blocks_tests.rs"]
mod tests;
