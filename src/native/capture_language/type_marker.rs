//! Shared lexer and byte-preserving view for capture `t:<type>` markers.
//!
//! This phase feeds the execution parser. The editor adopts the same scanner
//! in its own phase, so marker positions and escaping stay identical.

use super::dependencies::{
    is_trailing_transparent, protected_ranges, scan_line_blocked_tasks,
    scan_line_dependencies, ScannedDependency,
};
use super::model::{CaptureItem, ParsedCaptureText, RawLine, Token};
use super::pomodoro_literal::{
    lex_exact_name_after_hash, merge_leading_literal_token, opens_exact_name,
};
use super::relations::ScannedRelation;
use super::{
    draft::split_physical_lines, editor_parse::tokenize_line_with_spans,
    relations::scan_line_relations,
};
use crate::native::{
    config::themes::ConfiguredTheme, plan_budget::normalize_component,
    task_fields::canonical_theme_name,
};
use std::ops::Range;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TypeMarker {
    pub(crate) name: String,
    pub(crate) literal: bool,
    pub(crate) span: Range<usize>,
    pub(crate) name_span: Range<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TypeMarkerCompletion {
    pub(crate) span: Range<usize>,
    pub(crate) query: String,
    pub(crate) leading: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TypeMarkerOccurrence {
    Complete(TypeMarker),
    Incomplete { span: Range<usize> },
    Invalid { span: Range<usize>, raw: String },
    Escaped { span: Range<usize> },
}

impl TypeMarkerOccurrence {
    pub(crate) fn span(&self) -> &Range<usize> {
        match self {
            Self::Complete(marker) => &marker.span,
            Self::Incomplete { span }
            | Self::Invalid { span, .. }
            | Self::Escaped { span } => span,
        }
    }

    pub(crate) fn is_marker(&self) -> bool {
        !matches!(self, Self::Escaped { .. })
    }
}

#[derive(Debug, Clone)]
struct Candidate {
    occurrence: TypeMarkerOccurrence,
    edge_start: usize,
    edge_end: usize,
}

/// Scan an authored physical line for type markers. Child bullet prefixes
/// are excluded from edge checks, while all returned ranges remain absolute.
pub(crate) fn scan_line(line: &str, base: usize) -> Vec<TypeMarkerOccurrence> {
    scan_line_with_clip_control(line, base, true)
}

pub(crate) fn scan_line_with_clip_control(
    line: &str,
    base: usize,
    parse_clip_markers: bool,
) -> Vec<TypeMarkerOccurrence> {
    scan_line_inner(line, base, false, parse_clip_markers)
}

fn scan_child_line(
    line: &str,
    base: usize,
    parse_clip_markers: bool,
) -> Vec<TypeMarkerOccurrence> {
    scan_line_inner(line, base, true, parse_clip_markers)
}

fn scan_line_inner(
    line: &str,
    base: usize,
    child_line: bool,
    parse_clip_markers: bool,
) -> Vec<TypeMarkerOccurrence> {
    scan_line_with_positions(line, base, child_line, parse_clip_markers)
        .into_iter()
        .map(|(occurrence, _)| occurrence)
        .collect()
}

fn scan_line_with_positions(
    line: &str,
    base: usize,
    child_line: bool,
    parse_clip_markers: bool,
) -> Vec<(TypeMarkerOccurrence, bool)> {
    let (body_start, body) = if child_line {
        authored_body(line)
    } else {
        (0, line)
    };
    let protected = protected_ranges(body);
    let quoted_route_tokens = quoted_route_tokens(body);
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
    let mut index = 0usize;
    while index < body.len() {
        if !token_start(body, index) {
            index += char_len(body, index);
            continue;
        }
        let escaped = body[index..].starts_with("\\t:");
        let sigil = if escaped { index + 1 } else { index };
        if !body[sigil..].starts_with("t:")
            || protected
                .iter()
                .any(|(start, end)| *start <= sigil && sigil < *end)
            || quoted_route_tokens
                .iter()
                .any(|span| span.start <= sigil && sigil < span.end)
            || modifier_protected
                .iter()
                .any(|span| span.start <= sigil && sigil < span.end)
        {
            index += char_len(body, index);
            continue;
        }
        let parsed = parse_candidate(body, index, sigil, escaped);
        index = parsed.edge_end.max(index + 1);
        candidates.push(parsed);
    }

    let mut allowed = Vec::<Range<usize>>::new();
    for candidate in &candidates {
        allowed.push(candidate.edge_start..candidate.edge_end);
    }
    for (start, end) in whitespace_tokens(body) {
        if is_trailing_transparent(&body[start..end], parse_clip_markers) {
            allowed.push(start..end);
        }
    }
    allowed.extend(quoted_route_tokens.iter().cloned());
    for found in dependency_scans {
        if let ScannedDependency::Complete(entry) = found {
            allowed.push(entry.start..entry.end);
        } else if let ScannedDependency::Partial { start, end, .. } = found {
            allowed.push(start..end);
        }
    }
    for found in relation_scans {
        match found {
            ScannedRelation::Complete(entry) => {
                allowed.push(entry.start..entry.end)
            }
            ScannedRelation::PartialTarget { start, end, .. }
            | ScannedRelation::PartialLabel { start, end, .. } => {
                allowed.push(start..end)
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
            Some((candidate.occurrence, leading))
        })
        .collect()
}

/// Return the shared lexer's completable type marker under the cursor,
/// including its decoded query and whether it occupies the leading edge.
/// Invalid and escaped marker-shaped text never opens the type picker.
pub(crate) fn completion_at(
    item: &CaptureItem<'_>,
    cursor: usize,
) -> Option<TypeMarkerCompletion> {
    let lines = split_physical_lines(item.source);
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
        for (occurrence, leading) in scan_line_with_positions(
            line.text,
            line.start,
            line.start > first_line_start,
            true,
        ) {
            let span = occurrence.span().clone();
            if span.start < item.start
                || span.end > item.end
                || cursor < span.start
                || cursor > span.end
            {
                continue;
            }
            let query = match &occurrence {
                TypeMarkerOccurrence::Complete(_)
                | TypeMarkerOccurrence::Incomplete { .. } => {
                    decoded_completion_query(
                        line.text, line.start, span.start, cursor,
                    )?
                }
                TypeMarkerOccurrence::Invalid { .. }
                | TypeMarkerOccurrence::Escaped { .. } => continue,
            };
            return Some(TypeMarkerCompletion {
                span,
                query,
                leading,
            });
        }
    }
    None
}

pub(crate) fn completion_at_raw(
    raw_text: &str,
    cursor: usize,
) -> Option<TypeMarkerCompletion> {
    let draft = super::draft::split_capture_draft(raw_text);
    let item = draft.items.iter().find(|item| {
        item.lines
            .iter()
            .any(|line| cursor >= line.raw.start && cursor <= line.raw.end)
    })?;
    completion_at(item, cursor)
}

fn decoded_completion_query(
    line: &str,
    base: usize,
    marker_start: usize,
    cursor: usize,
) -> Option<String> {
    let prefix = line.get(marker_start - base..cursor - base)?;
    let rest = prefix.strip_prefix("t:")?;
    if rest.starts_with('"') || rest.starts_with('“') {
        Some(
            lex_exact_name_after_hash(rest, rest.len())
                .map(|lex| lex.decoded)
                .unwrap_or_default(),
        )
    } else {
        Some(rest.to_string())
    }
}

fn quoted_route_tokens(line: &str) -> Vec<Range<usize>> {
    let raw = RawLine {
        text: line,
        start: 0,
        end: line.len(),
    };
    let tokens = tokenize_line_with_spans(&raw);
    merge_leading_literal_token(&raw, tokens)
        .into_iter()
        .filter(|token| {
            (token.text.starts_with('@') || token.text.starts_with('^'))
                && token.text.find('#').is_some_and(|hash| {
                    opens_exact_name(&token.text[hash + 1..])
                })
        })
        .map(|token| token.start..token.end)
        .collect()
}

fn parse_candidate(
    line: &str,
    start: usize,
    sigil: usize,
    escaped: bool,
) -> Candidate {
    let content_start = sigil + 2;
    let rest = &line[content_start..];
    let candidate_end = token_end(line, start);
    let (occurrence, edge_end) = if rest.starts_with('"')
        || rest.starts_with('“')
    {
        let lex = lex_exact_name_after_hash(rest, rest.len())
            .expect("quote opener yields a literal");
        let marker_end = content_start + lex.literal_len;
        let literal_remainder = if lex.complete
            && marker_end < line.len()
            && !line[marker_end..]
                .chars()
                .next()
                .is_some_and(char::is_whitespace)
        {
            token_end(line, marker_end)
        } else {
            marker_end
        };
        let raw_end = if lex.complete {
            literal_remainder
        } else {
            line.len()
        };
        if escaped {
            (
                TypeMarkerOccurrence::Escaped {
                    span: start..raw_end,
                },
                raw_end,
            )
        } else if !lex.complete || lex.decoded.trim().is_empty() {
            (
                TypeMarkerOccurrence::Incomplete {
                    span: start..raw_end,
                },
                raw_end,
            )
        } else if lex.decoded.contains('+') {
            (
                TypeMarkerOccurrence::Invalid {
                    span: start..raw_end,
                    raw: line[start..raw_end].to_string(),
                },
                raw_end,
            )
        } else if marker_end != raw_end {
            (
                TypeMarkerOccurrence::Invalid {
                    span: start..raw_end,
                    raw: line[start..raw_end].to_string(),
                },
                raw_end,
            )
        } else {
            // Name content excludes the quotes with their real byte
            // lengths, so curly quotes keep char-boundary ranges.
            let (open_len, close_len) = if rest.starts_with('"') {
                (1, 1)
            } else {
                ("“".len(), "”".len())
            };
            (
                TypeMarkerOccurrence::Complete(TypeMarker {
                    name: lex.decoded,
                    literal: true,
                    span: start..marker_end,
                    name_span: content_start + open_len..marker_end - close_len,
                }),
                marker_end,
            )
        }
    } else {
        let suffix = &line[content_start..candidate_end];
        let slug_len = suffix
            .bytes()
            .take_while(|byte| byte.is_ascii_alphanumeric() || *byte == b'-')
            .count();
        let slug = &suffix[..slug_len];
        let valid_start = slug
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_alphanumeric);
        if escaped {
            (
                TypeMarkerOccurrence::Escaped {
                    span: start..candidate_end,
                },
                candidate_end,
            )
        } else if suffix.is_empty() {
            (
                TypeMarkerOccurrence::Incomplete {
                    span: start..candidate_end,
                },
                candidate_end,
            )
        } else if !valid_start || slug_len != suffix.len() {
            (
                TypeMarkerOccurrence::Invalid {
                    span: start..candidate_end,
                    raw: line[start..candidate_end].to_string(),
                },
                candidate_end,
            )
        } else {
            (
                TypeMarkerOccurrence::Complete(TypeMarker {
                    name: slug.to_string(),
                    literal: false,
                    span: start..candidate_end,
                    name_span: content_start..candidate_end,
                }),
                candidate_end,
            )
        }
    };
    Candidate {
        occurrence,
        edge_start: start,
        edge_end,
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
            cursor += char_len(line, cursor);
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

fn shift_occurrence(occurrence: &mut TypeMarkerOccurrence, base: usize) {
    let shift = |span: &mut Range<usize>| {
        span.start += base;
        span.end += base;
    };
    match occurrence {
        TypeMarkerOccurrence::Complete(marker) => {
            shift(&mut marker.span);
            shift(&mut marker.name_span);
        }
        TypeMarkerOccurrence::Incomplete { span }
        | TypeMarkerOccurrence::Invalid { span, .. }
        | TypeMarkerOccurrence::Escaped { span } => shift(span),
    }
}

fn token_start(line: &str, index: usize) -> bool {
    index == 0
        || line[..index]
            .chars()
            .next_back()
            .is_some_and(char::is_whitespace)
}

fn token_end(line: &str, start: usize) -> usize {
    let mut end = start;
    while end < line.len()
        && !line[end..].chars().next().unwrap().is_whitespace()
    {
        end += char_len(line, end);
    }
    end
}

fn char_len(line: &str, index: usize) -> usize {
    line[index..]
        .chars()
        .next()
        .map(char::len_utf8)
        .unwrap_or(1)
}

/// Collect this item's markers from its original physical lines. Item ranges
/// filter synthetic chain items so a marker belongs to exactly one split
/// operator.
pub(crate) fn collect_item_with_clip_control(
    item: &CaptureItem<'_>,
    parse_clip_markers: bool,
) -> Vec<TypeMarkerOccurrence> {
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
        let occurrences = if line.start > first_line_start {
            scan_child_line(line.text, line.start, parse_clip_markers)
        } else {
            scan_line_with_clip_control(
                line.text,
                line.start,
                parse_clip_markers,
            )
        };
        found.extend(occurrences.into_iter().filter(|occurrence| {
            occurrence.span().start >= item.start
                && occurrence.span().end <= item.end
        }));
    }
    found.sort_by_key(|occurrence| occurrence.span().start);
    found.dedup_by(|left, right| left.span() == right.span());
    found
}

/// Return a same-length line copy with type markers blanked. Escaped markers
/// lose only their backslash, leaving the visible `t:…` text in the body.
pub(crate) fn masked_line(line: &RawLine<'_>) -> String {
    masked_line_with_clip_control(line, true)
}

pub(crate) fn masked_line_with_clip_control(
    line: &RawLine<'_>,
    parse_clip_markers: bool,
) -> String {
    let mut bytes = line.text.as_bytes().to_vec();
    for occurrence in
        scan_line_with_clip_control(line.text, line.start, parse_clip_markers)
    {
        match occurrence {
            TypeMarkerOccurrence::Complete(_)
            | TypeMarkerOccurrence::Incomplete { .. }
            | TypeMarkerOccurrence::Invalid { .. } => {
                let range = occurrence.span();
                let start = range.start - line.start;
                let end = range.end - line.start;
                bytes[start..end].fill(b' ');
            }
            TypeMarkerOccurrence::Escaped { span } => {
                let start = span.start - line.start;
                bytes[start] = b' ';
            }
        }
    }
    String::from_utf8(bytes).expect("type-marker masking preserves UTF-8")
}

/// Tokenize the masked line but retain slices into the original line. Only
/// token classification changes; every returned span still indexes the
/// authored text.
pub(crate) fn masked_tokens_with_clip_control<'a>(
    line: &RawLine<'a>,
    parse_clip_markers: bool,
) -> Vec<Token<'a>> {
    let masked = masked_line_with_clip_control(line, parse_clip_markers);
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

/// Build an owned same-length source copy and an item view borrowing from it.
/// The execution wrapper remaps declaration slices back to the original
/// source before this storage is dropped.
pub(crate) fn masked_source(
    item: &CaptureItem<'_>,
    occurrences: &[TypeMarkerOccurrence],
) -> String {
    let mut bytes = item.source.as_bytes().to_vec();
    for occurrence in occurrences {
        match occurrence {
            TypeMarkerOccurrence::Complete(_)
            | TypeMarkerOccurrence::Incomplete { .. }
            | TypeMarkerOccurrence::Invalid { .. } => {
                let span = occurrence.span();
                bytes[span.clone()].fill(b' ');
            }
            TypeMarkerOccurrence::Escaped { span } => bytes[span.start] = b' ',
        }
    }
    String::from_utf8(bytes).expect("type-marker masking preserves UTF-8")
}

pub(crate) fn masked_item<'a>(
    item: &CaptureItem<'a>,
    source: &'a str,
) -> CaptureItem<'a> {
    CaptureItem {
        source,
        index: item.index,
        start: item.start,
        end: item.end,
        line_start: item.line_start,
        line_end: item.line_end,
        lines: item
            .lines
            .iter()
            .map(|line| ItemLineView::masked(line, source))
            .collect(),
    }
}

