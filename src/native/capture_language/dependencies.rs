//! `&note:block-id` dependency modifiers: lexical scanning and line extraction.
//!
//! This module owns the contract-phase grammar for task dependencies. It is
//! purely lexical and filesystem-free: it scans one physical line for
//! ampersand modifiers, classifies each as complete, partial (still being
//! typed), invalid, or escaped-literal, and strips directive modifiers from
//! the leading and trailing marker runs so the established `@`/schedule/
//! priority/clipboard pipeline resolves the remaining tokens unchanged.
//!
//! Mid-line ampersands stay literal body text: only modifiers in the
//! leading run (parent line only) or the trailing run (past `@...`/`#`
//! destination markers) are directives. `\&` consumes only the backslash
//! and leaves the visible `&...` in task text. `&` inside `[[wikilinks]]`
//! and `` `code` `` spans is never a modifier.

use super::draft::{classify_authored_line, AuthoredLineClass};
use super::editor_model::{DependencyEntry, RelationEntry, Span, SpanKind};
use super::editor_parse::tokenize_line_with_spans;
use super::markers::{
    is_escaped_priority_token, is_priority_marker_token, parse_clip_token,
    parse_schedule_token,
};
use super::model::{
    CaptureItem, CaptureKind, ParsedDependency, ParsedDependencyTarget,
    ParsedDependencyTargetKind, ParsedRelation, RawLine, SubBulletTarget,
    TaskToggleIntent, Token,
};
use super::relations::{scan_line_relations, RelationRef, ScannedRelation};

/// One complete `&note:block-id` modifier with original-draft byte offsets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DependencyRef {
    /// Exact typed text, sigil included (`&projects/foo:bar`).
    pub(crate) raw: String,
    /// Vault-relative note identity exactly as typed (quotes removed,
    /// escapes decoded, case and Unicode preserved).
    pub(crate) note: String,
    /// Prerequisite block ID exactly as typed.
    pub(crate) block_id: String,
    /// Whether the note was a `"quoted component"`.
    pub(crate) quoted: bool,
    /// Whole-modifier range, sigil included.
    pub(crate) start: usize,
    pub(crate) end: usize,
    /// Note component range as typed (inside the quotes when quoted).
    pub(crate) note_start: usize,
    pub(crate) note_end: usize,
    /// Block-ID range.
    pub(crate) block_start: usize,
    pub(crate) block_end: usize,
}

/// One malformed modifier with its repairable message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct InvalidDependency {
    pub(crate) raw: String,
    pub(crate) message: String,
    pub(crate) start: usize,
    pub(crate) end: usize,
}

/// One `&` occurrence classified by the line scanner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ScannedDependency {
    Complete(DependencyRef),
    /// `&`, `&query`, `&note:`, or an unterminated quoted note: the
    /// prerequisite picker is still open.
    Partial {
        raw: String,
        start: usize,
        end: usize,
    },
    Invalid(InvalidDependency),
    /// `\&...` literal: `backslash` is the consumed `\` byte; the visible
    /// token runs `start..end` (`&` included).
    Escaped {
        backslash: usize,
        start: usize,
        end: usize,
    },
}

impl ScannedDependency {
    pub(crate) fn start(&self) -> usize {
        match self {
            Self::Complete(entry) => entry.start,
            Self::Partial { start, .. } => *start,
            Self::Invalid(invalid) => invalid.start,
            Self::Escaped { start, .. } => *start,
        }
    }

    pub(crate) fn end(&self) -> usize {
        match self {
            Self::Complete(entry) => entry.end,
            Self::Partial { end, .. } => *end,
            Self::Invalid(invalid) => invalid.end,
            Self::Escaped { end, .. } => *end,
        }
    }
}

/// Byte ranges of `[[wikilink]]` and `` `code` `` spans on one line: `&`
/// inside them is literal text, never a modifier.
pub(crate) fn protected_ranges(line: &str) -> Vec<(usize, usize)> {
    let mut ranges = Vec::new();
    let bytes = line.as_bytes();
    let mut index = 0;
    while index < line.len() {
        if line[index..].starts_with("[[") {
            if let Some(close) = line[index + 2..].find("]]") {
                ranges.push((index, index + 2 + close + 2));
                index += 2 + close + 2;
                continue;
            }
            break;
        }
        if bytes[index] == b'`' {
            if let Some(next) = line[index + 1..].find('`') {
                ranges.push((index, index + 1 + next + 1));
                index += 1 + next + 1;
                continue;
            }
            break;
        }
        index += char_len(line, index);
    }
    ranges
}

/// Whether the byte before `amp` starts a token: line start or whitespace
/// (matching [`super::line::normalize_task_text`]'s `split_whitespace`
/// boundaries).
pub(crate) fn is_token_start(line: &str, amp: usize) -> bool {
    amp == 0
        || line[..amp]
            .chars()
            .next_back()
            .is_some_and(|previous| previous.is_whitespace())
}

/// The start byte of the `\` run directly before `amp`, with its length.
/// Returns `None` when no backslash precedes `amp`.
pub(crate) fn slash_run_before(
    line: &str,
    amp: usize,
) -> Option<(usize, usize)> {
    let mut index = amp;
    while index > 0 && line.as_bytes()[index - 1] == b'\\' {
        index -= 1;
    }
    (index < amp).then_some((index, amp - index))
}

/// Whether `amp` is an escaped literal: an odd `\` run whose own first
/// backslash opens the token (`\&...` at a token start).
pub(crate) fn escape_at(line: &str, amp: usize) -> Option<usize> {
    let (start, len) = slash_run_before(line, amp)?;
    (len % 2 == 1 && is_token_start(line, start)).then_some(start)
}

pub(crate) fn char_len(line: &str, index: usize) -> usize {
    line[index..]
        .chars()
        .next()
        .map(|character| character.len_utf8())
        .unwrap_or(1)
}

pub(crate) fn is_space_at(line: &str, index: usize) -> bool {
    line[index..]
        .chars()
        .next()
        .is_some_and(|character| character.is_whitespace())
}

/// Decode the inside of a quoted note: only `\"` and `\\` are escapes;
/// every other backslash stays literal.
pub(crate) fn decode_quoted_note(raw: &str) -> String {
    let mut decoded = String::with_capacity(raw.len());
    let bytes = raw.as_bytes();
    let mut index = 0;
    while index < raw.len() {
        let character = raw[index..].chars().next().expect("char boundary");
        if character == '\\'
            && index + 1 < raw.len()
            && matches!(bytes[index + 1], b'"' | b'\\')
        {
            decoded.push(bytes[index + 1] as char);
            index += 1 + character.len_utf8();
            continue;
        }
        decoded.push(character);
        index += character.len_utf8();
    }
    decoded
}

/// Validate one vault-relative note identity, returning the repairable
/// message when it cannot name a note. `sigil` renders the example in the
/// empty-note message (`&` for dependencies, `!` for completions).
pub(crate) fn validate_note(note: &str, sigil: u8) -> Option<String> {
    if note.is_empty() {
        return Some(format!(
            "dependency note is empty: use '{sigil}note:block-id' \
             (for spaces, quote the note: '{sigil}\"My Note\":block-id')",
            sigil = sigil as char,
        ));
    }
    if note.starts_with('/') {
        return Some(format!(
            "dependency note '{note}' must be vault-relative, not absolute"
        ));
    }
    if note.contains('*') {
        return Some(format!(
            "dependency note '{note}' contains '*', which is not allowed in the note component"
        ));
    }
    for component in note.split('/') {
        if component.is_empty() {
            return Some(format!(
                "dependency note '{note}' has an empty path component"
            ));
        }
        if component == "." || component == ".." {
            return Some(format!(
                "dependency note '{note}' must not contain '.' or '..'"
            ));
        }
    }
    None
}

/// Validate one prerequisite block ID against Bob's existing rules.
/// `sigil` renders the example (`&` for dependencies, `!` for completions).
pub(crate) fn validate_block_id(block_id: &str, sigil: u8) -> Option<String> {
    if !super::tokens::is_block_id(block_id) {
        return Some(format!(
            "dependency block ID '{block_id}' must contain only A-Z, a-z, \
             0-9 or '-' (use '{sigil}note:block-id')",
            sigil = sigil as char,
        ));
    }
    None
}

/// Scan one physical line for `&` modifiers. `base` is the line's byte
/// offset in the original draft, so every range is draft-absolute.
pub(crate) fn scan_line_dependencies(
    line: &str,
    base: usize,
) -> Vec<ScannedDependency> {
    scan_line_modifiers(line, base, b'&')
}

