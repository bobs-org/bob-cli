//! Pomodoro-marker rewrite and wikilink parsing for close planning.

use std::{cmp::Reverse, ops::Range};

use super::super::{
    capture::{leading_spaces_or_tabs_len, list_marker_len},
    capture_language,
};

const POMODORO_MARKER: &str = "🍅";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MarkerPolicy {
    Strip,
    Completed,
}

pub(crate) fn strip_pomodoro_markers(line: &str) -> String {
    rewrite_markers(line, MarkerPolicy::Strip)
}

pub(super) fn rewrite_completed_markers(line: &str) -> String {
    rewrite_markers(line, MarkerPolicy::Completed)
}

fn rewrite_markers(line: &str, policy: MarkerPolicy) -> String {
    let tokens = wikilink_tokens(line);
    let strike_spans = strikethrough_inner_spans(line);
    let mut edits = Vec::new();
    for token in &tokens {
        let exact = exact_struck(token, &strike_spans);
        let token_start = if exact {
            token.start.saturating_sub(2)
        } else {
            token.start
        };
        let prefix = pomodoro_marker_prefix(line, token_start);
        let marked = match policy {
            MarkerPolicy::Strip => false,
            MarkerPolicy::Completed => {
                if token.embedded {
                    false
                } else if exact {
                    prefix.count > 0
                } else {
                    true
                }
            }
        };
        let replacement = if marked {
            format!("{POMODORO_MARKER} ")
        } else {
            String::new()
        };
        if (marked && prefix.canonical) || (!marked && prefix.count == 0) {
            continue;
        }
        edits.push((prefix.start, token_start, replacement));
    }
    apply_edits(line, edits)
}

pub(super) struct MarkerPrefix {
    pub(super) start: usize,
    count: usize,
    canonical: bool,
}

pub(super) fn pomodoro_marker_prefix(
    line: &str,
    token_start: usize,
) -> MarkerPrefix {
    let token_start = token_start.min(line.len());
    let mut start = token_start;
    let mut count = 0;
    loop {
        let prefix = &line[..start];
        let without_ws = prefix.trim_end_matches([' ', '\t']);
        if without_ws.len() == prefix.len() {
            break;
        }
        if !without_ws.ends_with(POMODORO_MARKER) {
            break;
        }
        start = without_ws.len() - POMODORO_MARKER.len();
        count += 1;
    }
    MarkerPrefix {
        start,
        count,
        canonical: count == 1 && line.get(start..token_start) == Some("🍅 "),
    }
}

pub(super) fn apply_edits(
    line: &str,
    mut edits: Vec<(usize, usize, String)>,
) -> String {
    edits.sort_by_key(|right| Reverse(right.0));
    let mut rewritten = line.to_string();
    for (start, end, text) in edits {
        if start <= end && end <= rewritten.len() {
            rewritten.replace_range(start..end, &text);
        }
    }
    rewritten
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WikiToken {
    pub(crate) start: usize,
    pub(crate) end: usize,
    pub(crate) embedded: bool,
    pub(crate) path_part: String,
    pub(crate) block_id: String,
    pub(crate) token: String,
}

struct ParsedTarget {
    path_part: String,
    block_id: String,
}

pub(crate) fn wikilink_tokens(line: &str) -> Vec<WikiToken> {
    let mut tokens = Vec::new();
    let mut cursor = 0;
    while let Some(relative) = line[cursor..].find("[[") {
        let open = cursor + relative;
        let inner_start = open + 2;
        let Some(relative_close) = line[inner_start..].find("]]") else {
            break;
        };
        let close = inner_start + relative_close + 2;
        let inner = &line[inner_start..inner_start + relative_close];
        let raw_target = inner.split('|').next().unwrap_or("").trim();
        if let Some(parsed) = parse_block_target(raw_target) {
            let embedded = open > 0 && line.as_bytes()[open - 1] == b'!';
            let start = if embedded { open - 1 } else { open };
            tokens.push(WikiToken {
                start,
                end: close,
                embedded,
                path_part: parsed.path_part,
                block_id: parsed.block_id,
                token: line[open..close].to_string(),
            });
        }
        cursor = close;
    }
    tokens
}

fn parse_block_target(raw_target: &str) -> Option<ParsedTarget> {
    let target = normalize_transcluded_link_target(raw_target);
    let marker = target.find("#^")?;
    let raw_path = target[..marker].trim();
    let block_id = target[marker + 2..].trim();
    if !capture_language::is_block_id(block_id)
        || raw_path.contains('#')
        || raw_path.contains('^')
        || is_uri_scheme(raw_path)
    {
        return None;
    }
    Some(ParsedTarget {
        path_part: strip_markdown_extension(raw_path),
        block_id: block_id.to_string(),
    })
}

fn normalize_transcluded_link_target(value: &str) -> String {
    let mut target = strip_wrapping_quotes(value.trim()).to_string();
    if target.starts_with('<') && target.ends_with('>') && target.len() >= 2 {
        target = target[1..target.len() - 1].trim().to_string();
    }
    safe_decode_uri(&target)
}

fn strip_wrapping_quotes(value: &str) -> &str {
    let bytes = value.as_bytes();
    if bytes.len() >= 2
        && ((bytes[0] == b'"' && bytes[bytes.len() - 1] == b'"')
            || (bytes[0] == b'\'' && bytes[bytes.len() - 1] == b'\''))
    {
        value[1..value.len() - 1].trim()
    } else {
        value
    }
}

fn safe_decode_uri(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%'
            && index + 2 < bytes.len()
            && let Some(high) = from_hex(bytes[index + 1])
            && let Some(low) = from_hex(bytes[index + 2])
        {
            out.push((high << 4) | low);
            index += 3;
            continue;
        }
        out.push(bytes[index]);
        index += 1;
    }
    String::from_utf8(out).unwrap_or_else(|_| value.to_string())
}