/// Find continuation rows whose only authored content is one type marker.
/// They remain in the original item for ranges and ownership, but parsers
/// omit them from semantic child lists and whole-item claim counts.
pub(crate) fn marker_only_child_lines(
    item: &CaptureItem<'_>,
    occurrences: &[TypeMarkerOccurrence],
) -> Vec<(usize, usize)> {
    let mut skips = Vec::new();
    for line in item.lines.iter().skip(1) {
        let raw = line.raw;
        let (body_start, body) =
            super::draft::strip_bullet_marker_at(raw.text, 0)
                .or_else(|| super::draft::strip_bullet_marker_at(raw.text, 2))
                .map(|(start, body)| (raw.start + start, body))
                .unwrap_or((raw.start, raw.text));
        let leading = body.len() - body.trim_start().len();
        let trailing = body.len() - body.trim_end().len();
        if leading + trailing >= body.len() {
            continue;
        }
        let span = (body_start + leading, body_start + body.len() - trailing);
        let mut markers = occurrences.iter().filter(|occurrence| {
            occurrence.is_marker()
                && occurrence.span().start >= raw.start
                && occurrence.span().end <= raw.end
        });
        let single = markers.next();
        if markers.next().is_none()
            && single.is_some_and(|occurrence| {
                occurrence.span().start == span.0
                    && occurrence.span().end == span.1
            })
        {
            skips.push((raw.start, raw.end));
        }
    }
    skips
}