/// Scan one physical line for `*` blocked-task modifiers. `base` is the
/// line's byte offset in the original draft, so every range is
/// draft-absolute.
pub(crate) fn scan_line_blocked_tasks(
    line: &str,
    base: usize,
) -> Vec<ScannedDependency> {
    scan_line_modifiers(line, base, b'*')
}

fn scan_line_modifiers(
    line: &str,
    base: usize,
    sigil: u8,
) -> Vec<ScannedDependency> {
    let protected = protected_ranges(line);
    let mut found = Vec::new();
    let mut index = 0;
    while index < line.len() {
        if line.as_bytes()[index] != sigil {
            index += char_len(line, index);
            continue;
        }
        if sigil == b'*' {
            if line.as_bytes().get(index + 1) == Some(&b'*') {
                index += 1;
                continue;
            }
            let mut token_end = index + 1;
            while token_end < line.len() && !is_space_at(line, token_end) {
                token_end += char_len(line, token_end);
            }
            let token = &line[index..token_end];
            if let Some(extra_star) = token[1..].find('*') {
                let extra_star = extra_star + 1;
                let colon = token.find(':');
                if colon.is_none_or(|colon| colon < extra_star) {
                    index = token_end;
                    continue;
                }
            }
        }
        // An escape opens at its own backslash, so it is checked before
        // the token-start boundary (the `&` itself never is one).
        if let Some(backslash) = escape_at(line, index) {
            // The visible token is the whitespace-delimited run starting
            // at the backslash; only the `\` is consumed.
            let mut end = index + 1;
            while end < line.len() && !is_space_at(line, end) {
                end += char_len(line, end);
            }
            found.push(ScannedDependency::Escaped {
                backslash: base + backslash,
                start: base + index,
                end: base + end,
            });
            index = end;
            continue;
        }
        if !is_token_start(line, index)
            || protected
                .iter()
                .any(|(start, end)| *start <= index && index < *end)
        {
            index += 1;
            continue;
        }
        found.push(scan_modifier_with_sigil(line, base, index, sigil));
        let end = found.last().expect("pushed").end() - base;
        index = end.max(index + 1);
    }
    found
}

/// Classify the modifier starting at `sigil` byte `amp`, generalized over
/// the `&` dependency and `!` completion sigils.
fn scan_modifier_with_sigil(
    line: &str,
    base: usize,
    amp: usize,
    sigil: u8,
) -> ScannedDependency {
    let start = base + amp;
    let mut cursor = amp + 1;
    if cursor >= line.len() || is_space_at(line, cursor) {
        return ScannedDependency::Partial {
            raw: line[amp..cursor].to_string(),
            start,
            end: base + cursor,
        };
    }
    if line.as_bytes()[cursor] == b'"' {
        return scan_quoted_modifier(line, base, amp, sigil);
    }
    // Unquoted note: runs to the first `:` (or whitespace/end, which
    // leaves a partial query).
    let note_start = cursor;
    while cursor < line.len()
        && line.as_bytes()[cursor] != b':'
        && !is_space_at(line, cursor)
    {
        cursor += char_len(line, cursor);
    }
    if cursor >= line.len() || is_space_at(line, cursor) {
        return ScannedDependency::Partial {
            raw: line[amp..cursor].to_string(),
            start,
            end: base + cursor,
        };
    }
    let note = line[note_start..cursor].to_string();
    scan_block_id(line, base, amp, start, note, false, cursor, sigil)
}

/// Scan one whole-item `!note:block-id` token at the start of a trimmed
/// line. `line` must start with `!`; `base` is its draft byte offset.
/// Quote-aware through the shared `&` locator rules, so
/// `!"Shopping List":milk` scans as one token.
pub(crate) fn scan_bang_token(line: &str, base: usize) -> ScannedDependency {
    debug_assert!(line.starts_with('!'));
    scan_modifier_with_sigil(line, base, 0, b'!')
}

/// Classify a quoted-note modifier starting at `sigil` byte `amp`.
fn scan_quoted_modifier(
    line: &str,
    base: usize,
    amp: usize,
    sigil: u8,
) -> ScannedDependency {
    let start = base + amp;
    // `cursor` walks the bytes after the opening quote.
    let mut cursor = amp + 2;
    let mut closed = None;
    while cursor < line.len() {
        let byte = line.as_bytes()[cursor];
        if byte == b'\\' {
            cursor += 1;
            if cursor < line.len() {
                cursor += char_len(line, cursor);
            }
            continue;
        }
        if byte == b'"' {
            closed = Some(cursor);
            break;
        }
        cursor += char_len(line, cursor);
    }
    let Some(close) = closed else {
        // An unterminated quote stays one partial modifier through the
        // end of the line.
        return ScannedDependency::Partial {
            raw: line[amp..].to_string(),
            start,
            end: base + line.len(),
        };
    };
    let raw_note = line[amp + 2..close].to_string();
    let after = close + 1;
    if after >= line.len() || line.as_bytes()[after] != b':' {
        // `&"note"` without `:` yet: still typing.
        return ScannedDependency::Partial {
            raw: line[amp..after].to_string(),
            start,
            end: base + after,
        };
    }
    let note = decode_quoted_note(&raw_note);
    scan_block_id(line, base, amp, start, note, true, after, sigil)
}

/// Classify the `:block-id` tail: `colon` is the byte index of the `:`
/// separator. `note_end` reporting covers the typed component (inside the
/// quotes when quoted). `sigil` renders the validation examples.
#[allow(clippy::too_many_arguments)]
fn scan_block_id(
    line: &str,
    base: usize,
    amp: usize,
    start: usize,
    note: String,
    quoted: bool,
    colon: usize,
    sigil: u8,
) -> ScannedDependency {
    let mut end = colon + 1;
    while end < line.len() && !is_space_at(line, end) {
        end += char_len(line, end);
    }
    let raw = line[amp..end].to_string();
    if end == colon + 1 {
        return ScannedDependency::Partial {
            raw,
            start,
            end: base + end,
        };
    }
    if let Some(message) = validate_note(&note, sigil) {
        return ScannedDependency::Invalid(InvalidDependency {
            raw,
            message,
            start,
            end: base + end,
        });
    }
    let block_id = line[colon + 1..end].to_string();
    if let Some(message) = validate_block_id(&block_id, sigil) {
        return ScannedDependency::Invalid(InvalidDependency {
            raw,
            message,
            start,
            end: base + end,
        });
    }
    let (note_start, note_end) = if quoted {
        // Inside the quotes: the closing quote sits just before `colon`.
        (base + amp + 2, base + colon - 1)
    } else {
        (base + amp + 1, base + colon)
    };
    ScannedDependency::Complete(DependencyRef {
        raw,
        note,
        block_id: block_id.clone(),
        quoted,
        start,
        end: base + end,
        note_start,
        note_end,
        block_start: base + colon + 1,
        block_end: base + end,
    })
}

/// One line's extracted modifiers plus the editor spans for them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct LineDependencies {
    /// Complete modifiers in source order.
    pub(crate) entries: Vec<DependencyRef>,
    /// Complete `*` modifiers in source order.
    pub(crate) blocked_entries: Vec<DependencyRef>,
    /// Complete `~` / `~~` modifiers in source order.
    pub(crate) relation_entries: Vec<RelationRef>,
    /// Partial-query ranges in source order.
    pub(crate) partials: Vec<(usize, usize)>,
    /// Partial `*` modifier ranges.
    pub(crate) blocked_partials: Vec<(usize, usize)>,
    /// Partial `~` / `~~` target ranges (need `relation_target`).
    pub(crate) relation_target_partials: Vec<(usize, usize)>,
    /// Partial `|label` ranges (need `relation_label`).
    pub(crate) relation_label_partials: Vec<(usize, usize)>,
    /// Malformed modifiers in source order.
    pub(crate) invalid: Vec<InvalidDependency>,
    /// Malformed `*` modifiers in source order.
    pub(crate) blocked_invalid: Vec<InvalidDependency>,
    /// Malformed `~` / `~~` modifiers in source order.
    pub(crate) relation_invalid: Vec<InvalidDependency>,
    /// Semantic spans for every extracted modifier.
    pub(crate) spans: Vec<Span>,
    /// Consumed `\` bytes of `\&`/`\*`/`\~` escapes (for body unescaping).
    pub(crate) escapes: Vec<(usize, usize)>,
    /// Whitespace tokens consumed as directive modifiers.
    pub(crate) stripped_tokens: usize,
}

