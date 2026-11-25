//! `## Snoozed` section bounds and row grammar.
//!
//! Contract: `docs/snooze.md` § Row grammar. The first real `## Snoozed`
//! section ends at the next real H2 or EOF. Frontmatter and fenced headings
//! never open or close the section. Later duplicate sections are linted and
//! never acted on. Canonical active rows are column-zero
//! `- **HHMM** [[note#^id]]`; `HH:MM` and `H:MM` inside the bold are read
//! but writers emit four digits. History is
//! `- ~~**HHMM** [[note#^id]]~~ · woke HH:MM`. Children are the link's
//! authored notes and travel as a subtree.

use std::ops::Range;

use super::super::{markdown, task_dependencies::block_link_spans};

/// Zero-based line index of the first real `## Snoozed` header, if any.
pub(crate) fn first_snoozed_header(lines: &[&str]) -> Option<usize> {
    let frontmatter_end = markdown::strictly_closed_frontmatter_end(lines);
    let mut fence = None;
    for (index, line) in lines.iter().enumerate() {
        if frontmatter_end.is_some_and(|end| index <= end) {
            continue;
        }
        if let Some(open_fence) = fence {
            if markdown::closes_fence(line, open_fence) {
                fence = None;
            }
            continue;
        }
        if let Some(open_fence) = markdown::fence_marker(line) {
            fence = Some(open_fence);
            continue;
        }
        if is_snoozed_heading(line) {
            return Some(index);
        }
    }
    None
}

/// Zero-based line indexes of every real `## Snoozed` header, in order.
/// The first opens the section; the rest are duplicate-section lints.
pub(crate) fn duplicate_snoozed_headers(lines: &[&str]) -> Vec<usize> {
    let frontmatter_end = markdown::strictly_closed_frontmatter_end(lines);
    let mut fence = None;
    let mut headers = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        if frontmatter_end.is_some_and(|end| index <= end) {
            continue;
        }
        if let Some(open_fence) = fence {
            if markdown::closes_fence(line, open_fence) {
                fence = None;
            }
            continue;
        }
        if let Some(open_fence) = markdown::fence_marker(line) {
            fence = Some(open_fence);
            continue;
        }
        if is_snoozed_heading(line) {
            headers.push(index);
        }
    }
    headers
}

/// Content range (header-exclusive) of the first real `## Snoozed`
/// section: header line + 1 up to the next real H2 or EOF. `None` when the
/// note holds no real Snoozed heading.
pub(crate) fn snoozed_section_range(lines: &[&str]) -> Option<Range<usize>> {
    let header = first_snoozed_header(lines)?;
    let frontmatter_end = markdown::strictly_closed_frontmatter_end(lines);
    let mut fence = None;
    for (index, line) in lines.iter().enumerate().skip(header + 1) {
        if frontmatter_end.is_some_and(|end| index <= end) {
            continue;
        }
        if let Some(open_fence) = fence {
            if markdown::closes_fence(line, open_fence) {
                fence = None;
            }
            continue;
        }
        if let Some(open_fence) = markdown::fence_marker(line) {
            fence = Some(open_fence);
            continue;
        }
        if is_level_two_heading(line) {
            return Some(header + 1..index);
        }
    }
    Some(header + 1..lines.len())
}

/// Whether `line` is the real `## Snoozed` heading: exactly `##`,
/// whitespace, then `Snoozed`, then end or whitespace. Case-sensitive.
/// Like [`super::super::pomodoro::is_pomodoro_heading`], trailing text
/// after the name still opens the section; a bare `## Snoozed` is the
/// canonical writer form.
pub(crate) fn is_snoozed_heading(line: &str) -> bool {
    let Some(rest) = line.strip_prefix("##") else {
        return false;
    };
    let Some(rest) = trim_one_or_more_spaces(rest) else {
        return false;
    };
    let Some(rest) = rest.strip_prefix("Snoozed") else {
        return false;
    };
    rest.is_empty() || rest.starts_with(char::is_whitespace)
}

fn is_level_two_heading(line: &str) -> bool {
    line.strip_prefix("##")
        .and_then(trim_one_or_more_spaces)
        .is_some()
}

