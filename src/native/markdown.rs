use std::collections::BTreeSet;
use std::ops::Range;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct MarkdownFence {
    character: u8,
    length: usize,
}

pub(crate) fn fence_marker(line: &str) -> Option<MarkdownFence> {
    let indentation = line.bytes().take_while(|byte| *byte == b' ').count();
    if indentation > 3 {
        return None;
    }
    let line = &line[indentation..];
    let character = *line.as_bytes().first()?;
    if !matches!(character, b'`' | b'~') {
        return None;
    }
    let length = line.bytes().take_while(|byte| *byte == character).count();
    (length >= 3).then_some(MarkdownFence { character, length })
}

pub(crate) fn closes_fence(line: &str, open: MarkdownFence) -> bool {
    let Some(marker) = fence_marker(line) else {
        return false;
    };
    let trimmed = line.trim_start();
    marker.character == open.character
        && marker.length >= open.length
        && trimmed[marker.length..].trim().is_empty()
}

pub(crate) fn fenced_lines(
    lines: &[&str],
    range: Range<usize>,
) -> BTreeSet<usize> {
    let mut fenced = BTreeSet::new();
    let mut open = None;
    for index in range {
        let line = lines[index];
        if let Some(marker) = open {
            fenced.insert(index);
            if closes_fence(line, marker) {
                open = None;
            }
        } else if let Some(marker) = fence_marker(line) {
            fenced.insert(index);
            open = Some(marker);
        }
    }
    fenced
}

pub(crate) fn strictly_closed_frontmatter_end(lines: &[&str]) -> Option<usize> {
    if lines.first().copied().map(str::trim_end) != Some("---") {
        return None;
    }
    lines
        .iter()
        .enumerate()
        .skip(1)
        .find_map(|(index, line)| (line.trim_end() == "---").then_some(index))
}

/// Parse an ATX heading into its `(level, title)`, where `level` is the number
/// of leading `#` characters.
pub(crate) fn atx_heading(line: &str) -> Option<(usize, &str)> {
    let line = markdown_indented_line(line)?;
    let hashes = line
        .as_bytes()
        .iter()
        .take_while(|byte| **byte == b'#')
        .count();
    if !(1..=6).contains(&hashes) {
        return None;
    }

    if line
        .as_bytes()
        .get(hashes)
        .is_some_and(|byte| !byte.is_ascii_whitespace())
    {
        return None;
    }

    Some((hashes, strip_closing_atx_hashes(line[hashes..].trim())))
}

fn strip_closing_atx_hashes(title: &str) -> &str {
    let trimmed = title.trim_end();
    let without_hashes = trimmed.trim_end_matches('#');
    if without_hashes.len() == trimmed.len() {
        return trimmed;
    }

    if without_hashes
        .chars()
        .next_back()
        .is_none_or(char::is_whitespace)
    {
        without_hashes.trim_end()
    } else {
        trimmed
    }
}

fn markdown_indented_line(line: &str) -> Option<&str> {
    let spaces = line
        .as_bytes()
        .iter()
        .take_while(|byte| **byte == b' ')
        .count();
    if spaces > 3 {
        return None;
    }
    Some(&line[spaces..])
}

/// Split a physical line into content and its terminator (`\n`, `\r\n`, or empty).
#[allow(dead_code)]
pub(crate) fn split_line_ending(line: &str) -> (&str, &str) {
    if let Some(content) = line.strip_suffix("\r\n") {
        (content, "\r\n")
    } else if let Some(content) = line.strip_suffix('\n') {
        (content, "\n")
    } else {
        (line, "")
    }
}

/// Parse a Setext underline into heading level 1 (`=`) or 2 (`-`).
///
/// Accepts 0–3 leading spaces and trailing whitespace. The remaining
/// characters must all be the same marker.
#[allow(dead_code)]
pub(crate) fn setext_underline(line: &str) -> Option<usize> {
    let line = markdown_indented_line(line)?;
    let trimmed = line.trim_end();
    let mut characters = trimmed.chars();
    let marker = characters.next()?;
    if !matches!(marker, '=' | '-') {
        return None;
    }
    characters
        .all(|character| character == marker)
        .then_some(if marker == '=' { 1 } else { 2 })
}

/// Inner text of a standalone HTML comment line, trimmed.
#[allow(dead_code)]
pub(crate) fn standalone_html_comment(line: &str) -> Option<&str> {
    let trimmed = line.trim();
    let inner = trimmed.strip_prefix("<!--")?.strip_suffix("-->")?;
    Some(inner.trim())
}

#[allow(dead_code)]
pub(crate) fn html_comment_opens(line: &str) -> bool {
    line.trim_start().starts_with("<!--")
}

#[allow(dead_code)]
pub(crate) fn html_comment_closes(line: &str) -> bool {
    line.contains("-->")
}

/// A blockquote line: 0–3 leading spaces followed by `>`.
#[allow(dead_code)]
pub(crate) fn is_blockquote_line(line: &str) -> bool {
    markdown_indented_line(line).is_some_and(|rest| rest.starts_with('>'))
}

/// Document-level indented code: 4+ leading spaces on a non-blank line.
#[allow(dead_code)]
pub(crate) fn is_indented_code_line(line: &str) -> bool {
    !line.trim().is_empty()
        && line.bytes().take_while(|byte| *byte == b' ').count() >= 4
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn setext_underline_accepts_levels_and_indent() {
        assert_eq!(setext_underline("==="), Some(1));
        assert_eq!(setext_underline("="), Some(1));
        assert_eq!(setext_underline("----"), Some(2));
        assert_eq!(setext_underline("  ---"), Some(2));
        assert_eq!(setext_underline("---   "), Some(2));
        assert_eq!(setext_underline("    ---"), None);
        assert_eq!(setext_underline("- -"), None);
        assert_eq!(setext_underline("***"), None);
    }

    #[test]
    fn standalone_html_comment_reads_inner_text() {
        assert_eq!(
            standalone_html_comment("<!-- bob:task-status-group:v1:active -->"),
            Some("bob:task-status-group:v1:active")
        );
        assert_eq!(
            standalone_html_comment("  <!--  spaced  -->  "),
            Some("spaced")
        );
        assert_eq!(standalone_html_comment("<!-- unterminated"), None);
        assert_eq!(standalone_html_comment("not a comment"), None);
    }

    #[test]
    fn blockquote_and_indented_code_detection() {
        assert!(is_blockquote_line("> quoted"));
        assert!(is_blockquote_line("  > quoted"));
        assert!(!is_blockquote_line("    > four spaces"));
        assert!(is_indented_code_line("    code"));
        assert!(!is_indented_code_line("   almost"));
        assert!(!is_indented_code_line(""));
    }

    #[test]
    fn split_line_ending_preserves_crlf_lf_and_none() {
        assert_eq!(split_line_ending("ab\r\n"), ("ab", "\r\n"));
        assert_eq!(split_line_ending("ab\n"), ("ab", "\n"));
        assert_eq!(split_line_ending("ab"), ("ab", ""));
    }
}