/// Whether a trailing-run token is a marker a dependency may interleave
/// with (in either order): `@...` destination shapes, the bare `#`
/// Pomodoro-note marker, and schedule/priority/clipboard markers (whole
/// or still-typed). Skipped tokens stay in the stream for terminal-marker
/// extraction and route selection; only modifiers are directive.
pub(crate) fn is_trailing_transparent(text: &str, parse_clip: bool) -> bool {
    text.starts_with('@')
        || text == "#"
        || text == "s:"
        || parse_schedule_token(text).is_some()
        || is_priority_marker_token(text)
        || is_escaped_priority_token(text)
        || (parse_clip && parse_clip_token(text).is_some())
}

fn escaped_star_is_in_modifier_run(
    backslash: usize,
    tokens: &[Token<'_>],
    consumed: &[bool],
    parent: bool,
    parse_clip: bool,
) -> bool {
    let Some(token_index) =
        tokens.iter().position(|token| token.start == backslash)
    else {
        return false;
    };
    let token = tokens[token_index].text;
    let has_query = token
        .strip_prefix("\\*")
        .is_some_and(|query| !query.is_empty());

    if parent {
        let mut index = 0;
        while index < tokens.len() {
            if index == token_index {
                return has_query;
            }
            if consumed[index]
                || is_trailing_transparent(tokens[index].text, parse_clip)
                || tokens[index].text.starts_with("\\&")
                || tokens[index].text.starts_with("\\*")
            {
                index += 1;
                continue;
            }
            break;
        }
    }

    let mut index = tokens.len();
    while index > 0 {
        let candidate = index - 1;
        if candidate == token_index {
            return has_query || token_index > 0;
        }
        if consumed[candidate]
            || is_trailing_transparent(tokens[candidate].text, parse_clip)
            || tokens[candidate].text.starts_with("\\&")
            || tokens[candidate].text.starts_with("\\*")
        {
            index -= 1;
            continue;
        }
        break;
    }
    false
}

/// Strip directive dependency modifiers from a line's whitespace tokens.
///
/// Leading modifiers are directive on the parent line only; trailing
/// modifiers are directive past destination and schedule/priority/
/// clipboard markers. Returns the remaining tokens (order preserved) plus
/// the extracted modifiers. `line_text` is the raw line the tokens
/// borrow; `line_base` is its draft byte offset. Runs before
/// terminal-marker extraction so interleaved markers still resolve.
pub(crate) fn extract_line_dependencies<'a>(
    tokens: Vec<Token<'a>>,
    line_text: &str,
    line_base: usize,
    parent: bool,
    parse_clip: bool,
) -> (Vec<Token<'a>>, LineDependencies) {
    let (tokens, mut result) = extract_line_relation(
        tokens, line_text, line_base, parent, parse_clip, b'&',
    );
    let (tokens, mut blocked) = extract_line_relation(
        tokens, line_text, line_base, parent, parse_clip, b'*',
    );
    result.blocked_entries = std::mem::take(&mut blocked.blocked_entries);
    result.blocked_partials = std::mem::take(&mut blocked.blocked_partials);
    result.blocked_invalid = std::mem::take(&mut blocked.blocked_invalid);
    result.spans.append(&mut blocked.spans);
    result.escapes.append(&mut blocked.escapes);
    result.stripped_tokens += blocked.stripped_tokens;
    let (tokens, mut relations) = extract_line_relations(
        tokens, line_text, line_base, parent, parse_clip,
    );
    result.relation_entries = std::mem::take(&mut relations.relation_entries);
    result.relation_target_partials =
        std::mem::take(&mut relations.relation_target_partials);
    result.relation_label_partials =
        std::mem::take(&mut relations.relation_label_partials);
    result.relation_invalid = std::mem::take(&mut relations.relation_invalid);
    result.spans.append(&mut relations.spans);
    result.escapes.append(&mut relations.escapes);
    result.stripped_tokens += relations.stripped_tokens;
    result.spans.sort_by_key(|span| (span.start, span.end));
    (tokens, result)
}

fn extract_line_relation<'a>(
    tokens: Vec<Token<'a>>,
    line_text: &str,
    line_base: usize,
    parent: bool,
    parse_clip: bool,
    sigil: u8,
) -> (Vec<Token<'a>>, LineDependencies) {
    let scanned = scan_line_modifiers(line_text, line_base, sigil);
    if scanned.is_empty() {
        return (tokens, LineDependencies::default());
    }
    // Whitespace-token index -> covering scanned index, for tokens fully
    // inside one scanned range (a quoted note with spaces covers several).
    let mut covering: Vec<Option<usize>> = vec![None; tokens.len()];
    let other_sigil = if sigil == b'&' { b'*' } else { b'&' };
    let other_scanned = scan_line_modifiers(line_text, line_base, other_sigil);
    let relation_scanned = scan_line_relations(line_text, line_base);
    let covered_by_other_relation = |token: &Token<'_>| {
        other_scanned
            .iter()
            .any(|scan| scan.start() <= token.start && token.end <= scan.end())
            || relation_scanned.iter().any(|scan| {
                scan.start() <= token.start && token.end <= scan.end()
            })
    };
    for (token_index, token) in tokens.iter().enumerate() {
        for (scan_index, scan) in scanned.iter().enumerate() {
            if matches!(scan, ScannedDependency::Escaped { .. }) {
                continue;
            }
            if scan.start() <= token.start && token.end <= scan.end() {
                covering[token_index] = Some(scan_index);
                break;
            }
        }
    }
    // A bare `&` works at either run edge. A bare `*` is only an editor
    // trigger after existing text at the line end; line-initial `*` stays
    // a Markdown list marker.
    let bare_directive = |token_index: usize| -> bool {
        if sigil == b'*' {
            token_index > 0 && token_index + 1 == tokens.len()
        } else {
            (parent && token_index == 0) || token_index + 1 == tokens.len()
        }
    };
    let is_directive = |token_index: usize, scan_index: usize| -> bool {
        match &scanned[scan_index] {
            ScannedDependency::Escaped { .. } => false,
            ScannedDependency::Partial { raw, .. }
                if raw.as_bytes() == &[sigil] =>
            {
                bare_directive(token_index)
            }
            _ => true,
        }
    };

    let mut consumed = vec![false; tokens.len()];
    // Leading run (parent line only).
    if parent {
        let mut index = 0;
        while index < tokens.len() {
            if is_trailing_transparent(tokens[index].text, parse_clip)
                || covered_by_other_relation(&tokens[index])
                || tokens[index].text.starts_with("\\&")
                || tokens[index].text.starts_with("\\*")
                || tokens[index].text.starts_with("\\~")
            {
                index += 1;
                continue;
            }
            match covering[index] {
                Some(scan_index) if is_directive(index, scan_index) => {
                    // Consume the whole scanned group (a quoted note may
                    // span several whitespace tokens).
                    let (start, end) = {
                        let scan = &scanned[scan_index];
                        (scan.start(), scan.end())
                    };
                    for (other, token) in tokens.iter().enumerate() {
                        if start <= token.start && token.end <= end {
                            consumed[other] = true;
                        }
                    }
                    while index < tokens.len() && consumed[index] {
                        index += 1;
                    }
                }
                _ => break,
            }
        }
    }
    // Trailing run, past destination markers (escaped `\&` tokens are
    // transparent: they stay literal but do not stop the run).
    {
        let mut index = tokens.len();
        while index > 0 {
            let candidate = index - 1;
            if consumed[candidate] {
                index -= 1;
                continue;
            }
            if let Some(scan_index) = covering[candidate] {
                if is_directive(candidate, scan_index) {
                    let (start, end) = {
                        let scan = &scanned[scan_index];
                        (scan.start(), scan.end())
                    };
                    for (other, token) in tokens.iter().enumerate() {
                        if start <= token.start && token.end <= end {
                            consumed[other] = true;
                        }
                    }
                    index = candidate;
                    continue;
                }
                break;
            }
            if is_trailing_transparent(tokens[candidate].text, parse_clip) {
                index -= 1;
                continue;
            }
            if covered_by_other_relation(&tokens[candidate]) {
                index -= 1;
                continue;
            }
            if tokens[candidate].text.starts_with("\\&")
                || tokens[candidate].text.starts_with("\\*")
                || tokens[candidate].text.starts_with("\\~")
            {
                index -= 1;
                continue;
            }
            break;
        }
    }

    let mut result = LineDependencies::default();
    // Spans follow source order: walk the scans once.
    for scan in &scanned {
        match scan {
            ScannedDependency::Complete(entry) => {
                let kept =
                    tokens.iter().enumerate().any(|(token_index, token)| {
                        !consumed[token_index]
                            && entry.start <= token.start
                            && token.end <= entry.end
                    });
                if kept {
                    continue;
                }
                result.spans.push(Span {
                    start: entry.start,
                    end: entry.start + 1,
                    kind: if sigil == b'*' {
                        SpanKind::BlockedTaskSigil
                    } else {
                        SpanKind::DependencySigil
                    },
                });
                result.spans.push(Span {
                    start: entry.note_start,
                    end: entry.note_end,
                    kind: if sigil == b'*' {
                        SpanKind::BlockedTaskNote
                    } else {
                        SpanKind::DependencyNote
                    },
                });
                result.spans.push(Span {
                    start: entry.block_start,
                    end: entry.block_end,
                    kind: if sigil == b'*' {
                        SpanKind::BlockedTaskBlockId
                    } else {
                        SpanKind::DependencyBlockId
                    },
                });
                if sigil == b'*' {
                    result.blocked_entries.push(entry.clone());
                } else {
                    result.entries.push(entry.clone());
                }
            }
            ScannedDependency::Partial { start, end, .. } => {
                let kept =
                    tokens.iter().enumerate().any(|(token_index, token)| {
                        !consumed[token_index]
                            && *start <= token.start
                            && token.end <= *end
                    });
                if kept {
                    continue;
                }
                result.spans.push(Span {
                    start: *start,
                    end: *end,
                    kind: SpanKind::InteractivePlaceholder,
                });
                if sigil == b'*' {
                    result.blocked_partials.push((*start, *end));
                } else {
                    result.partials.push((*start, *end));
                }
            }
            ScannedDependency::Invalid(invalid) => {
                let kept =
                    tokens.iter().enumerate().any(|(token_index, token)| {
                        !consumed[token_index]
                            && invalid.start <= token.start
                            && token.end <= invalid.end
                    });
                if kept {
                    continue;
                }
                result.spans.push(Span {
                    start: invalid.start,
                    end: invalid.end,
                    kind: SpanKind::InteractivePlaceholder,
                });
                if sigil == b'*' {
                    result.blocked_invalid.push(invalid.clone());
                } else {
                    result.invalid.push(invalid.clone());
                }
            }
            ScannedDependency::Escaped { backslash, .. } => {
                if sigil != b'*'
                    || escaped_star_is_in_modifier_run(
                        *backslash, &tokens, &consumed, parent, parse_clip,
                    )
                {
                    result.escapes.push((*backslash, *backslash + 1));
                }
            }
        }
    }
    result.stripped_tokens =
        consumed.iter().filter(|consumed| **consumed).count();
    let remaining = tokens
        .into_iter()
        .enumerate()
        .filter(|(index, _)| !consumed[*index])
        .map(|(_, token)| token)
        .collect();
    (remaining, result)
}

