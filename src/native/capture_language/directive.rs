//! Offset-preserving `%` directive lexer.
//!
//! Directives sit at either edge of any physical line of an item, with only
//! other markers between the token and that edge. This phase recognizes
//! `%c` / `%c<N>`; every other `%…` token at an edge is unknown, and a
//! bare `%` is incomplete. Later phases extend [`Directives`] with note,
//! template, and listen.

use super::dependencies::{
    is_trailing_transparent, protected_ranges, scan_line_blocked_tasks,
    scan_line_dependencies, ScannedDependency,
};
use super::draft::split_physical_lines;
use super::model::{CaptureItem, ClipRequest, RawLine, Token};
use super::relations::ScannedRelation;
use super::{
    editor_parse::tokenize_line_with_spans, relations::scan_line_relations,
};
use std::num::NonZeroUsize;
use std::ops::Range;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Directives {
    pub(crate) clip: Option<ClipRequest>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DirectiveOccurrence {
    Complete {
        kind: DirectiveKind,
        span: Range<usize>,
    },
    Incomplete {
        span: Range<usize>,
    },
    Unknown {
        span: Range<usize>,
        raw: String,
    },
    Invalid {
        span: Range<usize>,
        raw: String,
        message: String,
    },
    Escaped {
        span: Range<usize>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DirectiveKind {
    Clip { count: NonZeroUsize },
}

impl DirectiveOccurrence {
    pub(crate) fn span(&self) -> &Range<usize> {
        match self {
            Self::Complete { span, .. }
            | Self::Incomplete { span }
            | Self::Unknown { span, .. }
            | Self::Invalid { span, .. }
            | Self::Escaped { span } => span,
        }
    }

    pub(crate) fn is_marker(&self) -> bool {
        !matches!(self, Self::Escaped { .. })
    }

    pub(crate) fn clip_request(&self) -> Option<ClipRequest> {
        match self {
            Self::Complete {
                kind: DirectiveKind::Clip { count },
                ..
            } => Some(ClipRequest::from_count(*count)),
            _ => None,
        }
    }
}

/// True when a whitespace-delimited token is claimed as a `%` directive
/// (complete, incomplete, unknown, invalid, or escaped) at an item edge.
pub(crate) fn is_directive_token(text: &str) -> bool {
    text.starts_with('%')
        || text
            .strip_prefix('\\')
            .is_some_and(|rest| rest.starts_with('%'))
}

pub(crate) fn incomplete_directive_error() -> String {
    "`%` is incomplete: add a directive (`%c`), or write `\\%` to keep a literal `%`"
        .to_string()
}

pub(crate) fn unknown_directive_error(token: &str) -> String {
    format!(
        "`{token}` is not a capture directive; write `%c` or `%c3`, or `\\{token}` to keep the text"
    )
}

pub(crate) fn invalid_clip_count_error(token: &str) -> String {
    if token
        .strip_prefix("%c")
        .is_some_and(|digits| digits.starts_with('0') && digits.len() > 1)
    {
        let rest = token.trim_start_matches("%c0");
        let example = if rest.is_empty() { "3" } else { rest };
        return format!(
            "`{token}` is invalid: don't pad the count with zeros (`%c{example}`)"
        );
    }
    format!(
        "`{token}` is invalid: `%c` reads the clipboard once; `%c3` reads three values"
    )
}

pub(crate) fn clip_not_read_error() -> String {
    "`%c` reads the clipboard, but --no-clip forbids reading it: drop --no-clip, or write `\\%c` to keep the text"
        .to_string()
}

/// Scan one physical line for edge `%` directives. Child bullet prefixes
/// are excluded from edge checks; returned ranges stay absolute.
#[allow(dead_code)]
pub(crate) fn scan_line(line: &str, base: usize) -> Vec<DirectiveOccurrence> {
    scan_line_inner(line, base, false)
}

fn scan_line_inner(
    line: &str,
    base: usize,
    child_line: bool,
) -> Vec<DirectiveOccurrence> {
    let (body_start, body) = if child_line {
        authored_body(line)
    } else {
        (0, line)
    };
    let protected = protected_ranges(body);
    let dependency_scans = scan_line_dependencies(body, 0)
        .into_iter()
        .chain(scan_line_blocked_tasks(body, 0))
        .collect::<Vec<_>>();
    let relation_scans = scan_line_relations(body, 0);
    let modifier_protected = dependency_scans
        .iter()
        .map(|scan| scan.start()..scan.end())
        .chain(relation_scans.iter().map(|scan| scan.start()..scan.end()))
        .collect::<Vec<_>>();

    let mut candidates = Vec::new();
    for (start, end) in whitespace_tokens(body) {
        let token = &body[start..end];
        if !is_directive_token(token)
            || protected.iter().any(|(protected_start, protected_end)| {
                *protected_start < end && start < *protected_end
            })
            || modifier_protected
                .iter()
                .any(|span| span.start < end && start < span.end)
        {
            continue;
        }
        candidates.push(Candidate {
            occurrence: classify_token(token, start..end),
            edge_start: start,
            edge_end: end,
        });
    }

    let mut allowed = Vec::<Range<usize>>::new();
    for candidate in &candidates {
        allowed.push(candidate.edge_start..candidate.edge_end);
    }
    for (start, end) in whitespace_tokens(body) {
        if is_trailing_transparent(&body[start..end], true) {
            allowed.push(start..end);
        }
    }
    for found in &dependency_scans {
        match found {
            ScannedDependency::Complete(entry) => {
                allowed.push(entry.start..entry.end)
            }
            ScannedDependency::Partial { start, end, .. } => {
                allowed.push(*start..*end)
            }
            _ => {}
        }
    }
    for found in &relation_scans {
        match found {
            ScannedRelation::Complete(entry) => {
                allowed.push(entry.start..entry.end)
            }
            ScannedRelation::PartialTarget { start, end, .. }
            | ScannedRelation::PartialLabel { start, end, .. } => {
                allowed.push(*start..*end)
            }
            ScannedRelation::Invalid(_) | ScannedRelation::Escaped { .. } => {}
        }
    }

    candidates
        .into_iter()
        .filter_map(|mut candidate| {
            let leading = edge_run_is_transparent(
                body,
                0,
                candidate.edge_start,
                &allowed,
            );
            let trailing = edge_run_is_transparent(
                body,
                candidate.edge_end,
                body.len(),
                &allowed,
            );
            if !leading && !trailing {
                return None;
            }
            shift_occurrence(&mut candidate.occurrence, base + body_start);
            Some(candidate.occurrence)
        })
        .collect()
}

struct Candidate {
    occurrence: DirectiveOccurrence,
    edge_start: usize,
    edge_end: usize,
}

fn classify_token(token: &str, span: Range<usize>) -> DirectiveOccurrence {
    if let Some(rest) = token.strip_prefix('\\')
        && rest.starts_with('%')
    {
        return DirectiveOccurrence::Escaped { span };
    }
    if token == "%" {
        return DirectiveOccurrence::Incomplete { span };
    }
    if let Some(rest) = token.strip_prefix("%c") {
        return classify_clip(token, rest, span);
    }
    DirectiveOccurrence::Unknown {
        span,
        raw: token.to_string(),
    }
}

fn classify_clip(
    token: &str,
    rest: &str,
    span: Range<usize>,
) -> DirectiveOccurrence {
    if rest.is_empty() {
        return DirectiveOccurrence::Complete {
            kind: DirectiveKind::Clip {
                count: NonZeroUsize::MIN,
            },
            span,
        };
    }
    if !rest.bytes().all(|byte| byte.is_ascii_digit()) {
        return DirectiveOccurrence::Unknown {
            span,
            raw: token.to_string(),
        };
    }
    if rest.starts_with('0') {
        return DirectiveOccurrence::Invalid {
            span,
            raw: token.to_string(),
            message: invalid_clip_count_error(token),
        };
    }
    match rest.parse::<usize>().ok().and_then(NonZeroUsize::new) {
        Some(count) => DirectiveOccurrence::Complete {
            kind: DirectiveKind::Clip { count },
            span,
        },
        None => DirectiveOccurrence::Invalid {
            span,
            raw: token.to_string(),
            message: invalid_clip_count_error(token),
        },
    }
}

pub(crate) fn collect_item(item: &CaptureItem<'_>) -> Vec<DirectiveOccurrence> {
    let lines = split_physical_lines(item.source);
    let mut found = Vec::new();
    let first_line_start = item
        .lines
        .first()
        .map(|line| line.raw.start)
        .unwrap_or(item.start);
    for item_line in &item.lines {
        let Some(line) = lines.iter().find(|line| {
            line.start <= item_line.raw.start && item_line.raw.start <= line.end
        }) else {
            continue;
        };
        let child = line.start > first_line_start;
        found.extend(
            scan_line_inner(line.text, line.start, child)
                .into_iter()
                .filter(|occurrence| {
                    occurrence.span().start >= item.start
                        && occurrence.span().end <= item.end
                }),
        );
    }
    found.sort_by_key(|occurrence| occurrence.span().start);
    found.dedup_by(|left, right| left.span() == right.span());
    // `URL %` / `URL % @route` / `@route URL %` still request companion
    // audio until the `%l` phase deletes the standalone listen marker.
    found.retain(|occurrence| !occurrence_is_listen_percent(item, occurrence));
    found
}

fn occurrence_is_listen_percent(
    item: &CaptureItem<'_>,
    occurrence: &DirectiveOccurrence,
) -> bool {
    if !matches!(occurrence, DirectiveOccurrence::Incomplete { .. }) {
        return false;
    }
    if item.lines.len() != 1 {
        return false;
    }
    let line = &item.lines[0].raw;
    let tokens = tokenize_line_with_spans(line);
    percent_token_is_listen(&tokens, occurrence.span())
}

fn percent_token_is_listen(tokens: &[Token<'_>], span: &Range<usize>) -> bool {
    let Some(index) = tokens
        .iter()
        .position(|token| token.start == span.start && token.end == span.end)
    else {
        return false;
    };
    if tokens[index].text != "%" {
        return false;
    }
    match tokens {
        [url, _] if index == 1 => {
            crate::native::url_routing::classify_token(url.text).is_some()
        }
        [url, _, route] if index == 1 => {
            crate::native::url_routing::classify_token(url.text).is_some()
                && is_plain_at_route(route.text)
        }
        [route, url, _] if index == 2 => {
            crate::native::url_routing::classify_token(url.text).is_some()
                && is_plain_at_route(route.text)
        }
        _ => false,
    }
}

fn is_plain_at_route(text: &str) -> bool {
    let Some(name) = text.strip_prefix('@') else {
        return false;
    };
    !name.is_empty()
        && !name.starts_with('@')
        && !name.contains('#')
        && !name.contains(':')
        && !name.contains('+')
}

/// Same-length line copy with directive tokens blanked. Escapes lose only
/// their backslash, leaving the visible `%…` text in the body.
pub(crate) fn masked_source(
    item: &CaptureItem<'_>,
    occurrences: &[DirectiveOccurrence],
) -> String {
    let mut bytes = item.source.as_bytes().to_vec();
    mask_bytes(&mut bytes, occurrences);
    String::from_utf8(bytes).expect("directive masking preserves UTF-8")
}

pub(crate) fn mask_bytes(
    bytes: &mut [u8],
    occurrences: &[DirectiveOccurrence],
) {
    for occurrence in occurrences {
        match occurrence {
            DirectiveOccurrence::Complete { span, .. }
            | DirectiveOccurrence::Incomplete { span }
            | DirectiveOccurrence::Unknown { span, .. }
            | DirectiveOccurrence::Invalid { span, .. } => {
                bytes[span.clone()].fill(b' ');
            }
            DirectiveOccurrence::Escaped { span } => bytes[span.start] = b' ',
        }
    }
}

pub(crate) fn masked_item<'a>(
    item: &CaptureItem<'a>,
    source: &'a str,
) -> CaptureItem<'a> {
    super::type_marker::masked_item(item, source)
}

/// Tokenize the masked line but retain slices into the original line.
#[allow(dead_code)]
pub(crate) fn masked_tokens<'a>(line: &RawLine<'a>) -> Vec<Token<'a>> {
    let occurrences = scan_line(line.text, line.start);
    let mut bytes = line.text.as_bytes().to_vec();
    mask_bytes(&mut bytes, &occurrences);
    let masked =
        String::from_utf8(bytes).expect("directive masking preserves UTF-8");
    let view = RawLine {
        text: &masked,
        start: line.start,
        end: line.end,
    };
    tokenize_line_with_spans(&view)
        .into_iter()
        .map(|token| Token {
            text: &line.text[token.start - line.start..token.end - line.start],
            start: token.start,
            end: token.end,
        })
        .collect()
}

