//! Detects Templater tags a daily note still carries.
//!
//! Core Daily Notes copies the raw template into a new note and Templater
//! renders it moments later, replacing the whole file. A writer that lands
//! in between loses its work, so writers refuse a note while any `<%`
//! opening delimiter (which covers `<%*`, `<%+`, `<%-`, and `<%_`) is left.
//! YAML frontmatter is scanned; fenced code blocks and inline code spans
//! are not, because a rendered note may legitimately show a literal tag.

use super::markdown::{closes_fence, fence_marker};

/// Longest excerpt kept, in characters.
const EXCERPT_CHARS: usize = 60;

/// The first Templater tag still pending in a note.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PendingTag {
    /// 1-based line of the tag.
    pub line: usize,
    /// The tag and the text after it, trimmed to about 60 characters.
    pub excerpt: String,
}

/// First pending Templater tag in `contents`, or `None` for a rendered note.
pub(crate) fn first_pending_tag(contents: &str) -> Option<PendingTag> {
    let mut fence = None;
    for (index, line) in contents.lines().enumerate() {
        if let Some(open) = fence {
            if closes_fence(line, open) {
                fence = None;
            }
            continue;
        }
        if let Some(open) = fence_marker(line) {
            fence = Some(open);
            continue;
        }
        if let Some(start) = tag_outside_code_spans(line) {
            return Some(PendingTag {
                line: index + 1,
                excerpt: line[start..]
                    .chars()
                    .take(EXCERPT_CHARS)
                    .collect::<String>()
                    .trim_end()
                    .to_string(),
            });
        }
    }
    None
}

/// Byte offset of the first `<%` outside inline code spans on one line.
///
/// A span opens at a backtick run and closes at the next run of the same
/// length; a run with no match is literal text.
fn tag_outside_code_spans(line: &str) -> Option<usize> {
    let bytes = line.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'`' => {
                let length = run_length(bytes, index, b'`');
                match matching_run_end(bytes, index + length, length) {
                    Some(end) => index = end,
                    None => index += length,
                }
            }
            b'<' if bytes.get(index + 1) == Some(&b'%') => return Some(index),
            _ => index += 1,
        }
    }
    None
}

fn run_length(bytes: &[u8], start: usize, byte: u8) -> usize {
    bytes[start..].iter().take_while(|b| **b == byte).count()
}

/// End (exclusive) of the next backtick run of exactly `length`.
fn matching_run_end(bytes: &[u8], from: usize, length: usize) -> Option<usize> {
    let mut index = from;
    while index < bytes.len() {
        if bytes[index] == b'`' {
            let run = run_length(bytes, index, b'`');
            if run == length {
                return Some(index + run);
            }
            index += run;
        } else {
            index += 1;
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frontmatter_tag_is_pending() {
        let note = "---\ndate: <% tp.date.now() %>\n---\n# Day\n";
        let tag = first_pending_tag(note).expect("pending");
        assert_eq!(tag.line, 2);
        assert_eq!(tag.excerpt, "<% tp.date.now() %>");
    }

    #[test]
    fn body_tag_is_pending() {
        let note = "# Day\n\n## Pomodoros\n- <% tp.file.title %>\n";
        assert_eq!(first_pending_tag(note).map(|tag| tag.line), Some(4));
    }

    #[test]
    fn every_opening_variant_is_pending() {
        for open in ["<%*", "<%+", "<%-", "<%_", "<%"] {
            let note = format!("line\n{open} tR += 1 %>\n");
            assert_eq!(
                first_pending_tag(&note).map(|tag| tag.line),
                Some(2),
                "{open}"
            );
        }
    }

    #[test]
    fn fenced_blocks_are_ignored() {
        let note = "```\n<% tp.x %>\n```\n~~~js\n<%* y %>\n~~~\ntext\n";
        assert_eq!(first_pending_tag(note), None);
        let after = "```\n<% x %>\n```\n<% real %>\n";
        assert_eq!(first_pending_tag(after).map(|tag| tag.line), Some(4));
    }

    #[test]
    fn unclosed_fence_hides_the_rest() {
        assert_eq!(first_pending_tag("```\n<% x %>\n"), None);
    }

    #[test]
    fn inline_code_is_ignored() {
        assert_eq!(first_pending_tag("use `<% x %>` here\n"), None);
        assert_eq!(first_pending_tag("use ``<% a ` b %>`` here\n"), None);
        let tag = first_pending_tag("`code` then <% x %>\n").expect("pending");
        assert_eq!(tag.excerpt, "<% x %>");
    }

    #[test]
    fn unmatched_backtick_is_literal() {
        assert_eq!(
            first_pending_tag("a ` b <% x %>\n").map(|tag| tag.line),
            Some(1)
        );
    }

    #[test]
    fn rendered_note_has_no_pending_tag() {
        let note = "---\ndate: 2026-11-09\n---\n## Pomodoros\n- 100% done <\n";
        assert_eq!(first_pending_tag(note), None);
    }

    #[test]
    fn excerpt_is_trimmed() {
        let long = format!("<% {} %>", "x".repeat(200));
        let tag = first_pending_tag(&long).expect("pending");
        assert_eq!(tag.excerpt.chars().count(), 60);
    }
}