fn escaped_tilde_is_in_modifier_run(
    backslash: usize,
    tokens: &[Token<'_>],
    consumed: &[bool],
    parent: bool,
    parse_clip: bool,
) -> bool {
    let Some(token_index) =
        tokens.iter().position(|token| token.start == backslash)
    else {
        return false;
    };
    let token = tokens[token_index].text;
    let has_query = token
        .strip_prefix("\\~~")
        .is_some_and(|query| !query.is_empty())
        || token
            .strip_prefix("\\~")
            .is_some_and(|query| !query.is_empty());

    if parent {
        let mut index = 0;
        while index < tokens.len() {
            if index == token_index {
                return has_query;
            }
            if consumed[index]
                || is_trailing_transparent(tokens[index].text, parse_clip)
                || tokens[index].text.starts_with("\\&")
                || tokens[index].text.starts_with("\\*")
                || tokens[index].text.starts_with("\\~")
            {
                index += 1;
                continue;
            }
            break;
        }
    }

    let mut index = tokens.len();
    while index > 0 {
        let candidate = index - 1;
        if candidate == token_index {
            return has_query || token_index > 0;
        }
        if consumed[candidate]
            || is_trailing_transparent(tokens[candidate].text, parse_clip)
            || tokens[candidate].text.starts_with("\\&")
            || tokens[candidate].text.starts_with("\\*")
            || tokens[candidate].text.starts_with("\\~")
        {
            index -= 1;
            continue;
        }
        break;
    }
    false
}

fn extract_line_relations<'a>(
    tokens: Vec<Token<'a>>,
    line_text: &str,
    line_base: usize,
    parent: bool,
    parse_clip: bool,
) -> (Vec<Token<'a>>, LineDependencies) {
    let scanned = scan_line_relations(line_text, line_base);
    if scanned.is_empty() {
        return (tokens, LineDependencies::default());
    }
    let mut covering: Vec<Option<usize>> = vec![None; tokens.len()];
    let amp_scanned = scan_line_modifiers(line_text, line_base, b'&');
    let star_scanned = scan_line_modifiers(line_text, line_base, b'*');
    let covered_by_other_relation = |token: &Token<'_>| {
        amp_scanned
            .iter()
            .any(|scan| scan.start() <= token.start && token.end <= scan.end())
            || star_scanned.iter().any(|scan| {
                scan.start() <= token.start && token.end <= scan.end()
            })
    };
    for (token_index, token) in tokens.iter().enumerate() {
        for (scan_index, scan) in scanned.iter().enumerate() {
            if matches!(scan, ScannedRelation::Escaped { .. }) {
                continue;
            }
            if scan.start() <= token.start && token.end <= scan.end() {
                covering[token_index] = Some(scan_index);
                break;
            }
        }
    }
    // A bare `~` / `~~` works as the first parent token or as the last
    // token with other text; line-initial `~~` alone on a child stays
    // literal, and mid-line bare sigils stay literal via the runs below.
    let bare_directive = |token_index: usize| -> bool {
        (parent && token_index == 0)
            || (token_index > 0 && token_index + 1 == tokens.len())
    };
    let is_directive = |token_index: usize, scan_index: usize| -> bool {
        match &scanned[scan_index] {
            ScannedRelation::Escaped { .. } => false,
            ScannedRelation::PartialTarget { raw, .. }
                if raw == "~" || raw == "~~" =>
            {
                bare_directive(token_index)
            }
            _ => true,
        }
    };

    let mut consumed = vec![false; tokens.len()];
    if parent {
        let mut index = 0;
        while index < tokens.len() {
            if is_trailing_transparent(tokens[index].text, parse_clip)
                || covered_by_other_relation(&tokens[index])
                || tokens[index].text.starts_with("\\&")
                || tokens[index].text.starts_with("\\*")
                || tokens[index].text.starts_with("\\~")
            {
                index += 1;
                continue;
            }
            match covering[index] {
                Some(scan_index) if is_directive(index, scan_index) => {
                    let (start, end) = {
                        let scan = &scanned[scan_index];
                        (scan.start(), scan.end())
                    };
                    for (other, token) in tokens.iter().enumerate() {
                        if start <= token.start && token.end <= end {
                            consumed[other] = true;
                        }
                    }
                    while index < tokens.len() && consumed[index] {
                        index += 1;
                    }
                }
                _ => break,
            }
        }
    }
    {
        let mut index = tokens.len();
        while index > 0 {
            let candidate = index - 1;
            if consumed[candidate] {
                index -= 1;
                continue;
            }
            if let Some(scan_index) = covering[candidate] {
                if is_directive(candidate, scan_index) {
                    let (start, end) = {
                        let scan = &scanned[scan_index];
                        (scan.start(), scan.end())
                    };
                    for (other, token) in tokens.iter().enumerate() {
                        if start <= token.start && token.end <= end {
                            consumed[other] = true;
                        }
                    }
                    index = candidate;
                    continue;
                }
                break;
            }
            if is_trailing_transparent(tokens[candidate].text, parse_clip) {
                index -= 1;
                continue;
            }
            if covered_by_other_relation(&tokens[candidate]) {
                index -= 1;
                continue;
            }
            if tokens[candidate].text.starts_with("\\&")
                || tokens[candidate].text.starts_with("\\*")
                || tokens[candidate].text.starts_with("\\~")
            {
                index -= 1;
                continue;
            }
            break;
        }
    }

    let mut result = LineDependencies::default();
    for scan in &scanned {
        match scan {
            ScannedRelation::Complete(entry) => {
                let kept =
                    tokens.iter().enumerate().any(|(token_index, token)| {
                        !consumed[token_index]
                            && entry.start <= token.start
                            && token.end <= entry.end
                    });
                if kept {
                    continue;
                }
                result.spans.push(Span {
                    start: entry.start,
                    end: entry.start + entry.sigil_len(),
                    kind: SpanKind::RelationSigil,
                });
                result.spans.push(Span {
                    start: entry.note_start,
                    end: entry.note_end,
                    kind: SpanKind::RelationNote,
                });
                result.spans.push(Span {
                    start: entry.block_start,
                    end: entry.block_end,
                    kind: SpanKind::RelationBlockId,
                });
                if let Some(separator) = entry.separator {
                    result.spans.push(Span {
                        start: separator.0,
                        end: separator.1,
                        kind: SpanKind::RelationLabelSeparator,
                    });
                }
                if let (Some(label_start), Some(label_end)) =
                    (entry.label_start, entry.label_end)
                {
                    result.spans.push(Span {
                        start: label_start,
                        end: label_end,
                        kind: SpanKind::RelationLabel,
                    });
                }
                result.relation_entries.push(entry.clone());
            }
            ScannedRelation::PartialTarget { start, end, .. } => {
                let kept =
                    tokens.iter().enumerate().any(|(token_index, token)| {
                        !consumed[token_index]
                            && *start <= token.start
                            && token.end <= *end
                    });
                if kept {
                    continue;
                }
                result.spans.push(Span {
                    start: *start,
                    end: *end,
                    kind: SpanKind::InteractivePlaceholder,
                });
                result.relation_target_partials.push((*start, *end));
            }
            ScannedRelation::PartialLabel { start, end, .. } => {
                let kept =
                    tokens.iter().enumerate().any(|(token_index, token)| {
                        !consumed[token_index]
                            && *start <= token.start
                            && token.end <= *end
                    });
                if kept {
                    continue;
                }
                result.spans.push(Span {
                    start: *start,
                    end: *end,
                    kind: SpanKind::InteractivePlaceholder,
                });
                result.relation_label_partials.push((*start, *end));
            }
            ScannedRelation::Invalid(invalid) => {
                let kept =
                    tokens.iter().enumerate().any(|(token_index, token)| {
                        !consumed[token_index]
                            && invalid.start <= token.start
                            && token.end <= invalid.end
                    });
                if kept {
                    continue;
                }
                result.spans.push(Span {
                    start: invalid.start,
                    end: invalid.end,
                    kind: SpanKind::InteractivePlaceholder,
                });
                result.relation_invalid.push(InvalidDependency {
                    raw: invalid.raw.clone(),
                    message: invalid.message.clone(),
                    start: invalid.start,
                    end: invalid.end,
                });
            }
            ScannedRelation::Escaped { backslash, .. } => {
                if escaped_tilde_is_in_modifier_run(
                    *backslash, &tokens, &consumed, parent, parse_clip,
                ) {
                    result.escapes.push((*backslash, *backslash + 1));
                }
            }
        }
    }
    result.stripped_tokens =
        consumed.iter().filter(|consumed| **consumed).count();
    let remaining = tokens
        .into_iter()
        .enumerate()
        .filter(|(index, _)| !consumed[*index])
        .map(|(_, token)| token)
        .collect();
    (remaining, result)
}

