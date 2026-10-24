//! LINKS and LINKED FROM line parsers (`docs/task-relations.md` §3).
//!
//! Block links are found first with the shared scanner so aliases,
//! embeds, and strikes never split on separators. Labels live in the
//! gaps: before each LINKS link, and in `(…)` after each LINKED FROM
//! link.

use std::{collections::BTreeSet, ops::Range};

use super::super::task_dependencies::{
    block_link_spans, blocks::has_task_tag, BlockLinkSpan,
};
use super::super::task_status_hooks::{
    after_list_marker, leading_indentation_width, nearest_parent_list_item,
};
use super::label::parse_relation_label;
use super::{
    LinkedFromLine, LinksLine, RelationEntry, BACKLINKS_LABEL, KNOT_EMOJI,
    LINKED_FROM_LABEL, LINKS_LABEL, LINKS_TO_LABEL, TOLERATED_EMOJI,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LineKind {
    Links,
    LinkedFrom,
}

struct Header {
    canonical_emoji: bool,
    canonical_label: bool,
    label_end: usize,
    kind: LineKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LinksChild {
    pub(crate) line_index: usize,
    pub(crate) parsed: LinksLine,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LinkedFromChild {
    pub(crate) line_index: usize,
    pub(crate) parsed: LinkedFromLine,
}

/// The source span and parsed value of one relation label on a managed
/// relation line. Spans are byte ranges local to the line, so callers can
/// replace only label text while preserving every other authored byte.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RelationLabelSpan {
    pub(crate) range: Range<usize>,
    pub(crate) label: String,
}

pub(crate) fn parse_links_line(line: &str) -> LinksLine {
    match parse_header(line) {
        None => LinksLine::NotALine,
        Some(Err(())) => LinksLine::Malformed,
        Some(Ok(header)) if header.kind != LineKind::Links => {
            LinksLine::NotALine
        }
        Some(Ok(header)) => match parse_links_entries(line, &header) {
            None => LinksLine::Malformed,
            Some(entries) if entries.is_empty() => LinksLine::Empty,
            Some(entries) => {
                let canonical = header.canonical_emoji
                    && header.canonical_label
                    && unique_identities(&entries)
                    && line == super::format::format_links_line(
                        super::super::task_dependencies::blocks::leading_whitespace(
                            line,
                        ),
                        &display_linked_from(&entries),
                    );
                LinksLine::Accepted { entries, canonical }
            }
        },
    }
}

pub(crate) fn parse_linked_from_line(line: &str) -> LinkedFromLine {
    match parse_header(line) {
        None => LinkedFromLine::NotALine,
        Some(Err(())) => LinkedFromLine::Malformed,
        Some(Ok(header)) if header.kind != LineKind::LinkedFrom => {
            LinkedFromLine::NotALine
        }
        Some(Ok(header)) => match parse_linked_from_entries(line, &header) {
            None => LinkedFromLine::Malformed,
            Some(entries) if entries.is_empty() => LinkedFromLine::Empty,
            Some(entries) => {
                let canonical = header.canonical_emoji
                    && header.canonical_label
                    && unique_identities(&entries)
                    && line
                        == super::format::format_linked_from_line(
                            super::super::task_dependencies::blocks::leading_whitespace(
                                line,
                            ),
                            &display_linked_from(&entries),
                        );
                LinkedFromLine::Accepted { entries, canonical }
            }
        },
    }
}

/// Direct-child LINKS lines under a `#task`, including malformed ones.
pub(crate) fn links_children_of(
    lines: &[&str],
    fenced: &BTreeSet<usize>,
    task_line: usize,
) -> Vec<LinksChild> {
    relation_children(lines, fenced, task_line, |line| {
        let parsed = parse_links_line(line);
        (!matches!(parsed, LinksLine::NotALine)).then_some(parsed)
    })
    .into_iter()
    .map(|(line_index, parsed)| LinksChild { line_index, parsed })
    .collect()
}

/// Direct-child LINKED FROM lines under a `#task`, including malformed.
pub(crate) fn linked_from_children_of(
    lines: &[&str],
    fenced: &BTreeSet<usize>,
    task_line: usize,
) -> Vec<LinkedFromChild> {
    relation_children(lines, fenced, task_line, |line| {
        let parsed = parse_linked_from_line(line);
        (!matches!(parsed, LinkedFromLine::NotALine)).then_some(parsed)
    })
    .into_iter()
    .map(|(line_index, parsed)| LinkedFromChild { line_index, parsed })
    .collect()
}

/// Relation-label spans on an accepted LINKS line. `None` means the line
/// is not an accepted LINKS line; an empty vector means it is valid and has
/// no labels.
pub(crate) fn links_label_spans(line: &str) -> Option<Vec<RelationLabelSpan>> {
    let Some(Ok(header)) = parse_header(line) else {
        return None;
    };
    if header.kind != LineKind::Links {
        return None;
    }
    parse_links_entries(line, &header)?;
    let spans = relevant_spans(line, header.label_end);
    let mut labels = Vec::new();
    let mut previous_end = header.label_end;
    for (index, span) in spans.iter().enumerate() {
        let gap = &line[previous_end..span.full_start];
        let (raw, base) = if index == 0 {
            (gap, previous_end)
        } else {
            let raw = skip_one_separator(gap);
            (raw, previous_end + gap.len() - raw.len())
        };
        if let Some(label) = parsed_label_span(raw, base) {
            labels.push(label);
        } else if let Some(label) = alias_label_span(line, span) {
            labels.push(label);
        }
        previous_end = span.full_end;
    }
    Some(labels)
}

/// Relation-label spans on an accepted LINKED FROM line. `None` means the
/// line is not an accepted LINKED FROM line; an empty vector means it is
/// valid and has no labels.
pub(crate) fn linked_from_label_spans(
    line: &str,
) -> Option<Vec<RelationLabelSpan>> {
    let Some(Ok(header)) = parse_header(line) else {
        return None;
    };
    if header.kind != LineKind::LinkedFrom {
        return None;
    }
    parse_linked_from_entries(line, &header)?;
    let spans = relevant_spans(line, header.label_end);
    let mut labels = Vec::new();
    for (index, span) in spans.iter().enumerate() {
        let end = spans
            .get(index + 1)
            .map_or(line.len(), |next| next.full_start);
        let gap = &line[span.end..end];
        if let Some(label) = parenthesized_label_span(gap, span.end) {
            labels.push(label);
        } else if let Some(label) = alias_label_span(line, span) {
            labels.push(label);
        }
    }
    Some(labels)
}

fn parsed_label_span(raw: &str, base: usize) -> Option<RelationLabelSpan> {
    let start = raw.len() - raw.trim_start().len();
    let trimmed = raw.trim();
    let label = parse_relation_label(trimmed).ok()??;
    Some(RelationLabelSpan {
        range: base + start..base + start + trimmed.len(),
        label,
    })
}

fn parenthesized_label_span(
    gap: &str,
    base: usize,
) -> Option<RelationLabelSpan> {
    let leading = gap.len() - gap.trim_start().len();
    let trimmed = gap.trim_start();
    let inner = trimmed.strip_prefix('(')?;
    let close = inner.find(')')?;
    let raw = &inner[..close];
    parsed_label_span(raw, base + leading + 1)
}

fn alias_label_span(
    line: &str,
    span: &BlockLinkSpan,
) -> Option<RelationLabelSpan> {
    if !span.aliased {
        return None;
    }
    let inside_start = span.open + 2;
    let inside = &line[inside_start..span.end - 2];
    let (target, alias) = inside.split_once('|')?;
    parsed_label_span(alias, inside_start + target.len() + 1)
}

fn relation_children<T>(
    lines: &[&str],
    fenced: &BTreeSet<usize>,
    task_line: usize,
    mut parse: impl FnMut(&str) -> Option<T>,
) -> Vec<(usize, T)> {
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
        if let Some(parsed) = parse(line) {
            children.push((line_index, parsed));
        }
    }
    children
}

fn parse_header(line: &str) -> Option<Result<Header, ()>> {
    let indent = line
        .bytes()
        .take_while(|byte| matches!(byte, b' ' | b'\t'))
        .count();
    let marker_end = after_list_marker(line, indent)?;
    let mut rest = line[marker_end..].trim_start();

    let mut canonical_emoji = false;
    for emoji in TOLERATED_EMOJI {
        if let Some(after) = rest.strip_prefix(emoji) {
            canonical_emoji = *emoji == KNOT_EMOJI;
            rest = after.trim_start();
            break;
        }
    }

    let body = rest.strip_prefix("**")?;
    let close = body.find("**")?;
    let label_raw = &body[..close];
    let label = label_raw.strip_suffix(':').unwrap_or(label_raw);
    let (kind, canonical_label) = if label == LINKS_LABEL {
        (LineKind::Links, label_raw == "LINKS:")
    } else if label == LINKS_TO_LABEL {
        (LineKind::Links, false)
    } else if label == LINKED_FROM_LABEL {
        (LineKind::LinkedFrom, label_raw == "LINKED FROM:")
    } else if label == BACKLINKS_LABEL {
        (LineKind::LinkedFrom, false)
    } else {
        return None;
    };
    let mut after = body[close + 2..].to_string();
    if let Some(stripped) = after.strip_prefix(':') {
        after = stripped.to_string();
    }
    if !after.is_empty() && !after.starts_with([' ', '\t']) {
        return Some(Err(()));
    }
    let label_end = line.len() - after.len();
    Some(Ok(Header {
        canonical_emoji,
        canonical_label,
        label_end,
        kind,
    }))
}

fn parse_links_entries(
    line: &str,
    header: &Header,
) -> Option<Vec<RelationEntry>> {
    let spans = relevant_spans(line, header.label_end);
    if residue_has_brackets(line, header.label_end, &spans) {
        return None;
    }
    if spans.is_empty() {
        let after = &line[header.label_end..];
        return if after.trim().is_empty() {
            Some(Vec::new())
        } else {
            None
        };
    }
    let mut entries = Vec::new();
    let mut prev_end = header.label_end;
    for (index, span) in spans.iter().enumerate() {
        let gap = &line[prev_end..span.full_start];
        let from_gap = if index == 0 {
            parse_relation_label(gap).ok()?
        } else {
            parse_relation_label(skip_one_separator(gap)).ok()?
        };
        let label = match (from_gap, alias_of(line, span)) {
            (Some(label), _) => Some(label),
            (None, Some(alias)) => parse_relation_label(&alias).ok()?,
            (None, None) => None,
        };
        entries.push(entry_from_span(span, label));
        prev_end = span.full_end;
    }
    let trailing = &line[prev_end..];
    if !is_separator_residue(trailing) {
        return None;
    }
    Some(entries)
}

fn parse_linked_from_entries(
    line: &str,
    header: &Header,
) -> Option<Vec<RelationEntry>> {
    let spans = relevant_spans(line, header.label_end);
    if residue_has_brackets(line, header.label_end, &spans) {
        return None;
    }
    if spans.is_empty() {
        let after = &line[header.label_end..];
        return if after.trim().is_empty() {
            Some(Vec::new())
        } else {
            None
        };
    }
    let leading = &line[header.label_end..spans[0].full_start];
    if !leading.chars().all(char::is_whitespace) {
        return None;
    }
    let mut entries = Vec::new();
    for (index, span) in spans.iter().enumerate() {
        let end = spans
            .get(index + 1)
            .map_or(line.len(), |next| next.full_start);
        let gap = &line[span.full_end..end];
        let is_last = index + 1 == spans.len();
        let label = parse_linked_from_gap(gap, is_last)?;
        let label = match (label, alias_of(line, span)) {
            (Some(label), _) => Some(label),
            (None, Some(alias)) => parse_relation_label(&alias).ok()?,
            (None, None) => None,
        };
        entries.push(entry_from_span(span, label));
    }
    Some(entries)
}

fn parse_linked_from_gap(gap: &str, is_last: bool) -> Option<Option<String>> {
    let mut rest = gap.trim_start();
    let mut label = None;
    if let Some(inner) = rest.strip_prefix('(') {
        let close = inner.find(')')?;
        let inside = &inner[..close];
        if inside.contains('(') {
            return None;
        }
        label = parse_relation_label(inside).ok()?;
        label.as_ref()?;
        rest = inner[close + 1..].trim_start();
    }
    if is_last {
        return is_separator_residue(rest).then_some(label);
    }
    rest = skip_one_separator(rest);
    rest.chars().all(char::is_whitespace).then_some(label)
}

fn relevant_spans(line: &str, label_end: usize) -> Vec<BlockLinkSpan> {
    block_link_spans(line)
        .into_iter()
        .filter(|span| span.full_start >= label_end)
        .collect()
}

fn residue_has_brackets(
    line: &str,
    label_end: usize,
    spans: &[BlockLinkSpan],
) -> bool {
    let mut residue = String::new();
    let mut cursor = label_end;
    for span in spans {
        residue.push_str(&line[cursor..span.full_start]);
        cursor = span.full_end.max(cursor);
    }
    residue.push_str(&line[cursor..]);
    residue.contains("[[") || residue.contains("]]")
}

fn skip_one_separator(gap: &str) -> &str {
    let rest = gap.trim_start();
    let rest = rest
        .strip_prefix('•')
        .or_else(|| rest.strip_prefix('·'))
        .or_else(|| rest.strip_prefix(','))
        .unwrap_or(rest);
    rest.trim_start()
}

fn is_separator_residue(text: &str) -> bool {
    text.chars().all(|character| {
        character.is_whitespace() || matches!(character, '•' | '·' | ',')
    })
}

fn alias_of(line: &str, span: &BlockLinkSpan) -> Option<String> {
    if !span.aliased {
        return None;
    }
    let inside = &line[span.open + 2..span.end - 2];
    inside.split_once('|').map(|(_, alias)| alias.to_string())
}

fn entry_from_span(
    span: &BlockLinkSpan,
    label: Option<String>,
) -> RelationEntry {
    RelationEntry {
        target: span.target.clone(),
        block_id: span.block_id.clone(),
        label,
        embedded: span.embedded,
        struck: span.struck,
        aliased: span.aliased,
    }
}

fn unique_identities(entries: &[RelationEntry]) -> bool {
    let mut seen = BTreeSet::new();
    entries
        .iter()
        .all(|entry| seen.insert(entry.canonical_text()))
}

fn display_linked_from(
    entries: &[RelationEntry],
) -> Vec<(String, Option<String>)> {
    entries
        .iter()
        .map(|entry| (entry.canonical_text(), entry.label.clone()))
        .collect()
}

pub(crate) fn is_subprojects_line(line: &str) -> bool {
    let indent = line
        .bytes()
        .take_while(|byte| matches!(byte, b' ' | b'\t'))
        .count();
    let Some(marker_end) = after_list_marker(line, indent) else {
        return false;
    };
    let body = line[marker_end..].trim_start();
    let body = body
        .strip_prefix("🧩")
        .map(|rest| rest.trim_start())
        .unwrap_or(body);
    body.contains("**Sub-projects:**") || body.contains("**Sub-projects**")
}