struct ItemLineView;

impl ItemLineView {
    fn masked<'a>(
        line: &super::model::ItemLine<'_>,
        source: &'a str,
    ) -> super::model::ItemLine<'a> {
        super::model::ItemLine {
            raw: RawLine {
                text: &source[line.raw.start..line.raw.end],
                start: line.raw.start,
                end: line.raw.end,
            },
            line_number: line.line_number,
        }
    }
}

/// Stable capture-language copy shared by execution and the later editor
/// adapter.
pub(crate) fn invalid_marker_error(raw: &str) -> String {
    if raw.contains('+') {
        let name = raw
            .strip_prefix("t:")
            .unwrap_or(raw)
            .trim_matches(['"', '“', '”']);
        if let Some((left, right)) = name.split_once('+') {
            return format!(
                "`t:{name}` names two types; a task has one type: write `t:{left}` or `t:{right}`"
            );
        }
        return format!(
            "`t:{name}` names two types; a task has one type: write `t:<type>`"
        );
    }
    format!(
        "`{raw}` is not a task type: names use letters, digits, and `-`; quote other names (`t:\"Q&A\"`) or write `\\t:fix.` to keep the text"
    )
}

pub(crate) fn needs_task_error(name: Option<&str>) -> String {
    match name {
        Some(name) => format!(
            "`t:{name}` needs a task: add the task text (`Fix the parser t:{name}`) or target one (`^sase:fix-x t:{name}`)"
        ),
        None => "`t:` is incomplete: type a task type after `t:` (for example `t:fix`)".to_string(),
    }
}

