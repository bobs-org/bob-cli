//! Notebook body rules: checkboxes, block IDs, shape, summaries.

use std::collections::{BTreeMap, HashSet};

use super::model::{NotebookIssue, Severity};

/// True when `line` opens or closes a fenced code block. Updates `fence`
/// exactly like the project scanner so line numbers stay physical.
pub(crate) fn track_fence(
    line: &str,
    fence: &mut Option<(char, usize)>,
) -> bool {
    let trimmed = line.trim_start_matches(' ');
    let indentation = line.len() - trimmed.len();
    if indentation > 3 {
        return fence.is_some();
    }
    if let Some((marker, opening)) = *fence {
        let count = trimmed.chars().take_while(|ch| *ch == marker).count();
        if count >= opening && trimmed[count..].trim().is_empty() {
            *fence = None;
        }
        return true;
    }
    let Some(marker) = trimmed.chars().next() else {
        return false;
    };
    if !matches!(marker, '`' | '~') {
        return false;
    }
    let count = trimmed.chars().take_while(|ch| *ch == marker).count();
    if count < 3 {
        return false;
    }
    *fence = Some((marker, count));
    true
}

/// Split contents into physical lines without the trailing `\r`.
pub(crate) fn physical_lines(contents: &str) -> Vec<&str> {
    contents
        .split('\n')
        .map(|line| line.strip_suffix('\r').unwrap_or(line))
        .collect()
}

/// Body start line (1-based) for a frontmatter block, or 1 when absent.
pub(crate) fn body_start_line(contents: &str) -> usize {
    let mut lines = contents.lines();
    let Some(first) = lines.next() else {
        return 1;
    };
    if first.strip_suffix('\r').unwrap_or(first) != "---" {
        return 1;
    }
    for (index, line) in (2..).zip(lines) {
        let line = line.strip_suffix('\r').unwrap_or(line);
        if line == "---" {
            return index + 1;
        }
    }
    1
}

/// Any checkbox list row outside fenced code, at any depth.
/// Covers unordered `-`, `*`, `+` and ordered `1.` / `1)` rows with an
/// arbitrary status character. Does not require `#task`.
pub(crate) fn is_checkbox_line(line: &str) -> bool {
    let rest = line.trim_start();
    // Ordered marker first.
    if let Some(dot) = ordered_marker_len(rest) {
        let after = rest[dot..].trim_start();
        return bracket_status(after);
    }
    let Some(first) = rest.chars().next() else {
        return false;
    };
    if !matches!(first, '-' | '*' | '+') {
        return false;
    }
    let after = rest[first.len_utf8()..].trim_start();
    bracket_status(after)
}

fn ordered_marker_len(line: &str) -> Option<usize> {
    let digits = line
        .char_indices()
        .take_while(|(_, ch)| ch.is_ascii_digit())
        .map(|(index, ch)| index + ch.len_utf8())
        .last()?;
    let after_digits = line[digits..].chars().next()?;
    if !matches!(after_digits, '.' | ')') {
        return None;
    }
    let marker = digits + after_digits.len_utf8();
    // Require whitespace after the marker.
    if line[marker..]
        .chars()
        .next()
        .is_some_and(|ch| ch.is_whitespace())
    {
        Some(marker)
    } else {
        None
    }
}

fn bracket_status(after_marker: &str) -> bool {
    let Some(after_open) = after_marker.strip_prefix('[') else {
        return false;
    };
    let mut chars = after_open.chars();
    let Some(_status) = chars.next() else {
        return false;
    };
    let Some(after_status) = chars.as_str().strip_prefix(']') else {
        return false;
    };
    after_status.is_empty() || after_status.starts_with(char::is_whitespace)
}

/// Collect checkbox findings for every applicable line.
pub(crate) fn checkbox_findings(
    body_lines: &[(usize, &str)],
) -> Vec<NotebookIssue> {
    let mut issues = Vec::new();
    let mut fence = None;
    for (line_number, line) in body_lines {
        if track_fence(line, &mut fence) {
            continue;
        }
        if is_checkbox_line(line) {
            issues.push(NotebookIssue::error(
                "notebook_checkbox",
                Some(*line_number),
                "notebooks hold no checkbox lines; move tasks to the owner area or project",
            ));
        }
    }
    issues
}