fn trim_one_or_more_spaces(value: &str) -> Option<&str> {
    let trimmed = value.trim_start();
    (trimmed.len() < value.len()).then_some(trimmed)
}

/// A parsed Snoozed section: header position, content range, rows in
/// source order, and duplicate-header lint positions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SnoozedSection {
    /// Zero-based header line of the first real section.
    pub(crate) header_line: usize,
    /// One-based header line number (vault/JSON convention).
    pub(crate) header_number: usize,
    /// Header-exclusive content range as zero-based line indexes.
    pub(crate) content: Range<usize>,
    pub(crate) rows: Vec<SnoozeRow>,
    /// Zero-based line indexes of later duplicate `## Snoozed` headers.
    pub(crate) duplicate_headers: Vec<usize>,
    /// One-based line numbers of later duplicate headers.
    pub(crate) duplicate_numbers: Vec<usize>,
}

/// One parsed row: active, history, or malformed-but-row-like.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SnoozeRow {
    /// Zero-based line index of the row bullet.
    pub(crate) line_index: usize,
    /// One-based line number of the row bullet.
    pub(crate) line: usize,
    pub(crate) kind: SnoozeRowKind,
    /// Target minutes since midnight for active rows and history rows.
    pub(crate) minutes: Option<u16>,
    /// Four-digit writer form of the target (`1530`).
    pub(crate) hhmm: Option<String>,
    /// Link target text before `#^` (may be empty for same-note links).
    pub(crate) target: Option<String>,
    /// Block id after `#^`.
    pub(crate) block_id: Option<String>,
    pub(crate) embedded: bool,
    pub(crate) aliased: bool,
    /// Zero-based exclusive end of the row's note subtree (children are
    /// `line_index + 1 .. children_end`).
    pub(crate) children_end: usize,
    /// Malformed reason for [`SnoozeRowKind::Malformed`] rows.
    pub(crate) malformed_reason: Option<&'static str>,
    /// Woke minutes since midnight for history rows.
    pub(crate) woke_minutes: Option<u16>,
    /// Woke `HH:MM` text for history rows.
    pub(crate) woke_hhmm: Option<String>,
}

/// Row shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SnoozeRowKind {
    Active,
    History,
    Malformed,
}

/// Parse the first real `## Snoozed` section of `lines` (logical lines
/// without endings). Returns `None` when the note holds no real heading.
/// Fenced and frontmatter headings never count; later duplicates are
/// reported in [`SnoozedSection::duplicate_headers`] and never parsed.
pub(crate) fn parse_section(lines: &[&str]) -> Option<SnoozedSection> {
    let header_line = first_snoozed_header(lines)?;
    let content = snoozed_section_range(lines)?;
    let all = duplicate_snoozed_headers(lines);
    let duplicates: Vec<usize> = all
        .into_iter()
        .filter(|index| *index != header_line)
        .collect();
    let fenced = markdown::fenced_lines(lines, content.clone());
    let mut rows = Vec::new();
    let mut index = content.start;
    while index < content.end {
        if fenced.contains(&index) {
            index += 1;
            continue;
        }
        let Some(row) = parse_row_at(lines, index, content.end, &fenced) else {
            index += 1;
            continue;
        };
        let end = row.children_end;
        rows.push(row);
        // `children_end` is exclusive, so it is already the next line
        // after this subtree. `max(index + 1)` still advances when a
        // malformed row reports `children_end == line_index`.
        index = end.max(index + 1);
    }
    Some(SnoozedSection {
        header_line,
        header_number: header_line + 1,
        content,
        rows,
        duplicate_numbers: duplicates.iter().map(|line| line + 1).collect(),
        duplicate_headers: duplicates,
    })
}

/// Parse one row at `index` when the line is row-like. Returns `None` for
/// plain prose (not a bullet at all). Indented row-like bullets parse as
/// [`SnoozeRowKind::Malformed`] with reason `indented` so readers report
/// them and never execute them.
fn parse_row_at(
    lines: &[&str],
    index: usize,
    section_end: usize,
    fenced: &std::collections::BTreeSet<usize>,
) -> Option<SnoozeRow> {
    let line = lines.get(index)?;
    let trimmed_end = line.trim_end();
    // Indented row-like text: report, never execute.
    if line.starts_with(' ') || line.starts_with('\t') {
        let after_indent = line.trim_start();
        if looks_row_like(after_indent) {
            return Some(malformed_row(
                index,
                section_end,
                lines,
                fenced,
                "indented",
            ));
        }
        return None;
    }
    if looks_row_like(trimmed_end) {
        return Some(parse_column_zero_row(index, section_end, lines, fenced));
    }
    None
}