/// An exact task-type name match, shared by editor lookup and the
/// capture writer. The configured vocabulary wins ties; a literal matches
/// by normalized-key equality, while a slug also tries the key with spaces
/// in place of `-` (`deep-work` finds `DEEP WORK`). Prefixes never match.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ResolvedTypeName {
    pub(crate) key: String,
    pub(crate) name: String,
}

pub(crate) fn resolve_exact_type_name(
    configured: Option<&[ConfiguredTheme]>,
    catalog_name: &dyn Fn(&str) -> Option<String>,
    name: &str,
    literal: bool,
) -> Option<ResolvedTypeName> {
    let key = normalize_component(name);
    let hyphen_key = key.replace('-', " ");
    let mut candidates = vec![key];
    if !literal && hyphen_key != candidates[0] {
        candidates.push(hyphen_key);
    }
    for candidate in candidates {
        if let Some(entry) = configured
            .unwrap_or_default()
            .iter()
            .find(|entry| entry.key == candidate)
        {
            return Some(ResolvedTypeName {
                key: entry.key.clone(),
                name: entry.name.clone(),
            });
        }
        if let Some(spelling) = catalog_name(&candidate) {
            return Some(ResolvedTypeName {
                key: candidate,
                name: spelling,
            });
        }
    }
    None
}