/// All `^block-id` occurrences outside fenced code, with their lines.
pub(crate) fn block_ids_outside_fences(
    body_lines: &[(usize, &str)],
) -> Vec<(usize, String)> {
    let mut found = Vec::new();
    let mut fence = None;
    for (line_number, line) in body_lines {
        if track_fence(line, &mut fence) {
            continue;
        }
        for id in extract_block_ids(line) {
            found.push((*line_number, id));
        }
    }
    found
}

fn extract_block_ids(line: &str) -> Vec<String> {
    let mut ids = Vec::new();
    let bytes = line.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'^' {
            let start = index + 1;
            let mut end = start;
            while end < bytes.len()
                && (bytes[end].is_ascii_alphanumeric()
                    || matches!(bytes[end], b'-' | b'_'))
            {
                end += 1;
            }
            if end > start {
                ids.push(line[start..end].to_string());
            }
            index = end.max(index + 1);
        } else {
            index += 1;
        }
    }
    ids
}

pub(crate) fn duplicate_block_id_findings(
    body_lines: &[(usize, &str)],
) -> Vec<NotebookIssue> {
    let ids = block_ids_outside_fences(body_lines);
    let mut counts: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for (line, id) in ids {
        counts.entry(id).or_default().push(line);
    }
    let mut issues = Vec::new();
    for (id, lines) in counts {
        if lines.len() > 1 {
            for line in lines.iter().skip(1) {
                issues.push(NotebookIssue::error(
                    "notebook_duplicate_block_id",
                    Some(*line),
                    format!("duplicate block ID `^{id}` in this notebook"),
                ));
            }
        }
    }
    issues.sort_by_key(|issue| issue.line.unwrap_or(0));
    issues
}