/// Whether the column-zero line opens a `- ` bullet with bold or struck
/// content: the cheap row-like gate shared by active and history rows.
fn looks_row_like(line: &str) -> bool {
    let Some(rest) = line.strip_prefix('-') else {
        return false;
    };
    let Some(rest) = rest.strip_prefix([' ', '\t']) else {
        return false;
    };
    rest.starts_with("**") || rest.starts_with("~~")
}

fn children_end(
    lines: &[&str],
    row: usize,
    section_end: usize,
    fenced: &std::collections::BTreeSet<usize>,
) -> usize {
    let mut end = row + 1;
    while end < section_end {
        if fenced.contains(&end) {
            break;
        }
        let line = lines.get(end).map_or("", |line| *line);
        if line.trim().is_empty() {
            // A blank line belongs to the subtree only when a deeper
            // indented line follows before the next column-zero line.
            let mut probe = end + 1;
            let mut belongs = false;
            while probe < section_end {
                let next = lines.get(probe).map_or("", |line| *line);
                if next.trim().is_empty() {
                    probe += 1;
                    continue;
                }
                belongs = next.starts_with(' ') || next.starts_with('\t');
                break;
            }
            if belongs {
                end += 1;
                continue;
            }
            break;
        }
        if line.starts_with(' ') || line.starts_with('\t') {
            end += 1;
            continue;
        }
        break;
    }
    end
}

fn malformed_row(
    index: usize,
    section_end: usize,
    lines: &[&str],
    fenced: &std::collections::BTreeSet<usize>,
    reason: &'static str,
) -> SnoozeRow {
    SnoozeRow {
        line_index: index,
        line: index + 1,
        kind: SnoozeRowKind::Malformed,
        minutes: None,
        hhmm: None,
        target: None,
        block_id: None,
        embedded: false,
        aliased: false,
        children_end: children_end(lines, index, section_end, fenced),
        malformed_reason: Some(reason),
        woke_minutes: None,
        woke_hhmm: None,
    }
}

/// Parse a column-zero row-like bullet into its final shape.
fn parse_column_zero_row(
    index: usize,
    section_end: usize,
    lines: &[&str],
    fenced: &std::collections::BTreeSet<usize>,
) -> SnoozeRow {
    let line = lines.get(index).map_or("", |line| *line);
    let end = children_end(lines, index, section_end, fenced);
    let base = SnoozeRow {
        line_index: index,
        line: index + 1,
        kind: SnoozeRowKind::Malformed,
        minutes: None,
        hhmm: None,
        target: None,
        block_id: None,
        embedded: false,
        aliased: false,
        children_end: end,
        malformed_reason: Some("malformed"),
        woke_minutes: None,
        woke_hhmm: None,
    };
    let Some(rest) = line.strip_prefix('-') else {
        return base;
    };
    let rest = rest.strip_prefix([' ', '\t']).unwrap_or(rest);
    if let Some(history) = parse_history_rest(rest, index, end) {
        return history;
    }
    if let Some(active) = parse_active_rest(rest, index, end) {
        return active;
    }
    base
}