/// Display spelling for a decoded marker name with no exact match: the
/// canonicalized name when valid, else the decoded text itself.
pub(crate) fn unmatched_type_display(name: &str) -> String {
    canonical_theme_name(name).unwrap_or_else(|| name.to_string())
}

/// Which subject a type marker found, shared by execution and the editor so
/// both refuse non-task forms with the same teaching copy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TypeMarkerSubject {
    NewTask,
    Bullet,
    SubBullet,
    ProjectNote,
    Reference,
    PomodoroNote,
    TaskComplete,
    ExistingLink,
    TaskToggle,
    Close,
    Start,
    Adjust,
    Shift,
}

impl TypeMarkerSubject {
    pub(crate) fn for_kind(kind: &super::model::CaptureKind) -> Self {
        use super::model::CaptureKind;
        match kind {
            CaptureKind::Task | CaptureKind::TaskWithBlockId { .. } => {
                Self::NewTask
            }
            CaptureKind::Pomodoro { close: Some(_), .. } => Self::Close,
            CaptureKind::Pomodoro { close: None, .. } => Self::NewTask,
            CaptureKind::Bullet { .. } => Self::Bullet,
            CaptureKind::SubBullet { .. } => Self::SubBullet,
            CaptureKind::ProjectNote { .. } => Self::ProjectNote,
            CaptureKind::Ref(_) => Self::Reference,
            CaptureKind::PomodoroNote => Self::PomodoroNote,
            CaptureKind::TaskComplete { .. } => Self::TaskComplete,
            CaptureKind::PomodoroLink { close: Some(_), .. } => Self::Close,
            CaptureKind::PomodoroLink { close: None, .. } => Self::ExistingLink,
            CaptureKind::TaskToggle { .. } => Self::TaskToggle,
            CaptureKind::PomodoroClose { .. } => Self::Close,
            CaptureKind::PomodoroStart { .. } => Self::Start,
            CaptureKind::PomodoroAdjust { .. } => Self::Adjust,
            CaptureKind::PomodoroShift { .. } => Self::Shift,
        }
    }
}