pub(crate) fn resolve(
    occurrences: &[DirectiveOccurrence],
) -> Result<Directives, (String, &'static str, Range<usize>)> {
    let markers = occurrences
        .iter()
        .filter(|occurrence| occurrence.is_marker())
        .collect::<Vec<_>>();
    if markers.len() > 1 {
        let span = markers[1].span().clone();
        return Err((
            super::line::duplicate_marker_error("directive (%c)"),
            "duplicate_capture_marker",
            span,
        ));
    }
    let Some(occurrence) = markers.first() else {
        return Ok(Directives { clip: None });
    };
    match occurrence {
        DirectiveOccurrence::Complete { .. } => Ok(Directives {
            clip: occurrence.clip_request(),
        }),
        DirectiveOccurrence::Incomplete { span } => Err((
            incomplete_directive_error(),
            "incomplete_directive",
            span.clone(),
        )),
        DirectiveOccurrence::Unknown { span, raw } => Err((
            unknown_directive_error(raw),
            "unknown_directive",
            span.clone(),
        )),
        DirectiveOccurrence::Invalid { span, message, .. } => {
            Err((message.clone(), "unknown_directive", span.clone()))
        }
        DirectiveOccurrence::Escaped { .. } => Ok(Directives { clip: None }),
    }
}