/// Unescape one body token: a consumed relationship escape drops its
/// backslash, leaving the visible `&...`, `*...`, or `~...` in task text.
pub(crate) fn unescape_dependency_text(
    text: &str,
    token_start: usize,
    escapes: &[(usize, usize)],
) -> String {
    if (text.starts_with("\\&")
        || text.starts_with("\\*")
        || text.starts_with("\\~"))
        && escapes.contains(&(token_start, token_start + 1))
    {
        text[1..].to_string()
    } else {
        text.to_string()
    }
}

/// Decode a completion query from modifier text after the `&` sigil:
/// strip one opening quote and resolve `\"`/`\\` escapes.
pub(crate) fn decode_dependency_query(text: &str) -> String {
    let text = text.strip_prefix('"').unwrap_or(text);
    let mut decoded = String::with_capacity(text.len());
    let bytes = text.as_bytes();
    let mut index = 0;
    while index < text.len() {
        let character = text[index..].chars().next().expect("char boundary");
        if character == '\\'
            && index + 1 < text.len()
            && matches!(bytes[index + 1], b'"' | b'\\')
        {
            decoded.push(bytes[index + 1] as char);
            index += 1 + character.len_utf8();
            continue;
        }
        decoded.push(character);
        index += character.len_utf8();
    }
    decoded
}

/// Every dependency modifier found on one capture item, across its parent
/// and authored-child lines.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ItemDependencySet {
    pub(crate) entries: Vec<DependencyRef>,
    pub(crate) blocked_entries: Vec<DependencyRef>,
    pub(crate) relation_entries: Vec<RelationRef>,
    pub(crate) partials: Vec<(usize, usize)>,
    pub(crate) blocked_partials: Vec<(usize, usize)>,
    pub(crate) relation_target_partials: Vec<(usize, usize)>,
    pub(crate) relation_label_partials: Vec<(usize, usize)>,
    pub(crate) invalid: Vec<InvalidDependency>,
    pub(crate) blocked_invalid: Vec<InvalidDependency>,
    pub(crate) relation_invalid: Vec<InvalidDependency>,
}

impl ItemDependencySet {
    pub(crate) fn absorb(&mut self, line: &LineDependencies) {
        for entry in &line.entries {
            if !self.entries.iter().any(|known| {
                known.start == entry.start && known.end == entry.end
            }) {
                self.entries.push(entry.clone());
            }
        }
        for entry in &line.blocked_entries {
            if !self.blocked_entries.iter().any(|known| {
                known.start == entry.start && known.end == entry.end
            }) {
                self.blocked_entries.push(entry.clone());
            }
        }
        for entry in &line.relation_entries {
            if !self.relation_entries.iter().any(|known| {
                known.start == entry.start && known.end == entry.end
            }) {
                self.relation_entries.push(entry.clone());
            }
        }
        for partial in &line.partials {
            if !self.partials.contains(partial) {
                self.partials.push(*partial);
            }
        }
        for partial in &line.blocked_partials {
            if !self.blocked_partials.contains(partial) {
                self.blocked_partials.push(*partial);
            }
        }
        for partial in &line.relation_target_partials {
            if !self.relation_target_partials.contains(partial) {
                self.relation_target_partials.push(*partial);
            }
        }
        for partial in &line.relation_label_partials {
            if !self.relation_label_partials.contains(partial) {
                self.relation_label_partials.push(*partial);
            }
        }
        for invalid in &line.invalid {
            if !self.invalid.iter().any(|known| {
                known.start == invalid.start && known.end == invalid.end
            }) {
                self.invalid.push(invalid.clone());
            }
        }
        for invalid in &line.blocked_invalid {
            if !self.blocked_invalid.iter().any(|known| {
                known.start == invalid.start && known.end == invalid.end
            }) {
                self.blocked_invalid.push(invalid.clone());
            }
        }
        for invalid in &line.relation_invalid {
            if !self.relation_invalid.iter().any(|known| {
                known.start == invalid.start && known.end == invalid.end
            }) {
                self.relation_invalid.push(invalid.clone());
            }
        }
        self.entries.sort_by_key(|entry| (entry.start, entry.end));
        self.blocked_entries
            .sort_by_key(|entry| (entry.start, entry.end));
        self.relation_entries
            .sort_by_key(|entry| (entry.start, entry.end));
        self.partials.sort();
        self.blocked_partials.sort();
        self.relation_target_partials.sort();
        self.relation_label_partials.sort();
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.entries.is_empty()
            && self.blocked_entries.is_empty()
            && self.relation_entries.is_empty()
            && self.partials.is_empty()
            && self.blocked_partials.is_empty()
            && self.relation_target_partials.is_empty()
            && self.relation_label_partials.is_empty()
            && self.invalid.is_empty()
            && self.blocked_invalid.is_empty()
            && self.relation_invalid.is_empty()
    }

