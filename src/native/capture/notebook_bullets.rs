//! Shared notebook note-bullet scan for completion, sub-note lookup,
//! and block-ID assignment. One Markdown dialect: note bullets at any
//! depth, excluding checkboxes, headings, fenced examples, and prose.

use std::collections::{HashMap, HashSet};

use crate::native::{collect_done, markdown, note_tasks};

/// One eligible note bullet in a notebook note.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NotebookBullet {
    /// Zero-based line index in the note.
    pub(crate) line_index: usize,
    /// One-based line number for display.
    pub(crate) line: usize,
    /// Bullet body text without the list marker, trailing block ID, or
    /// `[created::]` stamp, trimmed.
    pub(crate) text: String,
    /// Trailing block ID when present.
    pub(crate) block_id: Option<String>,
    /// H2 section title containing this bullet, if any.
    pub(crate) section: Option<String>,
    /// Indentation depth (0 for top-level).
    pub(crate) depth: usize,
    /// Nested child bullet count.
    pub(crate) child_count: usize,
    /// `line:digest` reference for stale-safe assignment.
    pub(crate) task_ref: String,
}

fn indent_width(line: &str) -> usize {
    line.chars()
        .take_while(|ch| *ch == ' ' || *ch == '\t')
        .count()
}

fn is_blank(line: &str) -> bool {
    line.trim().is_empty()
}

fn is_markdown_bullet(line: &str) -> bool {
    let trimmed = line.trim_start();
    if trimmed.starts_with("- ")
        || trimmed.starts_with("-\t")
        || trimmed.starts_with("* ")
        || trimmed.starts_with("*\t")
        || trimmed.starts_with("+ ")
        || trimmed.starts_with("+\t")
    {
        return true;
    }
    let digits = trimmed.bytes().take_while(u8::is_ascii_digit).count();
    digits > 0
        && trimmed.as_bytes().get(digits).is_some_and(|byte| {
            matches!(byte, b'.' | b')')
                && trimmed
                    .as_bytes()
                    .get(digits + 1)
                    .is_some_and(u8::is_ascii_whitespace)
        })
}

fn strip_list_marker(line: &str) -> &str {
    let trimmed = line.trim_start();
    for marker in ["- ", "* ", "+ "] {
        if let Some(rest) = trimmed.strip_prefix(marker) {
            return rest;
        }
    }
    for marker in ["-\t", "*\t", "+\t"] {
        if let Some(rest) = trimmed.strip_prefix(marker) {
            return rest;
        }
    }
    // Numbered list: `1. `, `2) `.
    let digits = trimmed.bytes().take_while(u8::is_ascii_digit).count();
    if digits > 0
        && let Some(sep) = trimmed.as_bytes().get(digits)
        && matches!(sep, b'.' | b')')
        && trimmed
            .as_bytes()
            .get(digits + 1)
            .is_some_and(u8::is_ascii_whitespace)
    {
        let mut rest = &trimmed[digits + 1..];
        rest = rest.trim_start_matches([' ', '\t']);
        return rest;
    }
    trimmed
}

fn mask_inline_code_and_wikilinks(line: &str) -> String {
    let mut chars = line.chars().collect::<Vec<_>>();
    let mut index = 0;
    while index < chars.len() {
        if chars[index] == '`' {
            let run_end = (index..chars.len())
                .take_while(|position| chars[*position] == '`')
                .last()
                .map_or(index + 1, |position| position + 1);
            let run = run_end - index;
            let close = (run_end..chars.len()).find(|position| {
                if chars[*position] != '`' {
                    return false;
                }
                let end = (*position..chars.len())
                    .take_while(|candidate| chars[*candidate] == '`')
                    .last()
                    .map_or(*position + 1, |candidate| candidate + 1);
                end - *position == run
            });
            if let Some(close) = close {
                let close_end = (close..chars.len())
                    .take_while(|position| chars[*position] == '`')
                    .last()
                    .map_or(close + 1, |position| position + 1);
                for character in &mut chars[index..close_end] {
                    *character = ' ';
                }
                index = close_end;
                continue;
            }
            index = run_end;
            continue;
        }
        if chars[index] == '['
            && chars.get(index + 1) == Some(&'[')
            && (index == 0 || chars[index - 1] != '\\')
            && let Some(close) = (index + 2..chars.len().saturating_sub(1))
                .find(|position| {
                    chars[*position] == ']' && chars[*position + 1] == ']'
                })
        {
            for character in &mut chars[index..close + 2] {
                *character = ' ';
            }
            index = close + 2;
            continue;
        }
        index += 1;
    }
    chars.into_iter().collect()
}