/// Parse active rest after `- `: `**TIME** <one block link>` and nothing
/// else. `HH:MM`/`H:MM` inside the bold are accepted on read.
fn parse_active_rest(
    rest: &str,
    index: usize,
    end: usize,
) -> Option<SnoozeRow> {
    let bold = parse_bold_time(rest)?;
    let after = rest[bold.consumed..].trim_start();
    // Link starts exactly here: find the single block link and require
    // nothing else on the line.
    let spans = block_link_spans(after);
    if spans.len() != 1 {
        return None;
    }
    let span = &spans[0];
    if after[..span.token_start].trim().is_empty()
        && after[span.end..].trim().is_empty()
    {
        let minutes = wall_minutes(&bold.text)?;
        return Some(SnoozeRow {
            line_index: index,
            line: index + 1,
            kind: SnoozeRowKind::Active,
            minutes: Some(minutes),
            hhmm: Some(format!("{:02}{:02}", minutes / 60, minutes % 60)),
            target: Some(span.target.clone()),
            block_id: Some(span.block_id.clone()),
            embedded: span.embedded,
            aliased: span.aliased,
            children_end: end,
            malformed_reason: None,
            woke_minutes: None,
            woke_hhmm: None,
        });
    }
    None
}

struct BoldTime {
    text: String,
    consumed: usize,
}

/// Parse a leading `**...**` time token; `consumed` is its byte length.
fn parse_bold_time(rest: &str) -> Option<BoldTime> {
    let inner = rest.strip_prefix("**")?;
    let close = inner.find("**")?;
    let text = inner[..close].to_string();
    Some(BoldTime {
        text,
        consumed: 4 + close,
    })
}

/// Validate a read-time wall text (`1530`, `15:30`, `H:MM`) into minutes.
/// Writers emit four digits; readers also accept the colon forms.
fn wall_minutes(text: &str) -> Option<u16> {
    let text = text.trim();
    if let Some((left, right)) = text.split_once(':') {
        let hour: u16 = left.trim().parse().ok()?;
        let minute: u16 = right.trim().parse().ok()?;
        if left.trim().is_empty() || right.trim().is_empty() {
            return None;
        }
        // Colon forms are 0-23 hours with two-digit minutes.
        if hour > 23 || minute > 59 || right.trim().len() != 2 {
            return None;
        }
        return Some(hour * 60 + minute);
    }
    if text.len() != 4 || !text.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let hour: u16 = text[..2].parse().ok()?;
    let minute: u16 = text[2..].parse().ok()?;
    if hour > 23 || minute > 59 {
        return None;
    }
    Some(hour * 60 + minute)
}

/// Parse history rest after `- `: `~~**TIME** <link>~~ · woke HH:MM`.
fn parse_history_rest(
    rest: &str,
    index: usize,
    end: usize,
) -> Option<SnoozeRow> {
    let inner = rest.strip_prefix("~~")?;
    let bold = parse_bold_time(inner)?;
    let after_bold = inner[bold.consumed..].trim_start();
    // The single link must be followed by the closing `~~`.
    let spans = block_link_spans(after_bold);
    if spans.len() != 1 {
        return None;
    }
    let span = &spans[0];
    if !after_bold[..span.token_start].trim().is_empty() {
        return None;
    }
    let after_link = after_bold[span.end..].trim_start();
    let tail = after_link.strip_prefix("~~")?.trim_start();
    let woke = tail.strip_prefix("·")?.trim_start();
    let woke = woke.strip_prefix("woke")?.trim_start();
    let woke_text = woke.trim();
    let woke_minutes = parse_woke_time(woke_text)?;
    let minutes = wall_minutes(&bold.text)?;
    Some(SnoozeRow {
        line_index: index,
        line: index + 1,
        kind: SnoozeRowKind::History,
        minutes: Some(minutes),
        hhmm: Some(format!("{:02}{:02}", minutes / 60, minutes % 60)),
        target: Some(span.target.clone()),
        block_id: Some(span.block_id.clone()),
        embedded: span.embedded,
        aliased: span.aliased,
        children_end: end,
        malformed_reason: None,
        woke_minutes: Some(woke_minutes),
        woke_hhmm: Some(format!(
            "{:02}:{:02}",
            woke_minutes / 60,
            woke_minutes % 60
        )),
    })
}