    pub(crate) fn has_entries(&self) -> bool {
        !self.entries.is_empty()
            || !self.blocked_entries.is_empty()
            || !self.relation_entries.is_empty()
    }

    pub(crate) fn has_partial_tokens(&self) -> bool {
        !self.partials.is_empty()
            || !self.blocked_partials.is_empty()
            || !self.relation_target_partials.is_empty()
            || !self.relation_label_partials.is_empty()
    }

    pub(crate) fn sigil(&self) -> u8 {
        let mut candidates: Vec<(usize, u8)> = Vec::new();
        candidates.extend(
            self.entries
                .iter()
                .map(|entry| (entry.start, b'&'))
                .chain(self.partials.iter().map(|partial| (partial.0, b'&')))
                .chain(
                    self.invalid.iter().map(|invalid| (invalid.start, b'&')),
                ),
        );
        candidates.extend(
            self.blocked_entries
                .iter()
                .map(|entry| (entry.start, b'*'))
                .chain(
                    self.blocked_partials
                        .iter()
                        .map(|partial| (partial.0, b'*')),
                )
                .chain(
                    self.blocked_invalid
                        .iter()
                        .map(|invalid| (invalid.start, b'*')),
                ),
        );
        candidates.extend(
            self.relation_entries
                .iter()
                .map(|entry| (entry.start, b'~'))
                .chain(
                    self.relation_target_partials
                        .iter()
                        .map(|partial| (partial.0, b'~')),
                )
                .chain(
                    self.relation_label_partials
                        .iter()
                        .map(|partial| (partial.0, b'~')),
                )
                .chain(
                    self.relation_invalid
                        .iter()
                        .map(|invalid| (invalid.start, b'~')),
                ),
        );
        candidates.sort_by_key(|(start, sigil)| (*start, *sigil));
        candidates.first().map(|(_, sigil)| *sigil).unwrap_or(b'&')
    }

    pub(crate) fn dependency_entries(&self) -> Vec<DependencyEntry> {
        self.entries
            .iter()
            .map(|entry| DependencyEntry {
                raw: entry.raw.clone(),
                note: entry.note.clone(),
                block_id: entry.block_id.clone(),
                quoted: entry.quoted,
                start: entry.start,
                end: entry.end,
            })
            .collect()
    }

    pub(crate) fn blocked_task_entries(&self) -> Vec<DependencyEntry> {
        self.blocked_entries
            .iter()
            .map(|entry| DependencyEntry {
                raw: entry.raw.clone(),
                note: entry.note.clone(),
                block_id: entry.block_id.clone(),
                quoted: entry.quoted,
                start: entry.start,
                end: entry.end,
            })
            .collect()
    }

    pub(crate) fn relation_entries(&self) -> Vec<RelationEntry> {
        self.relation_entries
            .iter()
            .map(|entry| RelationEntry {
                raw: entry.raw.clone(),
                direction: entry.direction,
                note: entry.note.clone(),
                block_id: entry.block_id.clone(),
                quoted: entry.quoted,
                hidden: entry.hidden,
                label: entry.label.clone(),
                start: entry.start,
                end: entry.end,
                label_start: entry.label_start,
                label_end: entry.label_end,
            })
            .collect()
    }
}

/// Scan one whole capture item (parent plus authored children) for
/// directive dependency modifiers. This is the single source of truth the
/// execution fail-closed gate and the operator-item rejection share with
/// the line-level editor extraction: the parent contributes its leading
/// and trailing runs, each authored child its trailing run.
pub(crate) fn scan_item_dependencies(
    item: &CaptureItem<'_>,
    parse_clip: bool,
) -> ItemDependencySet {
    let mut found = ItemDependencySet::default();
    let Some((parent, children)) = item.lines.split_first() else {
        return found;
    };
    let parent_tokens = tokenize_line_with_spans(&parent.raw);
    let (_, parent_deps) = extract_line_dependencies(
        parent_tokens,
        parent.raw.text,
        parent.raw.start,
        true,
        parse_clip,
    );
    found.absorb(&parent_deps);
    for child in children {
        let AuthoredLineClass::Item(authored) =
            classify_authored_line(child.raw)
        else {
            continue;
        };
        let child_line = RawLine {
            text: authored.body,
            start: authored.body_start,
            end: child.raw.end,
        };
        let child_tokens = tokenize_line_with_spans(&child_line);
        let (_, child_deps) = extract_line_dependencies(
            child_tokens,
            child_line.text,
            child_line.start,
            false,
            parse_clip,
        );
        found.absorb(&child_deps);
    }
    found
}

/// Complete modifiers as execution [`ParsedDependency`] values, in typed
/// order. The writer resolves each `note`/`block_id` against staged
/// vault contents.
pub(crate) fn execution_dependencies(
    found: &ItemDependencySet,
) -> Vec<ParsedDependency> {
    found
        .entries
        .iter()
        .map(|entry| ParsedDependency {
            raw: entry.raw.clone(),
            note: entry.note.clone(),
            block_id: entry.block_id.clone(),
            quoted: entry.quoted,
        })
        .collect()
}

pub(crate) fn execution_blocked_tasks(
    found: &ItemDependencySet,
) -> Vec<ParsedDependency> {
    found
        .blocked_entries
        .iter()
        .map(|entry| ParsedDependency {
            raw: entry.raw.clone(),
            note: entry.note.clone(),
            block_id: entry.block_id.clone(),
            quoted: entry.quoted,
        })
        .collect()
}

pub(crate) fn execution_relations(
    found: &ItemDependencySet,
) -> Vec<ParsedRelation> {
    found
        .relation_entries
        .iter()
        .map(|entry| ParsedRelation {
            raw: entry.raw.clone(),
            direction: entry.direction,
            note: entry.note.clone(),
            block_id: entry.block_id.clone(),
            quoted: entry.quoted,
            hidden: entry.hidden,
            label: entry.label.clone(),
        })
        .collect()
}

/// Map stripped tokens through relationship unescaping without moving their
/// borrows: a token starting with a consumed escape drops its backslash
/// and keeps the visible modifier in task text. `escapes` are the absolute
/// backslash ranges from the same line's extraction.
pub(crate) fn unescape_execution_tokens<'a>(
    tokens: Vec<Token<'a>>,
    escapes: &[(usize, usize)],
) -> Vec<Token<'a>> {
    tokens
        .into_iter()
        .map(|token| {
            let text = token.text;
            if (text.starts_with("\\&")
                || text.starts_with("\\*")
                || text.starts_with("\\~"))
                && escapes.contains(&(token.start, token.start + 1))
                && let Some(unescaped) = text.strip_prefix('\\')
            {
                return Token {
                    text: unescaped,
                    start: token.start + 1,
                    end: token.end,
                };
            }
            token
        })
        .collect()
}

/// Error when at least one modifier is still being typed: an unresolved
/// query cannot silently fall back to prose on submit.
pub(crate) fn incomplete_dependency_error() -> String {
    "prerequisite selection is incomplete: finish typing the \
     `&note:block-id` modifier or remove it (the `&` picker needs a note \
     and a block ID)"
        .to_string()
}

pub(crate) fn incomplete_blocked_task_error() -> String {
    "blocked-task selection is incomplete: finish typing the \
     `*note:block-id` modifier or remove it (the `*` picker needs a note \
     and a block ID; escape a literal star such as `\\*nix`)"
        .to_string()
}

pub(crate) fn operator_relation_error(sigil: u8) -> String {
    format!(
        "task relationships need a task owner: '{sigil}note:block-id' cannot \
         attach to a Pomodoro operator capture (add task text or '@note+task-id' \
         for the dependent)",
        sigil = sigil as char,
    )
}

pub(crate) fn unsupported_target_relation_error(
    mode_word: &str,
    sigil: u8,
) -> String {
    format!(
        "task relationships need a task owner: '{sigil}note:block-id' cannot \
         attach to a {mode_word} capture (add task text or '@note+task-id' \
         for the dependent)",
        sigil = sigil as char,
    )
}

pub(crate) fn invalid_relation_target_error(
    marker_text: &str,
    sigil: u8,
) -> String {
    format!(
        "relationship-only '{marker_text}' must be bare: it mixes a Pomodoro \
         action with '{sigil}note:block-id' (capture them as separate blank-line \
         items)",
        sigil = sigil as char,
    )
}

