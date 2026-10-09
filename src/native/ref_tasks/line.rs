//! Candidate and follow-up line parsing.

/// One `#ref` task line that resolves to no ref note.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OrphanRefTask {
    pub path: String,
    pub line_index: usize,
    pub reason: String,
    pub target: String,
}

/// One `🔖` follow-up link attributed to a ref note.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RefFollowUp {
    pub path: String,
    pub line_index: usize,
    pub mark: char,
    pub text: String,
    pub block_id: String,
}

/// Drop leading `>` quote markers: up to three spaces, `>`, one optional
/// space, repeated. Mirrors the private `freshness/placement.rs`
/// `strip_blockquote_prefix`.
pub(crate) fn strip_blockquote_prefix(mut line: &str) -> &str {
    loop {
        let spaces = line.bytes().take_while(|byte| *byte == b' ').count();
        if spaces > 3 || line.as_bytes().get(spaces) != Some(&b'>') {
            return line;
        }
        line = &line[spaces + 1..];
        if let Some(rest) = line.strip_prefix(' ') {
            line = rest;
        }
    }
}

/// The checkbox mark on a task line, if `line` (already blockquote-stripped)
/// is a list task line: indentation, then `-`/`*`/`+`, a space, `[m]`, then
/// end-of-line or whitespace.
pub(crate) fn task_mark(line: &str) -> Option<char> {
    let trimmed_start = line.trim_start_matches(|c| c == ' ' || c == '\t');
    // Indentation then marker.
    let after_marker = trimmed_start
        .strip_prefix("- ")
        .or_else(|| trimmed_start.strip_prefix("* "))
        .or_else(|| trimmed_start.strip_prefix("+ "))?;
    // task_mark callers pass the blockquote-stripped line, but the `- `
    // above already consumed the marker+space; now expect `[m]`.
    let bracket = after_marker.strip_prefix('[')?;
    let mark = bracket.chars().next()?;
    let rest = bracket[mark.len_utf8()..].strip_prefix(']')?;
    if !rest.is_empty() && !rest.starts_with(char::is_whitespace) {
        return None;
    }
    Some(mark)
}

/// True when the line is a task line per the locator rules.
pub(crate) fn is_task_line(line: &str) -> bool {
    task_mark(line).is_some()
}

/// True when whitespace tokens include `#ref`, ASCII case-insensitive.
/// `#references` and `#ref/x` do not count.
pub(crate) fn has_ref_token(line: &str) -> bool {
    line.split_whitespace()
        .any(|token| token.eq_ignore_ascii_case("#ref"))
}

/// True when whitespace tokens include the exact token `^ref`.
pub(crate) fn has_caret_ref(line: &str) -> bool {
    line.split_whitespace().any(|token| token == "^ref")
}

/// True when the line is an embed line (`- ![[...]]` or managed embed):
/// after the checkbox, the body starts with `!`. Such lines are ignored.
pub(crate) fn is_embed_task_line(stripped: &str) -> bool {
    let trimmed_start = stripped.trim_start_matches(|c| c == ' ' || c == '\t');
    let after = trimmed_start
        .strip_prefix("- ")
        .or_else(|| trimmed_start.strip_prefix("* "))
        .or_else(|| trimmed_start.strip_prefix("+ "));
    let Some(after) = after else {
        return false;
    };
    // Skip `[m] ` prefix.
    let Some(bracket_end) = after.find(']') else {
        return false;
    };
    let rest = after[bracket_end + 1..].trim_start();
    rest.starts_with('!')
}

/// Parse a follow-up `## Tasks`-style line via the shared
/// `highlights_ref` parser so both paths strip the `🔖` link and inline
/// fields identically.
pub(crate) fn parse_follow_up_task(
    line: &str,
) -> Option<(char, String, String)> {
    let task = crate::native::highlights_ref::parse_follow_up_task(line)?;
    Some((task.mark, task.text, task.block_id))
}