pub(crate) fn type_marker_not_applicable_error(
    name: &str,
    subject: TypeMarkerSubject,
) -> String {
    let marker = format!("t:{name}");
    let (action, remedy) = match subject {
        TypeMarkerSubject::NewTask => return needs_task_error(Some(name)),
        TypeMarkerSubject::Bullet => (
            "writes a plain bullet",
            "drop the section marker to capture a task",
        ),
        TypeMarkerSubject::SubBullet => (
            "adds a child bullet",
            "capture `@<route>+<block-id> t:<type>` alone to retype that task",
        ),
        TypeMarkerSubject::ProjectNote => {
            ("creates a project note", "set `task_type:` in the new note")
        }
        TypeMarkerSubject::Reference => (
            "queues a reference typed by `plan.reading_type`",
            "add `-R` to keep it as a task",
        ),
        TypeMarkerSubject::PomodoroNote => {
            ("adds a Pomodoro note", "add task text to capture a task")
        }
        TypeMarkerSubject::TaskComplete => (
            "completes an existing task",
            "remove the completion gesture to capture a task",
        ),
        TypeMarkerSubject::ExistingLink => (
            "links an existing task",
            "capture the link alone to retype that task",
        ),
        TypeMarkerSubject::TaskToggle => (
            "toggles an existing task",
            "capture the toggle alone to retype that task",
        ),
        TypeMarkerSubject::Close => (
            "closes a Pomodoro",
            "remove the close gesture to capture a task",
        ),
        TypeMarkerSubject::Start => (
            "starts a Pomodoro",
            "remove the start gesture to capture a task",
        ),
        TypeMarkerSubject::Adjust => (
            "adjusts a Pomodoro duration",
            "remove the adjustment gesture to capture a task",
        ),
        TypeMarkerSubject::Shift => (
            "shifts a Pomodoro session",
            "remove the shift gesture to capture a task",
        ),
    };
    format!("`{marker}` types a task, but this capture {action}; {remedy}")
}