/// Shared parse/execute refusal when a bare existing owner carries `$<N>`,
/// `s:`, `%…`, or child bullets. Those extras need task text.
pub(crate) fn relationship_needs_text_error(
    offending: &str,
    owner: &str,
    rel_raws: &[String],
) -> String {
    let rel = rel_raws.join(" ");
    let example = if offending == "child bullets" {
        format!("Waiting {owner} {rel}` then `- detail")
    } else if rel.is_empty() {
        format!("Note {owner} {offending}")
    } else {
        format!("Note {owner} {rel} {offending}")
    };
    format!(
        "`{offending}` needs task text: `{owner}` with no text only adds task \
         relationships; add text to write a sub-bullet (`{example}`)"
    )
}

/// Typed `&`/`*`/`~` modifiers in source order, for teaching examples.
pub(crate) fn relation_modifier_raws(found: &ItemDependencySet) -> Vec<String> {
    let mut entries: Vec<(usize, String)> = found
        .entries
        .iter()
        .map(|entry| (entry.start, entry.raw.clone()))
        .chain(
            found
                .blocked_entries
                .iter()
                .map(|entry| (entry.start, entry.raw.clone())),
        )
        .chain(
            found
                .relation_entries
                .iter()
                .map(|entry| (entry.start, entry.raw.clone())),
        )
        .collect();
    entries.sort_by_key(|(start, _)| *start);
    entries.into_iter().map(|(_, raw)| raw).collect()
}

pub(crate) fn incomplete_relation_target_error() -> String {
    "`~` needs a task; choose one, or write `\\~` for a literal tilde"
        .to_string()
}

pub(crate) fn incomplete_relation_label_error() -> String {
    "relation label is incomplete: finish typing the `|label` or remove it \
     (labels are 1-40 characters; quote multi-word labels: `|\"follow up\"`)"
        .to_string()
}

/// First `$<N>`, retired `p:<N>`, `s:<N>`, or clipboard token on the parent
/// line.
pub(crate) fn first_relationship_text_token<'a>(
    tokens: &[Token<'a>],
    parse_clip: bool,
) -> Option<&'a str> {
    tokens.iter().find_map(|token| {
        if is_priority_marker_token(token.text)
            || parse_schedule_token(token.text).is_some()
        {
            return Some(token.text);
        }
        if parse_clip && parse_clip_token(token.text).is_some() {
            return Some(token.text);
        }
        None
    })
}

/// `#name`, `=…`, or `!` on an existing-owner marker: a Pomodoro action,
/// refused with [`invalid_relation_target_error`] rather than
/// [`relationship_needs_text_error`].
pub(crate) fn owner_marker_is_suffixed(marker: &str) -> bool {
    let rest = marker
        .strip_prefix('@')
        .or_else(|| marker.strip_prefix('^'))
        .unwrap_or(marker);
    rest.contains('#') || rest.contains('=') || rest.ends_with('!')
}