/// Warn for numbered top-level lists, `====` dividers, H3+, extra or
/// misplaced H1s, and stray top-level paragraphs. Nested detail under a
/// note never counts: once a column-zero note bullet opens a region,
/// indented lines belong to that note until the next column-zero item
/// or heading.
pub(crate) fn shape_findings(
    body_lines: &[(usize, &str)],
    first_bullet_line: Option<usize>,
    first_h2_line: Option<usize>,
    h1_lines: &[usize],
) -> Vec<NotebookIssue> {
    let mut issues = Vec::new();
    // Extra or misplaced H1s.
    if h1_lines.len() > 1 {
        for line in h1_lines.iter().skip(1) {
            issues.push(NotebookIssue::warning(
                "notebook_shape",
                Some(*line),
                "notebooks hold at most one early H1 before the first bullet or H2",
            ));
        }
    }
    if let Some(first_h1) = h1_lines.first()
        && let (Some(bullet), Some(h2)) = (first_bullet_line, first_h2_line)
    {
        let limit = bullet.min(h2);
        if *first_h1 > limit {
            issues.push(NotebookIssue::warning(
                "notebook_shape",
                Some(*first_h1),
                "H1 belongs before the first bullet or H2 section",
            ));
        }
    } else if let Some(first_h1) = h1_lines.first() {
        let limit = first_bullet_line.or(first_h2_line);
        if let Some(limit) = limit
            && *first_h1 > limit
        {
            issues.push(NotebookIssue::warning(
                "notebook_shape",
                Some(*first_h1),
                "H1 belongs before the first bullet or H2 section",
            ));
        }
    }

    let mut fence = None;
    let mut seen_bullet_region = false;
    for (line_number, line) in body_lines {
        if track_fence(line, &mut fence) {
            continue;
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        // Headings.
        if line.trim_start().starts_with('#') {
            let hashes = line
                .trim_start()
                .chars()
                .take_while(|ch| *ch == '#')
                .count();
            if hashes >= 3 {
                issues.push(NotebookIssue::warning(
                    "notebook_shape",
                    Some(*line_number),
                    "notebooks use only H1 and H2 headings",
                ));
            }
            continue;
        }
        // Column-zero items.
        if !line.starts_with([' ', '\t']) {
            if is_note_bullet(line) {
                seen_bullet_region = true;
                continue;
            }
            if is_numbered_list_item(line) {
                issues.push(NotebookIssue::warning(
                    "notebook_shape",
                    Some(*line_number),
                    "numbered lists are not notebook notes; use `-` bullets",
                ));
                continue;
            }
            if is_setext_divider(trimmed) {
                issues.push(NotebookIssue::warning(
                    "notebook_shape",
                    Some(*line_number),
                    "notebooks use `##` sections, not `====` dividers",
                ));
                continue;
            }
            // Stray top-level paragraph: text at column zero outside a
            // bullet region's nested detail. The optional preamble (before
            // the first bullet/H2) is allowed once.
            let limit = first_bullet_line.or(first_h2_line);
            let is_preamble = limit.is_some_and(|first| *line_number < first)
                && !seen_bullet_region;
            if !is_preamble && looks_like_paragraph(line) {
                // Only flag the first stray paragraph per region start to
                // avoid cascading on wrapped prose.
                issues.push(NotebookIssue::warning(
                    "notebook_shape",
                    Some(*line_number),
                    "stray top-level paragraph; keep prose in the preamble or nested under a note",
                ));
                // Prevent a wrapped paragraph from warning on every line:
                // mark the region so only the first line warns.
                seen_bullet_region = true;
            }
        }
    }
    issues.sort_by_key(|issue| issue.line.unwrap_or(0));
    issues
}

fn is_note_bullet(line: &str) -> bool {
    if line.starts_with([' ', '\t']) {
        return false;
    }
    let Some(first) = line.chars().next() else {
        return false;
    };
    if !matches!(first, '-' | '*' | '+') {
        return false;
    }
    line[first.len_utf8()..]
        .chars()
        .next()
        .is_some_and(|ch| ch.is_whitespace())
}

fn is_numbered_list_item(line: &str) -> bool {
    if line.starts_with([' ', '\t']) {
        return false;
    }
    ordered_marker_len(line).is_some()
}

fn is_setext_divider(trimmed: &str) -> bool {
    !trimmed.is_empty()
        && trimmed.chars().all(|ch| ch == '=')
        && trimmed.len() >= 3
}

fn looks_like_paragraph(line: &str) -> bool {
    // A column-zero line that is not a bullet, heading, divider, fence,
    // blockquote, table rule, or HTML comment.
    let trimmed = line.trim();
    if trimmed.is_empty()
        || trimmed.starts_with(['-', '*', '+', '#', '>', '|'])
        || trimmed.starts_with("<!--")
        || trimmed.starts_with("```")
        || trimmed.starts_with("~~~")
    {
        return false;
    }
    // Numbered items handled above; anything else with text counts.
    true
}

/// Validate the `summary:` frontmatter value.
pub(crate) fn summary_findings(summary: Option<&str>) -> Vec<NotebookIssue> {
    let Some(raw) = summary else {
        return vec![NotebookIssue::warning(
            "notebook_summary_missing",
            None,
            "add a one-line `summary:` describing what belongs in this notebook",
        )];
    };
    if raw.trim().is_empty() {
        return vec![NotebookIssue::warning(
            "notebook_summary_missing",
            None,
            "add a one-line `summary:` describing what belongs in this notebook",
        )];
    }
    let mut issues = Vec::new();
    if raw.contains('\n') {
        issues.push(NotebookIssue::warning(
            "notebook_summary_multiline",
            None,
            "keep `summary:` to one line",
        ));
    }
    // Measured in characters, not bytes.
    let first_line = raw.lines().next().unwrap_or(raw);
    if first_line.chars().count() > 140 {
        issues.push(NotebookIssue::warning(
            "notebook_summary_long",
            None,
            "keep `summary:` to at most 140 characters",
        ));
    }
    issues
}

/// Deduplicate findings by (code, line, message) keeping first order.
pub(crate) fn dedupe(mut issues: Vec<NotebookIssue>) -> Vec<NotebookIssue> {
    let mut seen = HashSet::new();
    issues.retain(|issue| {
        seen.insert((issue.code.to_string(), issue.line, issue.message.clone()))
    });
    issues.sort_by(|left, right| {
        left.line
            .cmp(&right.line)
            .then_with(|| left.code.cmp(right.code))
            .then_with(|| left.message.cmp(&right.message))
    });
    // Errors sort before warnings on the same line for stable reports.
    issues.sort_by(|left, right| {
        left.line.cmp(&right.line).then_with(|| {
            severity_rank(left.severity).cmp(&severity_rank(right.severity))
        })
    });
    issues
}

fn severity_rank(severity: Severity) -> u8 {
    match severity {
        Severity::Error => 0,
        Severity::Warning => 1,
    }
}