/// Scan eligible note bullets in document order.
pub(crate) fn scan_notebook_bullets(contents: &str) -> Vec<NotebookBullet> {
    let raw_lines: Vec<&str> = contents.split('\n').collect();
    let logical: Vec<String> = raw_lines
        .iter()
        .map(|line| line.strip_suffix('\r').unwrap_or(line).to_string())
        .collect();
    let borrowed: Vec<&str> = logical.iter().map(String::as_str).collect();
    let fenced = markdown::fenced_lines(&borrowed, 0..borrowed.len());
    let frontmatter_end = markdown::strictly_closed_frontmatter_end(&borrowed);
    // Heading lines (any level) can never parent notes.
    let mut heading_lines = HashSet::new();
    {
        let spans = crate::native::capture::sections::line_spans(contents);
        for heading in
            crate::native::capture::sections::markdown_headings(&spans)
        {
            heading_lines.insert(heading.line_index);
        }
    }
    // H2 sections for the `section` field.
    let sections =
        crate::native::capture_language::notebook::notebook_sections(contents);
    let mut bullets = Vec::new();
    for (index, line) in logical.iter().enumerate() {
        if frontmatter_end.is_some_and(|end| index <= end) {
            continue;
        }
        if fenced.contains(&index) {
            continue;
        }
        if heading_lines.contains(&index) {
            continue;
        }
        if !is_markdown_bullet(line) {
            continue;
        }
        if crate::native::notebooks::rules::is_checkbox_line(line) {
            continue;
        }
        let visible = mask_inline_code_and_wikilinks(line);
        let block_id =
            crate::native::collect_done::trailing_block_id_in_line(&visible);
        let mut body = strip_list_marker(line).trim().to_string();
        // Strip a trailing `^id`.
        if let Some(id) = block_id.as_deref()
            && let Some(stripped) =
                body.strip_suffix(&format!("^{id}")).map(str::trim_end)
        {
            body = stripped.to_string();
        }
        // Strip a trailing `[created::...]` stamp for display.
        if let Some(stamp_start) = body.rfind("[created::")
            && body.ends_with(']')
        {
            body = body[..stamp_start].trim_end().to_string();
        }
        let section = sections
            .iter()
            .rev()
            .find(|sec| sec.line_index < index)
            .map(|sec| sec.title.clone());
        let depth = indent_width(line) / 2;
        let child_count =
            count_child_bullets(&logical, &fenced, &heading_lines, index);
        let digest = note_tasks::line_digest(raw_lines[index]);
        bullets.push(NotebookBullet {
            line_index: index,
            line: index + 1,
            text: body,
            block_id,
            section,
            depth,
            child_count,
            task_ref: format!("{}:{digest}", index + 1),
        });
    }
    bullets
}

fn count_child_bullets(
    lines: &[String],
    fenced: &std::collections::BTreeSet<usize>,
    headings: &HashSet<usize>,
    parent_index: usize,
) -> usize {
    let parent_indent = indent_width(&lines[parent_index]);
    let mut count = 0;
    let mut index = parent_index + 1;
    while index < lines.len() {
        let line = &lines[index];
        if fenced.contains(&index) {
            index += 1;
            continue;
        }
        if is_blank(line) {
            index += 1;
            continue;
        }
        if headings.contains(&index) {
            break;
        }
        let trimmed = line.trim_start();
        if trimmed.starts_with('#') && !line.starts_with([' ', '\t']) {
            break;
        }
        let width = indent_width(line);
        if is_markdown_bullet(line)
            && !crate::native::notebooks::rules::is_checkbox_line(line)
            && width > parent_indent
        {
            count += 1;
            index += 1;
            continue;
        }
        if width <= parent_indent && !line.starts_with([' ', '\t']) {
            break;
        }
        if is_markdown_bullet(line) && width <= parent_indent {
            break;
        }
        index += 1;
    }
    count
}

/// Duplicate IDs among live (outside-fence) anchors.
pub(crate) fn duplicate_ids(contents: &str) -> HashSet<String> {
    let lines: Vec<&str> = contents
        .split('\n')
        .map(|line| line.strip_suffix('\r').unwrap_or(line))
        .collect();
    let frontmatter_end = markdown::strictly_closed_frontmatter_end(&lines);
    let scan_start = frontmatter_end.map_or(0, |end| end + 1);
    let fenced = markdown::fenced_lines(&lines, scan_start..lines.len());
    let mut counts: HashMap<String, usize> = HashMap::new();
    for (index, line) in lines.iter().enumerate() {
        if frontmatter_end.is_some_and(|end| index <= end) {
            continue;
        }
        if fenced.contains(&index) {
            continue;
        }
        let visible = mask_inline_code_and_wikilinks(line);
        if let Some(id) = collect_done::trailing_block_id_in_line(&visible) {
            *counts.entry(id).or_insert(0) += 1;
        }
    }
    counts
        .into_iter()
        .filter(|(_, count)| *count > 1)
        .map(|(id, _)| id)
        .collect()
}

/// Resolve a `line:digest` ref against the eligible bullet snapshot:
/// exact line+digest first, otherwise exactly one matching digest after
/// a line shift; stale or ambiguous matches refuse.
pub(crate) fn resolve_bullet_ref(
    bullets: &[NotebookBullet],
    line: usize,
    digest: &str,
) -> Result<usize, ()> {
    // Exact line+digest first.
    for (pos, bullet) in bullets.iter().enumerate() {
        let (ref_line, ref_digest) = parse_ref(&bullet.task_ref)?;
        if ref_line == line && ref_digest == digest {
            return Ok(pos);
        }
    }
    // Exactly one matching digest after a line shift.
    let mut matches = Vec::new();
    for (pos, bullet) in bullets.iter().enumerate() {
        let (_, ref_digest) = parse_ref(&bullet.task_ref)?;
        if ref_digest == digest {
            matches.push(pos);
        }
    }
    if matches.len() == 1 {
        return Ok(matches[0]);
    }
    Err(())
}

fn parse_ref(task_ref: &str) -> Result<(usize, &str), ()> {
    let (line, digest) = task_ref.split_once(':').ok_or(())?;
    Ok((line.parse().map_err(|_| ())?, digest))
}