/// Build the shared relationship-only extra-text refusal for a bare owner.
pub(crate) fn refuse_relationship_needs_text(
    found: &ItemDependencySet,
    owner: &str,
    parent_tokens: &[Token<'_>],
    parse_clip: bool,
    has_children: bool,
) -> String {
    let offending = first_relationship_text_token(parent_tokens, parse_clip)
        .unwrap_or(if has_children { "child bullets" } else { "$2" });
    relationship_needs_text_error(
        offending,
        owner,
        &relation_modifier_raws(found),
    )
}

/// Closing a Pomodoro cannot also write a `*` edge: the two actions
/// belong on separate blank-line items.
pub(crate) fn close_blocked_task_error(marker_text: &str) -> String {
    format!(
        "cannot mix a Pomodoro close with '*note:block-id': '{marker_text}' \
         closes the session, so capture the block as a separate blank-line item \
         (close first, then N *T)"
    )
}

/// Error when modifiers name prerequisites but no dependent: add task
/// text or an explicit existing parent.
pub(crate) fn ownerless_relation_error(sigil: u8) -> String {
    format!(
        "task relationships need a dependent: add task text or '@note+task-id' \
         for the task that should use the '{sigil}note:block-id' modifier(s)",
        sigil = sigil as char,
    )
}

/// Decide which task owns an execution item's modifiers once its finished
/// kind is known, mirroring the editor ownership contract.
///
/// Returns `Ok(None)` when the item names no local owner: draft
/// resolution retries after `@@` inheritance and reports the ownerless
/// diagnostic when no inherited parent applies either.
#[allow(clippy::too_many_arguments)]
pub(crate) fn resolve_execution_ownership(
    kind: &CaptureKind,
    route: Option<&str>,
    body_empty: bool,
    has_creation_modifiers: bool,
    local_marker_text: Option<&str>,
    sigil: u8,
) -> Result<Option<ParsedDependencyTarget>, String> {
    match kind {
        CaptureKind::TaskToggle {
            block_id,
            pomodoro_name,
            intent,
            ..
        } => {
            let marker = local_marker_text.unwrap_or("@route+id");
            if pomodoro_name.is_some()
                || matches!(intent, TaskToggleIntent::Toggle)
                || has_creation_modifiers
            {
                return Err(invalid_relation_target_error(marker, sigil));
            }
            Ok(Some(ParsedDependencyTarget {
                kind: ParsedDependencyTargetKind::ExistingTask,
                route: route.map(str::to_string),
                block_id: Some(block_id.clone()),
                inherited: false,
            }))
        }
        CaptureKind::PomodoroLink {
            block_id,
            pomodoro_name,
            start,
            close,
            spelling,
            ..
        } => {
            // Bare `@route:id` and `^route:id` aliases select the same
            // existing dependent as `@route+id`; any other link shape
            // (`#name`, `=` suffix, creation modifiers) cannot carry
            // dependencies.
            let bare = matches!(
                spelling,
                super::model::PomodoroLinkSpelling::At
                    | super::model::PomodoroLinkSpelling::Caret
            ) && pomodoro_name.is_none()
                && start.is_none()
                && close.is_none()
                && !has_creation_modifiers;
            if !bare {
                return Err(invalid_relation_target_error(
                    local_marker_text.unwrap_or("@route:id"),
                    sigil,
                ));
            }
            Ok(Some(ParsedDependencyTarget {
                kind: ParsedDependencyTargetKind::ExistingTask,
                route: route.map(str::to_string),
                block_id: Some(block_id.clone()),
                inherited: false,
            }))
        }
        CaptureKind::SubBullet { target, .. } => match target {
            SubBulletTarget::BlockId(block_id) => {
                Ok(Some(ParsedDependencyTarget {
                    kind: ParsedDependencyTargetKind::ExistingTask,
                    route: route.map(str::to_string),
                    block_id: Some(block_id.clone()),
                    inherited: false,
                }))
            }
            SubBulletTarget::Ref { .. } => {
                Err(format!(
                    "task relationships cannot attach to a `--task-ref` picker \
                     selection: use '@note+task-id' for the dependent ('{sigil}note:block-id')",
                    sigil = sigil as char,
                ))
            }
        },
        CaptureKind::Task | CaptureKind::TaskWithBlockId { .. } => {
            if body_empty {
                return Ok(None);
            }
            let block_id = match kind {
                CaptureKind::TaskWithBlockId { block_id } => {
                    Some(block_id.clone())
                }
                _ => None,
            };
            Ok(Some(ParsedDependencyTarget {
                kind: ParsedDependencyTargetKind::NewTask,
                route: route.map(str::to_string),
                block_id,
                inherited: false,
            }))
        }
        CaptureKind::Pomodoro {
            block_id, close, ..
        } => {
            if close.is_some() && sigil == b'*' {
                return Err(close_blocked_task_error(
                    local_marker_text.unwrap_or("@route:id=x"),
                ));
            }
            if body_empty {
                return Ok(None);
            }
            Ok(Some(ParsedDependencyTarget {
                kind: ParsedDependencyTargetKind::NewTask,
                route: route.map(str::to_string),
                block_id: Some(block_id.clone()),
                inherited: false,
            }))
        }
        CaptureKind::Bullet { .. } => {
            Err(unsupported_target_relation_error("section-bullet", sigil))
        }
        CaptureKind::ProjectNote { .. } => {
            Err(unsupported_target_relation_error("project-note", sigil))
        }
        CaptureKind::PomodoroNote => {
            Err(unsupported_target_relation_error("Pomodoro-note", sigil))
        }
        CaptureKind::PomodoroAdjust { .. }
        | CaptureKind::PomodoroShift { .. } => Err(
            unsupported_target_relation_error("Pomodoro session-operator", sigil),
        ),
        CaptureKind::PomodoroClose { .. } => {
            Err(unsupported_target_relation_error("Pomodoro-close", sigil))
        }
        CaptureKind::PomodoroStart { .. } => {
            Err(unsupported_target_relation_error("Pomodoro-start", sigil))
        }
        CaptureKind::TaskComplete { .. } => {
            // A `!` item never carries `&` modifiers: any second token
            // is claimed invalid before ownership resolves.
            Err(unsupported_target_relation_error("task-complete", sigil))
        }
        CaptureKind::Ref(_) => {
            Err(unsupported_target_relation_error("reference", sigil))
        }
    }
}

/// Finish dependency ownership after `@@` inheritance (draft resolution
/// calls this right after `inherit_global_destination`): an ownerless
/// item with an inherited `@@route+id` parent applies its modifiers to
/// that parent, an ownerless item with an inherited route still needs
/// task text, and anything still ownerless reports the diagnostic.
pub(crate) fn finish_inherited_dependency_target(
    route: Option<&str>,
    kind: &CaptureKind,
    body_empty: bool,
    has_target: bool,
    has_dependencies: bool,
    inherited: bool,
    sigil: u8,
) -> Result<Option<ParsedDependencyTarget>, String> {
    if has_target || !has_dependencies || !inherited {
        return Ok(None);
    }
    match kind {
        CaptureKind::SubBullet {
            target: SubBulletTarget::BlockId(block_id),
            ..
        } => Ok(Some(ParsedDependencyTarget {
            kind: ParsedDependencyTargetKind::ExistingTask,
            route: route.map(str::to_string),
            block_id: Some(block_id.clone()),
            inherited: true,
        })),
        CaptureKind::Task if !body_empty => Ok(Some(ParsedDependencyTarget {
            kind: ParsedDependencyTargetKind::NewTask,
            route: route.map(str::to_string),
            block_id: None,
            inherited: true,
        })),
        _ => Err(ownerless_relation_error(sigil)),
    }
}

#[cfg(test)]
mod tests {
    use super::super::model::tokenize_with_spans;
    use super::*;

    fn extract(text: &str, parent: bool) -> (Vec<String>, LineDependencies) {
        let tokens = tokenize_with_spans(text);
        let (remaining, found) =
            extract_line_dependencies(tokens, text, 0, parent, false);
        (
            remaining
                .iter()
                .map(|token| token.text.to_string())
                .collect(),
            found,
        )
    }

    #[test]
    fn trailing_complete_modifier_strips_and_spans() {
        let (remaining, found) = extract("Buy milk &foo:bar", true);
        assert_eq!(remaining, vec!["Buy", "milk"]);
        assert_eq!(found.entries.len(), 1);
        let entry = &found.entries[0];
        assert_eq!(entry.raw, "&foo:bar");
        assert_eq!(entry.note, "foo");
        assert_eq!(entry.block_id, "bar");
        assert!(!entry.quoted);
        assert_eq!((entry.start, entry.end), (9, 17));
    }

    #[test]
    fn mid_line_modifiers_stay_literal() {
        for text in [
            "Buy &foo:bar groceries",
            "R&D research",
            "Research & Development",
        ] {
            let (remaining, found) = extract(text, true);
            assert!(found.entries.is_empty(), "{text}");
            assert!(found.partials.is_empty(), "{text}");
            assert_eq!(remaining.join(" "), text);
        }
    }

    #[test]
    fn quoted_note_with_spaces_stays_atomic() {
        let (remaining, found) =
            extract("Buy milk &\"Shopping List\":bar", true);
        assert_eq!(remaining, vec!["Buy", "milk"]);
        assert_eq!(found.entries.len(), 1);
        let entry = &found.entries[0];
        assert_eq!(entry.note, "Shopping List");
        assert!(entry.quoted);
        assert_eq!(&entry.raw, "&\"Shopping List\":bar");
    }

    #[test]
    fn quoted_escapes_decode() {
        let (remaining, found) = extract("Buy milk &\"a\\\"b\\\\c\":id", true);
        assert_eq!(remaining, vec!["Buy", "milk"]);
        assert_eq!(found.entries.len(), 1);
        assert_eq!(found.entries[0].note, "a\"b\\c");
    }

    #[test]
    fn unterminated_quote_is_one_partial() {
        let (_, found) = extract("Buy milk &\"Shopping", true);
        assert!(found.entries.is_empty());
        assert_eq!(found.partials, vec![(9, 19)]);
    }

    #[test]
    fn bare_sigil_is_directive_only_at_the_ends() {
        let (_, leading) = extract("& Buy milk", true);
        assert_eq!(leading.partials, vec![(0, 1)]);
        let (_, trailing) = extract("Buy milk &", true);
        assert_eq!(trailing.partials, vec![(9, 10)]);
        let (_, middle) = extract("Buy & milk", true);
        assert!(middle.partials.is_empty());
    }

    #[test]
    fn traversal_and_bad_ids_are_invalid() {
        for text in ["&../x:id", "&/abs:id", "&foo:bar!", "&:id"] {
            let scanned = scan_line_dependencies(text, 0);
            assert!(
                matches!(scanned[..], [ScannedDependency::Invalid(_)]),
                "{text}"
            );
        }
    }

    #[test]
    fn protected_spans_are_never_modifiers() {
        for text in ["See [[a &x:y]] ok", "Run `a &x:y` ok"] {
            assert!(scan_line_dependencies(text, 0).is_empty(), "{text}");
        }
    }

    #[test]
    fn escape_consumes_only_its_backslash() {
        let scanned = scan_line_dependencies("Buy \\&foo:bar", 0);
        assert!(
            matches!(
                scanned[..],
                [ScannedDependency::Escaped {
                    backslash: 4,
                    start: 5,
                    ..
                }]
            ),
            "{scanned:?}"
        );
        let (remaining, found) = extract("Buy \\&foo:bar", true);
        assert_eq!(remaining, vec!["Buy", "\\&foo:bar"]);
        assert!(found.entries.is_empty());
        assert_eq!(
            unescape_dependency_text("\\&foo:bar", 4, &found.escapes),
            "&foo:bar"
        );
    }

    #[test]
    fn unicode_offsets_stay_on_char_boundaries() {
        // `é` is two bytes: the modifier starts at byte 4, not char 3.
        let (remaining, found) = extract("café &foo:bar", true);
        assert_eq!(remaining, vec!["café"]);
        assert_eq!(found.entries.len(), 1);
        assert_eq!((found.entries[0].start, found.entries[0].end), (6, 14));
        assert_eq!(&"café &foo:bar"[6..14], "&foo:bar");
    }

    #[test]
    fn blocked_task_modifiers_have_separate_entries_and_spans() {
        let (remaining, found) =
            extract("Call registrar &school:transcript *job:apply-x", true);
        assert_eq!(remaining, vec!["Call", "registrar"]);
        assert_eq!(found.entries.len(), 1);
        assert_eq!(found.entries[0].raw, "&school:transcript");
        assert_eq!(found.blocked_entries.len(), 1);
        assert_eq!(found.blocked_entries[0].raw, "*job:apply-x");
        assert!(found.spans.iter().any(|span| {
            span.kind == SpanKind::BlockedTaskSigil
                && span.start == "Call registrar &school:transcript ".len()
        }));
    }

    #[test]
    fn star_markdown_and_mid_line_tokens_stay_literal() {
        for text in [
            "Fix *urgent* bug",
            "Read **Dune**",
            "* Buy milk",
            "5 * 3",
            "R*D",
            "Fix *foo:bar now",
            "Fix *foo:bar*",
        ] {
            let (remaining, found) = extract(text, true);
            assert!(found.blocked_entries.is_empty(), "{text}");
            assert!(found.blocked_partials.is_empty(), "{text}");
            assert_eq!(remaining.join(" "), text, "{text}");
        }
    }

    #[test]
    fn escaped_star_unescapes_only_inside_a_modifier_run() {
        let (remaining, found) = extract("Support \\*nix", true);
        assert_eq!(remaining, vec!["Support", "\\*nix"]);
        assert_eq!(
            unescape_dependency_text("\\*nix", 8, &found.escapes),
            "*nix"
        );

        let (remaining, found) = extract("Support \\*nix today", true);
        assert_eq!(remaining, vec!["Support", "\\*nix", "today"]);
        assert_eq!(
            unescape_dependency_text("\\*nix", 8, &found.escapes),
            "\\*nix"
        );
    }
}