fn from_hex(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn is_uri_scheme(path: &str) -> bool {
    let Some(colon) = path.find(':') else {
        return false;
    };
    let scheme = &path[..colon];
    let mut chars = scheme.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    first.is_ascii_alphabetic()
        && chars.all(|character| {
            character.is_ascii_alphanumeric()
                || matches!(character, '+' | '.' | '-')
        })
}

fn strip_markdown_extension(path: &str) -> String {
    let bytes = path.as_bytes();
    if bytes.len() >= 3 && bytes[bytes.len() - 3..].eq_ignore_ascii_case(b".md")
    {
        path[..path.len() - 3].to_string()
    } else {
        path.to_string()
    }
}

pub(crate) fn strikethrough_inner_spans(line: &str) -> Vec<Range<usize>> {
    let mut delimiters = Vec::new();
    let mut cursor = 0;
    while let Some(relative) = line[cursor..].find("~~") {
        let position = cursor + relative;
        delimiters.push(position);
        cursor = position + 2;
    }
    delimiters
        .chunks_exact(2)
        .map(|pair| (pair[0] + 2)..pair[1])
        .collect()
}

pub(crate) fn range_is_struck(
    start: usize,
    end: usize,
    spans: &[Range<usize>],
) -> bool {
    spans
        .iter()
        .any(|span| start >= span.start && end <= span.end)
}

pub(crate) fn exact_struck(token: &WikiToken, spans: &[Range<usize>]) -> bool {
    spans
        .iter()
        .any(|span| span.start == token.start && span.end == token.end)
}

fn trimmed_body_range(line: &str) -> Option<(usize, usize)> {
    let indent = leading_spaces_or_tabs_len(line);
    let after_indent = &line[indent..];
    let marker_len = list_marker_len(after_indent)?;
    let after_marker = &after_indent[marker_len..];
    let whitespace = leading_spaces_or_tabs_len(after_marker);
    if whitespace == 0 {
        return None;
    }
    let body_start = indent + marker_len + whitespace;
    let trimmed = line[body_start..].trim_end_matches([' ', '\t']);
    let body_end = body_start + trimmed.len();
    (body_start < body_end).then_some((body_start, body_end))
}

pub(super) fn move_only_destination(line: &str) -> Option<String> {
    let (body_start, body_end) = trimmed_body_range(line)?;
    if line.as_bytes().get(body_end - 1) != Some(&b'#') {
        return None;
    }
    let directive = body_end - 1;
    if directive <= body_start {
        return None;
    }
    let plains = wikilink_tokens(line)
        .into_iter()
        .filter(|token| !token.embedded)
        .collect::<Vec<_>>();
    if plains.len() != 1 {
        return None;
    }
    let target = &plains[0];
    if target.start != body_start || target.end != directive {
        return None;
    }
    Some(format!("{}{}", &line[..directive], &line[directive + 1..]))
}

pub(crate) fn bare_plain_link(line: &str) -> Option<WikiToken> {
    let (body_start, body_end) = trimmed_body_range(line)?;
    let plains = wikilink_tokens(line)
        .into_iter()
        .filter(|token| !token.embedded)
        .collect::<Vec<_>>();
    if plains.len() != 1 {
        return None;
    }
    let target = plains.into_iter().next()?;
    (target.start == body_start && target.end == body_end).then_some(target)
}

/// The embedded sibling of [`bare_plain_link`]: the trimmed body is exactly
/// one `![[path#^id]]` transclusion. The start planner lists these alongside
/// plain Task Links.
pub(crate) fn bare_embedded_link(line: &str) -> Option<WikiToken> {
    let (body_start, body_end) = trimmed_body_range(line)?;
    let embeds = wikilink_tokens(line)
        .into_iter()
        .filter(|token| token.embedded)
        .collect::<Vec<_>>();
    if embeds.len() != 1 {
        return None;
    }
    let target = embeds.into_iter().next()?;
    (target.start == body_start && target.end == body_end).then_some(target)
}
