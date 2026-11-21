//! Line-preserving text edits shared by the tidy planner and the req writers.
//!
//! Every edit works on physical lines with their own endings, so a mixed
//! LF/CRLF note keeps the endings of every line it does not touch, and a
//! note without a final newline keeps that state.

use super::super::capture::{document_line_ending, leading_spaces_or_tabs_len};

/// The physical lines of `contents`, each with its own ending.
pub(super) fn physical_lines(contents: &str) -> Vec<&str> {
    contents.split_inclusive('\n').collect()
}

/// `line` with a line ending, adding `ending` when the line has none.
pub(super) fn with_ending(line: &str, ending: &str) -> String {
    if line.ends_with('\n') {
        line.to_string()
    } else {
        format!("{line}{ending}")
    }
}

/// The text of a physical line without its ending.
pub(super) fn bare(line: &str) -> &str {
    let line = line.strip_suffix('\n').unwrap_or(line);
    line.strip_suffix('\r').unwrap_or(line)
}

/// Undo a final newline the edit added to a note that had none.
pub(super) fn restore_final_newline(original: &str, updated: String) -> String {
    if original.is_empty() || original.ends_with('\n') {
        return updated;
    }
    match updated
        .strip_suffix("\r\n")
        .or_else(|| updated.strip_suffix('\n'))
    {
        Some(stripped) => stripped.to_string(),
        None => updated,
    }
}

/// Replace the leading `old_indent` of a physical line with `new_indent`.
/// Blank lines and lines shallower than `old_indent` stay as they are.
pub(super) fn rebase_indent(
    line: &str,
    old_indent: &str,
    new_indent: &str,
) -> String {
    if bare(line).trim().is_empty()
        || leading_spaces_or_tabs_len(line) < old_indent.len()
    {
        return line.to_string();
    }
    format!("{new_indent}{}", &line[old_indent.len()..])
}

/// Insert whole physical lines at byte `offset` (a line start, or the end of
/// the note). A note that does not end in a newline gains one before the
/// insertion, so the lines never join.
pub(super) fn insert_lines(
    contents: &str,
    offset: usize,
    lines: &[String],
) -> String {
    let ending = document_line_ending(contents);
    let mut text = String::new();
    if offset > 0 && !contents[..offset].ends_with('\n') {
        text.push_str(ending);
    }
    for line in lines {
        text.push_str(&with_ending(line, ending));
    }
    let mut updated = String::with_capacity(contents.len() + text.len());
    updated.push_str(&contents[..offset]);
    updated.push_str(&text);
    updated.push_str(&contents[offset..]);
    updated
}

/// One range of physical lines cut out of a note.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Pulled {
    /// The cut lines, each with its own ending.
    pub(super) lines: Vec<String>,
    /// The indentation of the first cut line.
    pub(super) indent: String,
}

/// Cut inclusive 0-based line ranges (sorted, non-overlapping) out of a
/// note. Returns the remaining text and the cut lines per range, in order.
pub(super) fn pull_ranges(
    contents: &str,
    ranges: &[(usize, usize)],
) -> (String, Vec<Pulled>) {
    let physical = physical_lines(contents);
    let mut remaining = String::with_capacity(contents.len());
    let mut pulled = Vec::with_capacity(ranges.len());
    let mut next = 0;
    for &(start, end) in ranges {
        for line in &physical[next..start] {
            remaining.push_str(line);
        }
        let cut: Vec<String> = physical[start..=end]
            .iter()
            .map(|line| (*line).to_string())
            .collect();
        let indent_len = leading_spaces_or_tabs_len(&cut[0]);
        pulled.push(Pulled {
            indent: cut[0][..indent_len].to_string(),
            lines: cut,
        });
        next = end + 1;
    }
    for line in &physical[next..] {
        remaining.push_str(line);
    }
    (remaining, pulled)
}

/// The pulled lines re-indented under `new_indent`.
pub(super) fn rebased(pulled: &Pulled, new_indent: &str) -> Vec<String> {
    pulled
        .lines
        .iter()
        .map(|line| rebase_indent(line, &pulled.indent, new_indent))
        .collect()
}