/// History wake time is always `HH:MM` in the struck tail.
fn parse_woke_time(text: &str) -> Option<u16> {
    let (left, right) = text.split_once(':')?;
    if left.len() != 2 || right.len() != 2 {
        return None;
    }
    let hour: u16 = left.parse().ok()?;
    let minute: u16 = right.parse().ok()?;
    if hour > 23 || minute > 59 {
        return None;
    }
    if text.len() != 5 {
        return None;
    }
    Some(hour * 60 + minute)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(text: &str) -> Vec<&str> {
        // Logical lines without endings; CRLF-safe via lines().
        text.lines().collect()
    }

    #[test]
    fn section_bounds_stop_at_next_h2() {
        let text =
            "## Pomodoros\n\n## Snoozed\n\n- **1530** [[a#^x]]\n\n## Next\n";
        let owned = lines(text);
        let range = snoozed_section_range(&owned).expect("section");
        assert_eq!(range, 3..6);
    }

    #[test]
    fn frontmatter_and_fenced_headings_do_not_count() {
        let text = "---\n## Snoozed\n---\n\n```\n## Snoozed\n```\n\n## Snoozed\n\n- **1530** [[a#^x]]\n";
        let owned = lines(text);
        assert_eq!(first_snoozed_header(&owned), Some(8));
    }

    #[test]
    fn consecutive_active_rows_all_parse() {
        let text = "## Snoozed\n\n- **1530** [[job#^first]]\n- **1530** [[job#^second]]\n";
        let owned = lines(text);
        let section = parse_section(&owned).expect("section");
        assert_eq!(section.rows.len(), 2);
        assert_eq!(section.rows[0].block_id.as_deref(), Some("first"));
        assert_eq!(section.rows[1].block_id.as_deref(), Some("second"));
    }

    #[test]
    fn duplicate_sections_are_linted() {
        let text = "## Snoozed\n\n- **1530** [[a#^x]]\n\n## Snoozed\n";
        let owned = lines(text);
        let section = parse_section(&owned).expect("section");
        assert_eq!(section.rows.len(), 1);
        assert_eq!(section.duplicate_headers, vec![4]);
        assert_eq!(section.duplicate_numbers, vec![5]);
    }

    #[test]
    fn active_row_accepts_colon_and_embedded_aliased_links() {
        let text = "## Snoozed\n\n- **15:30** ![[job#^email-dan|Email Dan]]\n  - note\n";
        let owned = lines(text);
        let section = parse_section(&owned).expect("section");
        assert_eq!(section.rows.len(), 1);
        let row = &section.rows[0];
        assert_eq!(row.kind, SnoozeRowKind::Active);
        assert_eq!(row.minutes, Some(15 * 60 + 30));
        assert_eq!(row.hhmm.as_deref(), Some("1530"));
        assert_eq!(row.target.as_deref(), Some("job"));
        assert_eq!(row.block_id.as_deref(), Some("email-dan"));
        assert!(row.embedded);
        assert!(row.aliased);
        assert_eq!(row.children_end, 4);
    }

    #[test]
    fn history_row_parses_struck_tail() {
        let text = "## Snoozed\n\n- ~~**1530** [[job#^x]]~~ · woke 15:32\n";
        let owned = lines(text);
        let section = parse_section(&owned).expect("section");
        assert_eq!(section.rows.len(), 1);
        let row = &section.rows[0];
        assert_eq!(row.kind, SnoozeRowKind::History);
        assert_eq!(row.minutes, Some(15 * 60 + 30));
        assert_eq!(row.woke_minutes, Some(15 * 60 + 32));
        assert_eq!(row.woke_hhmm.as_deref(), Some("15:32"));
    }

    #[test]
    fn fenced_example_rows_never_execute() {
        let text = "## Snoozed\n\n```\n- **1530** [[a#^x]]\n```\n\n- **1900** [[b#^y]]\n";
        let owned = lines(text);
        let section = parse_section(&owned).expect("section");
        assert_eq!(section.rows.len(), 1);
        assert_eq!(section.rows[0].block_id.as_deref(), Some("y"));
    }

    #[test]
    fn indented_and_extra_link_rows_are_malformed() {
        let text = "## Snoozed\n\n  - **1530** [[a#^x]]\n\n- **1530** [[a#^x]] [[b#^y]]\n";
        let owned = lines(text);
        let section = parse_section(&owned).expect("section");
        assert_eq!(section.rows.len(), 2);
        assert!(section
            .rows
            .iter()
            .all(|row| row.kind == SnoozeRowKind::Malformed));
        assert_eq!(section.rows[0].malformed_reason, Some("indented"));
    }
}