/// Classify whether a marker's parsed item is a new task subject.
pub(crate) fn is_new_task(parsed: &ParsedCaptureText) -> bool {
    matches!(
        parsed.kind,
        super::model::CaptureKind::Task
            | super::model::CaptureKind::TaskWithBlockId { .. }
            | super::model::CaptureKind::Pomodoro { close: None, .. }
    ) && !parsed.body.trim().is_empty()
}

/// Return the shared teaching error when a type marker no longer has a new
/// task as its final subject. Global and forced destinations can change an
/// initially ordinary task into a child after the item parser runs.
pub(crate) fn not_applicable_error(
    parsed: &ParsedCaptureText,
) -> Option<String> {
    let marker = parsed.type_marker.as_ref()?;
    (!is_new_task(parsed)).then(|| {
        type_marker_not_applicable_error(
            &marker.name,
            TypeMarkerSubject::for_kind(&parsed.kind),
        )
    })
}

/// Classify whether a marker's parsed item retypes the existing task it
/// targets: whole-item links (without a close suffix), Ensure Next and
/// toggle forms, and relationship-only existing owners. Mirrors the
/// phase-`existing` rows of the plan's §1.6 table.
pub(crate) fn is_existing_type_marker_subject(
    parsed: &ParsedCaptureText,
) -> bool {
    use super::model::{CaptureKind, ParsedDependencyTargetKind};
    match &parsed.kind {
        CaptureKind::PomodoroLink { close, .. } => close.is_none(),
        CaptureKind::TaskToggle { .. } => true,
        CaptureKind::SubBullet { target, .. } => {
            use super::model::SubBulletTarget;
            matches!(target, SubBulletTarget::BlockId(_))
                && parsed.body.trim().is_empty()
                && parsed.sub_bullets.is_empty()
                && matches!(
                    parsed.dependency_target.as_ref(),
                    Some(target) if target.kind
                        == ParsedDependencyTargetKind::ExistingTask
                )
        }
        _ => false,
    }
}

/// Subject judgment for a refused marker on a link-close form: a
/// whole-item link carrying a close suffix refuses as a close gesture,
/// not as an existing link.
pub(crate) fn refusal_subject_for_kind(
    kind: &super::model::CaptureKind,
) -> TypeMarkerSubject {
    use super::model::CaptureKind;
    match kind {
        CaptureKind::PomodoroLink { close, .. } if close.is_some() => {
            TypeMarkerSubject::Close
        }
        _ => TypeMarkerSubject::for_kind(kind),
    }
}
