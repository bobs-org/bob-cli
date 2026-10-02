//! Depends-On line parser (`docs/task-dependencies.md` §2).
//!
//! The parse algorithm finds the block links first with the shared link
//! scanner, then checks what surrounds them — it never splits on
//! separators, because an alias can contain one. After removing the
//! block links and the separators between them, the line must hold
//! exactly the emoji (or nothing), the label, and whitespace; anything
//! else makes the line malformed.

use std::collections::BTreeSet;

use super::super::task_status_hooks::{
    after_list_marker, leading_indentation_width, nearest_parent_list_item,
};
use super::block_link_spans;

/// Parse verdict for one candidate Depends-On line.
///
/// `line` is the candidate line on its own; `context` (fenced code,
/// nesting, Work Log ancestry) is decided by [`dependency_child_of`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DependencyLine {
    /// A line with `n` targets (`links.len()`).
    Accepted {
        links: Vec<DependencyLink>,
        /// Whether the line already matches the writer form.
        canonical: bool,
    },
    /// Label but no links (R9).
    Empty,
    /// Label-like but with trailing prose, half-typed links, bare note
    /// links, or other residue (R10).
    Malformed,
    /// Not a Depends-On line at all.
    NotALine,
}

/// One prerequisite link on a Depends-On line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DependencyLink {
    pub(crate) target: String,
    pub(crate) block_id: String,
    pub(crate) embedded: bool,
    pub(crate) struck: bool,
    pub(crate) aliased: bool,
}

/// A Depends-On line found under its owning task.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DependencyChild {
    pub(crate) line_index: usize,
    pub(crate) parsed: DependencyLine,
}

const CHAIN_EMOJI_CANONICAL: &str = "⛓️";
const CHAIN_EMOJI_TOLERATED: &[&str] = &["⛓️", "⛓", "🔗"];

/// Whether a full Markdown line is a Depends-On line in any form —
/// accepted, empty, or malformed. Guards (capture close, section titles,
/// managed-log parsers) use this to leave the line alone.
pub(crate) fn is_dependency_line(line: &str) -> bool {
    !matches!(parse_dependency_line(line), DependencyLine::NotALine)
}

/// Parse one candidate Depends-On line on its own
/// (`docs/task-dependencies.md` §11.1 DP vectors).
pub(crate) fn parse_dependency_line(line: &str) -> DependencyLine {
    let indent = line
        .bytes()
        .take_while(|byte| matches!(byte, b' ' | b'\t'))
        .count();
    let Some(marker_end) = after_list_marker(line, indent) else {
        return DependencyLine::NotALine;
    };
    let mut rest = line[marker_end..].trim_start();

    let mut canonical_emoji = false;
    for emoji in CHAIN_EMOJI_TOLERATED {
        if let Some(after) = rest.strip_prefix(emoji) {
            canonical_emoji = *emoji == CHAIN_EMOJI_CANONICAL;
            rest = after.trim_start();
            break;
        }
    }

    let Some(body) = rest.strip_prefix("**") else {
        return DependencyLine::NotALine;
    };
    let Some(close) = body.find("**") else {
        return DependencyLine::NotALine;
    };
    let label_raw = &body[..close];
    let label = label_raw.strip_suffix(':').unwrap_or(label_raw);
    if label != "DEPENDS ON" && label != "DEPENDENCIES" {
        return DependencyLine::NotALine;
    }
    let canonical_label = label_raw == "DEPENDS ON:";
    let mut after = body[close + 2..].to_string();
    if let Some(stripped) = after.strip_prefix(':') {
        after = stripped.to_string();
    }
    if !after.is_empty() && !after.starts_with([' ', '\t']) {
        return DependencyLine::Malformed;
    }
    let label_end = line.len() - after.len();

    let links = block_link_spans(line);
    let mut spans = links
        .iter()
        .map(|link| (link.full_start, link.full_end))
        .collect::<Vec<_>>();
    spans.sort();
    let mut residue = String::new();
    let mut cursor = label_end;
    for (start, end) in &spans {
        if *start < label_end {
            continue;
        }
        residue.push_str(&line[cursor..*start]);
        cursor = (*end).max(cursor);
    }
    residue.push_str(&line[cursor..]);
    if residue.contains("[[") || residue.contains("]]") {
        return DependencyLine::Malformed;
    }
    if !residue.chars().all(|character| {
        character.is_whitespace() || matches!(character, '•' | '·' | ',')
    }) {
        return DependencyLine::Malformed;
    }
    if links.is_empty() {
        if after.trim().is_empty() {
            return DependencyLine::Empty;
        }
        return DependencyLine::Malformed;
    }
    let canonical_separators = canonical_gaps(line, label_end, &spans);
    let canonical_links = links
        .iter()
        .all(|link| !link.embedded && !link.struck && !link.aliased);
    DependencyLine::Accepted {
        links: links
            .into_iter()
            .map(|link| DependencyLink {
                target: link.target,
                block_id: link.block_id,
                embedded: link.embedded,
                struck: link.struck,
                aliased: link.aliased,
            })
            .collect(),
        canonical: canonical_emoji
            && canonical_label
            && canonical_separators
            && canonical_links,
    }
}

/// Whether the gaps around the link spans use exactly the writer-form
/// separators: one space before the first link, ` • ` between links,
/// and nothing after the last link.
fn canonical_gaps(
    line: &str,
    label_end: usize,
    spans: &[(usize, usize)],
) -> bool {
    let mut relevant = spans
        .iter()
        .filter(|(start, _)| *start >= label_end)
        .collect::<Vec<_>>();
    relevant.sort();
    if relevant.is_empty() {
        return false;
    }
    if &line[label_end..relevant[0].0] != " " {
        return false;
    }
    for pair in relevant.windows(2) {
        if &line[pair[0].1..pair[1].0] != " • " {
            return false;
        }
    }
    line[relevant[relevant.len() - 1].1..].is_empty()
}

/// First direct-child Depends-On line under `task_line`
/// (`docs/task-dependencies.md` §2.3 step 3).
///
/// Reuses the hooks' [`nearest_parent_list_item`], skipping fenced code.
/// Grandchildren, fenced lines, and lines nested under another child
/// (for example a Work Log entry) never count; a direct child in any
/// later position still counts.
pub(crate) fn dependency_child_of(
    lines: &[&str],
    fenced: &BTreeSet<usize>,
    task_line: usize,
) -> Option<DependencyChild> {
    let source_indent = leading_indentation_width(lines[task_line]);
    for line_index in task_line + 1..lines.len() {
        let line = lines[line_index];
        if line.trim().is_empty() {
            continue;
        }
        if fenced.contains(&line_index) {
            continue;
        }
        if leading_indentation_width(line) <= source_indent {
            break;
        }
        if nearest_parent_list_item(lines, line_index) != Some(task_line) {
            continue;
        }
        match parse_dependency_line(line) {
            DependencyLine::NotALine => continue,
            parsed => {
                return Some(DependencyChild { line_index, parsed });
            }
        }
    }
    None
}