fn authored_body(line: &str) -> (usize, &str) {
    if let Some((body_start, body)) =
        super::draft::strip_bullet_marker_at(line, 0)
    {
        return (body_start, body);
    }
    if let Some((body_start, body)) =
        super::draft::strip_bullet_marker_at(line, 2)
    {
        return (body_start, body);
    }
    (0, line)
}

fn whitespace_tokens(line: &str) -> Vec<(usize, usize)> {
    let mut tokens = Vec::new();
    let mut start = None;
    for (index, character) in line.char_indices() {
        if character.is_whitespace() {
            if let Some(token_start) = start.take() {
                tokens.push((token_start, index));
            }
        } else if start.is_none() {
            start = Some(index);
        }
    }
    if let Some(token_start) = start {
        tokens.push((token_start, line.len()));
    }
    tokens
}

fn edge_run_is_transparent(
    line: &str,
    mut cursor: usize,
    limit: usize,
    allowed: &[Range<usize>],
) -> bool {
    while cursor < limit {
        while cursor < limit
            && line[cursor..]
                .chars()
                .next()
                .is_some_and(char::is_whitespace)
        {
            cursor += line[cursor..]
                .chars()
                .next()
                .map(char::len_utf8)
                .unwrap_or(1);
        }
        if cursor == limit {
            return true;
        }
        let Some(span) = allowed
            .iter()
            .filter(|span| span.start == cursor && span.end <= limit)
            .max_by_key(|span| span.end)
        else {
            return false;
        };
        cursor = span.end;
    }
    true
}

fn shift_occurrence(occurrence: &mut DirectiveOccurrence, base: usize) {
    let shift = |span: &mut Range<usize>| {
        span.start += base;
        span.end += base;
    };
    match occurrence {
        DirectiveOccurrence::Complete { span, .. }
        | DirectiveOccurrence::Incomplete { span }
        | DirectiveOccurrence::Unknown { span, .. }
        | DirectiveOccurrence::Invalid { span, .. }
        | DirectiveOccurrence::Escaped { span } => shift(span),
    }
}
