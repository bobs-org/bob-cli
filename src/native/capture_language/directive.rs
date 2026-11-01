//! Offset-preserving `%` directive lexer.
//!
//! Directives sit at either edge of any physical line of an item, with only
//! other markers between the token and that edge. The lexer recognizes
//! `%c` / `%c<N>`, `%n`, `%t`, and `%l`; every other `%…` token at an edge
//! is unknown, and a bare `%` is incomplete.

use super::dependencies::{
    is_trailing_transparent, protected_ranges, scan_line_blocked_tasks,
    scan_line_dependencies, ScannedDependency,
};
use super::draft::split_physical_lines;
use super::editor_parse::tokenize_line_with_spans;
use super::model::{CaptureItem, ClipRequest, RawLine, Token};
use super::relations::scan_line_relations;
use super::relations::ScannedRelation;
use std::num::NonZeroUsize;
use std::ops::Range;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Directives {
    pub(crate) clip: Option<ClipRequest>,
    pub(crate) note: bool,
    pub(crate) template: bool,
    pub(crate) listen: bool,
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
    Note,
    Template,
    Listen,
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
    "`%` is incomplete: add a directive (`%c`, `%n`, `%t`, `%l`), or write `\\%` to keep a literal `%`"
        .to_string()
}

pub(crate) fn unknown_directive_error(token: &str) -> String {
    format!(
        "`{token}` is not a capture directive; write `%c`, `%n`, `%t`, or `%l`, or `\\{token}` to keep the text"
    )
}

pub(crate) fn directive_not_applicable_error() -> String {
    "`%l` queues companion audio for a bare URL".to_string()
}

pub(crate) fn template_directive_misplaced_error() -> String {
    "`%t` marks the first item as the template".to_string()
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
    if token == "%n" {
        return DirectiveOccurrence::Complete {
            kind: DirectiveKind::Note,
            span,
        };
    }
    if token == "%t" {
        return DirectiveOccurrence::Complete {
            kind: DirectiveKind::Template,
            span,
        };
    }
    if token == "%l" {
        return DirectiveOccurrence::Complete {
            kind: DirectiveKind::Listen,
            span,
        };
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

/// Byte ranges of every complete `%t` directive at an edge of `lines`, read
/// as one item: the first line is the parent line and the rest are
/// authored child lines.
pub(crate) fn template_tokens(lines: &[RawLine<'_>]) -> Vec<Range<usize>> {
    lines
        .iter()
        .enumerate()
        .flat_map(|(index, line)| template_tokens_in_line(line, index > 0))
        .collect()
}

pub(crate) fn template_tokens_in_line(
    line: &RawLine<'_>,
    child_line: bool,
) -> Vec<Range<usize>> {
    scan_line_inner(line.text, line.start, child_line)
        .into_iter()
        .filter_map(|occurrence| match occurrence {
            DirectiveOccurrence::Complete {
                kind: DirectiveKind::Template,
                span,
            } => Some(span),
            _ => None,
        })
        .collect()
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
    found
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

fn directive_discriminant(occurrence: &DirectiveOccurrence) -> Option<u8> {
    match occurrence {
        DirectiveOccurrence::Complete { kind, .. } => Some(match kind {
            DirectiveKind::Clip { .. } => 0,
            DirectiveKind::Note => 1,
            DirectiveKind::Listen => 2,
            DirectiveKind::Template => 3,
        }),
        _ => None,
    }
}

fn duplicate_label(discriminant: u8) -> &'static str {
    match discriminant {
        0 => "directive (%c)",
        1 => "directive (%n)",
        3 => "directive (%t)",
        _ => "directive (%l)",
    }
}

pub(crate) fn resolve(
    occurrences: &[DirectiveOccurrence],
) -> Result<Directives, (String, &'static str, Range<usize>)> {
    let markers = occurrences
        .iter()
        .filter(|occurrence| occurrence.is_marker())
        .collect::<Vec<_>>();
    // Each directive appears at most once per item; different directives
    // compose (`%n %c`). A repeated letter refuses as a duplicate.
    let mut seen: Vec<u8> = Vec::new();
    for marker in &markers {
        if let Some(discriminant) = directive_discriminant(marker) {
            if seen.contains(&discriminant) {
                let span = marker.span().clone();
                return Err((
                    super::line::duplicate_marker_error(duplicate_label(
                        discriminant,
                    )),
                    "duplicate_capture_marker",
                    span,
                ));
            }
            seen.push(discriminant);
        }
    }
    // Two markers where at least one is incomplete/unknown/invalid is a
    // duplicate; report the second marker.
    let non_complete = markers
        .iter()
        .filter(|marker| directive_discriminant(marker).is_none())
        .count();
    if markers.len() > 1 && non_complete > 0 {
        let span = markers[1].span().clone();
        // When the duplicate is a repeated complete kind it was already
        // reported above; here the label is generic.
        return Err((
            super::line::duplicate_marker_error("directive (%c)"),
            "duplicate_capture_marker",
            span,
        ));
    }
    let mut directives = Directives {
        clip: None,
        note: false,
        template: false,
        listen: false,
    };
    for occurrence in markers {
        match occurrence {
            DirectiveOccurrence::Complete { kind, .. } => match kind {
                DirectiveKind::Clip { .. } => {
                    directives.clip = occurrence.clip_request();
                }
                DirectiveKind::Note => directives.note = true,
                DirectiveKind::Template => directives.template = true,
                DirectiveKind::Listen => directives.listen = true,
            },
            DirectiveOccurrence::Incomplete { span } => {
                return Err((
                    incomplete_directive_error(),
                    "incomplete_directive",
                    span.clone(),
                ));
            }
            DirectiveOccurrence::Unknown { span, raw } => {
                return Err((
                    unknown_directive_error(raw),
                    "unknown_directive",
                    span.clone(),
                ));
            }
            DirectiveOccurrence::Invalid { span, message, .. } => {
                return Err((
                    message.clone(),
                    "unknown_directive",
                    span.clone(),
                ));
            }
            DirectiveOccurrence::Escaped { .. } => {}
        }
    }
    Ok(directives)
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
