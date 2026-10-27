//! `~` / `~~` relation modifiers: lexical scanning and line extraction.
//!
//! This module mirrors `dependencies.rs` for the `&` / `*` family. A `~`
//! modifier links this item's task to another task, and `~~` links another
//! task to this item's task. Both carry an optional `|label` validated
//! with `task_relations::label::normalize_relation_label`.

use super::editor_model::RelationDirection;

fn protected_ranges(line: &str) -> Vec<(usize, usize)> {
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

fn is_token_start(line: &str, amp: usize) -> bool {
    amp == 0
        || line[..amp]
            .chars()
            .next_back()
            .is_some_and(|previous| previous.is_whitespace())
}

fn slash_run_before(line: &str, amp: usize) -> Option<(usize, usize)> {
    let mut index = amp;
    while index > 0 && line.as_bytes()[index - 1] == b'\\' {
        index -= 1;
    }
    (index < amp).then_some((index, amp - index))
}

fn escape_at(line: &str, amp: usize) -> Option<usize> {
    let (start, len) = slash_run_before(line, amp)?;
    (len % 2 == 1 && is_token_start(line, start)).then_some(start)
}

fn char_len(line: &str, index: usize) -> usize {
    line[index..]
        .chars()
        .next()
        .map(|character| character.len_utf8())
        .unwrap_or(1)
}

fn is_space_at(line: &str, index: usize) -> bool {
    line[index..]
        .chars()
        .next()
        .is_some_and(|character| character.is_whitespace())
}

fn decode_quoted_note(raw: &str) -> String {
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

fn validate_relation_block_id(block_id: &str) -> Option<String> {
    if !super::tokens::is_block_id(block_id) {
        return Some(format!(
            "relation block ID '{block_id}' must contain only A-Z, a-z, \
             0-9 or '-' (use '~note:block-id')"
        ));
    }
    None
}

/// One complete `~note:block-id[|label]` modifier. A `||label`
/// spelling sets `hidden`: the stored Markdown is identical to the
/// `|` form, but the label hides from suggestions once the capture
/// succeeds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RelationRef {
    /// Exact typed text, sigil included (`~work:foo|"follow up"`).
    pub(crate) raw: String,
    pub(crate) direction: RelationDirection,
    /// Vault-relative note identity exactly as typed.
    pub(crate) note: String,
    pub(crate) block_id: String,
    pub(crate) quoted: bool,
    /// True when the label separator was `||`.
    pub(crate) hidden: bool,
    /// Normalised label, when `|label` was typed.
    pub(crate) label: Option<String>,
    /// Whether the label was `"quoted"`.
    pub(crate) label_quoted: bool,
    pub(crate) start: usize,
    pub(crate) end: usize,
    pub(crate) note_start: usize,
    pub(crate) note_end: usize,
    pub(crate) block_start: usize,
    pub(crate) block_end: usize,
    pub(crate) separator: Option<(usize, usize)>,
    pub(crate) label_start: Option<usize>,
    pub(crate) label_end: Option<usize>,
}

impl RelationRef {
    pub(crate) fn sigil_len(&self) -> usize {
        match self.direction {
            RelationDirection::To => 1,
            RelationDirection::From => 2,
        }
    }
}

/// One malformed relation modifier with its teaching message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct InvalidRelation {
    pub(crate) raw: String,
    pub(crate) message: String,
    pub(crate) start: usize,
    pub(crate) end: usize,
}

/// One `~` occurrence classified by the line scanner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ScannedRelation {
    Complete(RelationRef),
    /// A partial target query: bare `~`/`~~`, `~query`, or `~note:`.
    PartialTarget {
        raw: String,
        start: usize,
        end: usize,
    },
    /// A complete target with a partial label: `~note:id|` or an open quote.
    /// `hidden` is true when the separator was `||` (including the
    /// empty `~note:id||` probe).
    PartialLabel {
        raw: String,
        start: usize,
        end: usize,
        hidden: bool,
    },
    Invalid(InvalidRelation),
    Escaped {
        backslash: usize,
        start: usize,
        end: usize,
    },
}

impl ScannedRelation {
    pub(crate) fn start(&self) -> usize {
        match self {
            Self::Complete(entry) => entry.start,
            Self::PartialTarget { start, .. } => *start,
            Self::PartialLabel { start, .. } => *start,
            Self::Invalid(invalid) => invalid.start,
            Self::Escaped { start, .. } => *start,
        }
    }

    pub(crate) fn end(&self) -> usize {
        match self {
            Self::Complete(entry) => entry.end,
            Self::PartialTarget { end, .. } => *end,
            Self::PartialLabel { end, .. } => *end,
            Self::Invalid(invalid) => invalid.end,
            Self::Escaped { end, .. } => *end,
        }
    }
}

fn token_end_at(line: &str, index: usize) -> usize {
    let mut end = index;
    while end < line.len() && !is_space_at(line, end) {
        end += char_len(line, end);
    }
    end
}

fn is_strike_token(token: &str) -> bool {
    token.len() > 4 && token.starts_with("~~") && token.ends_with("~~")
}

/// Scan one physical line for `~` / `~~` modifiers.
pub(crate) fn scan_line_relations(
    line: &str,
    base: usize,
) -> Vec<ScannedRelation> {
    let protected = protected_ranges(line);
    let mut found = Vec::new();
    let mut index = 0;
    while index < line.len() {
        if line.as_bytes()[index] != b'~' {
            index += char_len(line, index);
            continue;
        }
        // The second `~` of a `~~` pair is never a fresh start.
        if index > 0
            && line.as_bytes()[index - 1] == b'~'
            && line[..index].ends_with('~')
            && is_relation_start(line, index - 1)
        {
            index += 1;
            continue;
        }
        if let Some(backslash) = escape_at(line, index) {
            let mut end = index + 1;
            while end < line.len() && !is_space_at(line, end) {
                end += char_len(line, end);
            }
            found.push(ScannedRelation::Escaped {
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
        match scan_relation_at(line, base, index) {
            Some(ScannedRelation::Escaped { .. }) => unreachable!(),
            Some(scan) => {
                let end = scan.end() - base;
                found.push(scan);
                index = end.max(index + 1);
            }
            None => {
                // Literal guard (`~/path`, digit, strike): skip the token.
                let end = token_end_at(line, index);
                index = end.max(index + 1);
            }
        }
    }
    found
}

fn is_relation_start(line: &str, index: usize) -> bool {
    is_token_start(line, index)
}

fn scan_relation_at(
    line: &str,
    base: usize,
    tilde: usize,
) -> Option<ScannedRelation> {
    let start = base + tilde;
    let sigil_len = if line.as_bytes().get(tilde + 1) == Some(&b'~') {
        2
    } else {
        1
    };
    let direction = if sigil_len == 2 {
        RelationDirection::From
    } else {
        RelationDirection::To
    };
    // `~~~` never scans: it is an invalid sigil, not a relation.
    if sigil_len == 2 && line.as_bytes().get(tilde + 2) == Some(&b'~') {
        let end = token_end_at(line, tilde);
        return Some(ScannedRelation::Invalid(InvalidRelation {
            raw: line[tilde..end].to_string(),
            message: "relation sigil must be `~` or `~~` without extra `~` \
                (write `~note:block-id` or `~~note:block-id`)"
                .to_string(),
            start,
            end: base + end,
        }));
    }
    // Strike guard: a whitespace-delimited `~~text~~` token is body.
    {
        let end = token_end_at(line, tilde);
        let token = &line[tilde..end];
        if is_strike_token(token) {
            return None;
        }
    }
    let mut cursor = tilde + sigil_len;
    // Next-byte `/` guard: `~/path` stays literal.
    if line.as_bytes().get(cursor) == Some(&b'/') {
        return None;
    }
    // Bare sigil: `~` / `~~` with whitespace or end after it.
    if cursor >= line.len() || is_space_at(line, cursor) {
        return Some(ScannedRelation::PartialTarget {
            raw: line[tilde..cursor].to_string(),
            start,
            end: base + cursor,
        });
    }
    // Digit guard when the token holds no `:`: `~5m` stays literal.
    {
        let end = token_end_at(line, tilde);
        let token = &line[tilde..end];
        if !token.contains(':') {
            let first = line[cursor..].chars().next().unwrap_or(' ');
            if first.is_ascii_digit() {
                return None;
            }
        }
    }
    if line.as_bytes()[cursor] == b'"' {
        return Some(scan_relation_quoted_note(
            line, base, tilde, start, sigil_len, direction,
        ));
    }
    // Unquoted note: runs to `:` or whitespace/end.
    let note_start = cursor;
    while cursor < line.len()
        && line.as_bytes()[cursor] != b':'
        && !is_space_at(line, cursor)
    {
        cursor += char_len(line, cursor);
    }
    if cursor >= line.len() || is_space_at(line, cursor) {
        // `~query` with no `:` yet: still typing the target.
        // Digit-only queries without a colon already returned above.
        return Some(ScannedRelation::PartialTarget {
            raw: line[tilde..cursor].to_string(),
            start,
            end: base + cursor,
        });
    }
    let note = line[note_start..cursor].to_string();
    Some(scan_relation_block_id(
        line, base, tilde, start, sigil_len, direction, note, false,
        note_start, cursor,
    ))
}

fn scan_relation_quoted_note(
    line: &str,
    base: usize,
    tilde: usize,
    start: usize,
    sigil_len: usize,
    direction: RelationDirection,
) -> ScannedRelation {
    let mut cursor = tilde + sigil_len + 1;
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
        return ScannedRelation::PartialTarget {
            raw: line[tilde..].to_string(),
            start,
            end: base + line.len(),
        };
    };
    let raw_note = line[tilde + sigil_len + 1..close].to_string();
    let after = close + 1;
    if after >= line.len() || line.as_bytes()[after] != b':' {
        return ScannedRelation::PartialTarget {
            raw: line[tilde..after].to_string(),
            start,
            end: base + after,
        };
    }
    let note = decode_quoted_note(&raw_note);
    scan_relation_block_id(
        line,
        base,
        tilde,
        start,
        sigil_len,
        direction,
        note,
        true,
        tilde + sigil_len + 1,
        after,
    )
}

#[allow(clippy::too_many_arguments)]
fn scan_relation_block_id(
    line: &str,
    base: usize,
    tilde: usize,
    start: usize,
    sigil_len: usize,
    direction: RelationDirection,
    note: String,
    quoted: bool,
    note_start: usize,
    colon: usize,
) -> ScannedRelation {
    // Block ID runs to `|` or whitespace/end.
    let mut cursor = colon + 1;
    while cursor < line.len()
        && line.as_bytes()[cursor] != b'|'
        && !is_space_at(line, cursor)
    {
        cursor += char_len(line, cursor);
    }
    let raw_end = cursor;
    if cursor == colon + 1 {
        // `~note:` with no block ID yet: still typing.
        // Include the colon so the picker keeps the note.
        return ScannedRelation::PartialTarget {
            raw: line[tilde..cursor].to_string(),
            start,
            end: base + cursor,
        };
    }
    // Include a trailing `|` with no label as a label partial below;
    // otherwise the raw ends at the block ID.
    let block_id = line[colon + 1..cursor].to_string();
    if let Some(message) = validate_relation_note(&note) {
        return ScannedRelation::Invalid(InvalidRelation {
            raw: line[tilde..raw_end].to_string(),
            message,
            start,
            end: base + raw_end,
        });
    }
    if let Some(message) = validate_relation_block_id(&block_id) {
        return ScannedRelation::Invalid(InvalidRelation {
            raw: line[tilde..raw_end].to_string(),
            message,
            start,
            end: base + raw_end,
        });
    }
    // No `|` yet: complete unlabelled, or end/space after the block ID.
    if cursor >= line.len() || is_space_at(line, cursor) {
        return complete_relation(
            line, base, tilde, start, sigil_len, direction, note, quoted,
            false, note_start, colon, block_id, cursor, None, None, None,
            raw_end,
        );
    }
    // `|` separator present. A second `|` hides the label from
    // suggestions once the capture succeeds; the stored Markdown is
    // identical either way.
    debug_assert_eq!(line.as_bytes()[cursor], b'|');
    let hidden = line.as_bytes().get(cursor + 1) == Some(&b'|');
    if hidden && line.as_bytes().get(cursor + 2) == Some(&b'|') {
        // `~note:id|||label`: three pipes never scan as a relation.
        let end = token_end_at(line, tilde);
        return ScannedRelation::Invalid(InvalidRelation {
            raw: line[tilde..end].to_string(),
            message: "use || to hide a label, | to name one".to_string(),
            start,
            end: base + end,
        });
    }
    let separator = (base + cursor, base + cursor + if hidden { 2 } else { 1 });
    let label_begin = cursor + if hidden { 2 } else { 1 };
    if label_begin >= line.len() || is_space_at(line, label_begin) {
        return ScannedRelation::PartialLabel {
            raw: line[tilde..label_begin].to_string(),
            start,
            end: base + label_begin,
            hidden,
        };
    }
    if line.as_bytes()[label_begin] == b'"' {
        return scan_relation_quoted_label(
            line, base, tilde, start, sigil_len, direction, note, quoted,
            note_start, colon, block_id, cursor, separator, hidden,
        );
    }
    let mut label_end = label_begin;
    while label_end < line.len() && !is_space_at(line, label_end) {
        label_end += char_len(line, label_end);
    }
    let label_raw = line[label_begin..label_end].to_string();
    if crate::native::task_relations::label::normalize_relation_label(
        &label_raw,
    )
    .is_err()
    {
        // Find the token end for the invalid range (to whitespace).
        let end = token_end_at(line, tilde);
        return ScannedRelation::Invalid(InvalidRelation {
            raw: line[tilde..end].to_string(),
            message: invalid_label_message(&label_raw),
            start,
            end: base + end,
        });
    }
    let normalized =
        crate::native::task_relations::label::normalize_relation_label(
            &label_raw,
        )
        .expect("label validates");
    complete_relation(
        line,
        base,
        tilde,
        start,
        sigil_len,
        direction,
        note,
        quoted,
        hidden,
        note_start,
        colon,
        block_id,
        cursor,
        Some(normalized),
        Some(separator),
        Some((base + label_begin, base + label_end)),
        label_end,
    )
}

#[allow(clippy::too_many_arguments)]
fn scan_relation_quoted_label(
    line: &str,
    base: usize,
    tilde: usize,
    start: usize,
    sigil_len: usize,
    direction: RelationDirection,
    note: String,
    quoted: bool,
    note_start: usize,
    colon: usize,
    block_id: String,
    pipe: usize,
    separator: (usize, usize),
    hidden: bool,
) -> ScannedRelation {
    let label_begin = separator.1 - base;
    debug_assert_eq!(line.as_bytes()[label_begin], b'"');
    let mut cursor = label_begin + 1;
    let mut closed = None;
    while cursor < line.len() {
        if line.as_bytes()[cursor] == b'"' {
            closed = Some(cursor);
            break;
        }
        // No escapes inside relation labels (Rule 2).
        cursor += char_len(line, cursor);
    }
    let Some(close) = closed else {
        return ScannedRelation::PartialLabel {
            raw: line[tilde..].to_string(),
            start,
            end: base + line.len(),
            hidden,
        };
    };
    let label_raw = line[label_begin + 1..close].to_string();
    if crate::native::task_relations::label::normalize_relation_label(
        &label_raw,
    )
    .is_err()
    {
        let end = close + 1;
        return ScannedRelation::Invalid(InvalidRelation {
            raw: line[tilde..end].to_string(),
            message: invalid_label_message(&label_raw),
            start,
            end: base + end,
        });
    }
    let normalized =
        crate::native::task_relations::label::normalize_relation_label(
            &label_raw,
        )
        .expect("label validates");
    // After the closing quote only whitespace/end is allowed.
    let after = close + 1;
    if after < line.len() && !is_space_at(line, after) {
        let end = token_end_at(line, tilde);
        return ScannedRelation::Invalid(InvalidRelation {
            raw: line[tilde..end].to_string(),
            message: format!(
                "relation label {raw} must end after the closing quote: \
                 write `~note:block-id|\"label\"` with nothing glued after it",
                raw = &line[tilde..end],
            ),
            start,
            end: base + end,
        });
    }
    complete_relation(
        line,
        base,
        tilde,
        start,
        sigil_len,
        direction,
        note,
        quoted,
        hidden,
        note_start,
        colon,
        block_id,
        pipe,
        Some(normalized),
        Some(separator),
        Some((base + label_begin + 1, base + close)),
        after,
    )
}

#[allow(clippy::too_many_arguments)]
fn complete_relation(
    line: &str,
    base: usize,
    tilde: usize,
    start: usize,
    _sigil_len: usize,
    direction: RelationDirection,
    note: String,
    quoted: bool,
    hidden: bool,
    note_start: usize,
    colon: usize,
    block_id: String,
    _pipe: usize,
    label: Option<String>,
    separator: Option<(usize, usize)>,
    label_span: Option<(usize, usize)>,
    raw_end: usize,
) -> ScannedRelation {
    let (note_start_abs, note_end_abs) = if quoted {
        (base + note_start, base + colon - 1)
    } else {
        (base + note_start, base + colon)
    };
    let label_quoted = line[tilde..raw_end].contains('|')
        && line[tilde..raw_end].contains('"');
    ScannedRelation::Complete(RelationRef {
        raw: line[tilde..raw_end].to_string(),
        direction,
        note,
        block_id: block_id.clone(),
        quoted,
        hidden,
        label: label.clone(),
        label_quoted,
        start,
        end: base + raw_end,
        note_start: note_start_abs,
        note_end: note_end_abs,
        block_start: base + colon + 1,
        block_end: if separator.is_some() {
            // Block ID ends where `|` starts.
            separator.expect("separator").0
        } else {
            base + raw_end
        },
        separator,
        label_start: label_span.map(|span| span.0),
        label_end: label_span.map(|span| span.1),
    })
}

fn validate_relation_note(note: &str) -> Option<String> {
    if note.is_empty() {
        return Some(
            "relation note is empty: use '~note:block-id' \
             (for spaces, quote the note: '~\"My Note\":block-id')"
                .to_string(),
        );
    }
    if note.starts_with('/') {
        return Some(format!(
            "relation note '{note}' must be vault-relative, not absolute"
        ));
    }
    if note.contains('*') {
        return Some(format!(
            "relation note '{note}' contains '*', which is not allowed in the note component"
        ));
    }
    for component in note.split('/') {
        if component.is_empty() {
            return Some(format!(
                "relation note '{note}' has an empty path component"
            ));
        }
        if component == "." || component == ".." {
            return Some(format!(
                "relation note '{note}' must not contain '.' or '..'"
            ));
        }
    }
    if note.contains('~') {
        return Some(format!(
            "relation note '{note}' contains '~', which is not allowed in the note component"
        ));
    }
    if note.contains('|') {
        return Some(format!(
            "relation note '{note}' contains '|', which is not allowed in the note component"
        ));
    }
    None
}

fn invalid_label_message(label: &str) -> String {
    use crate::native::task_relations::label::normalize_relation_label;
    match normalize_relation_label(label) {
        Ok(_) => format!(
            "relation label '{label}' is invalid: labels may not contain \
             `[`, `]`, `(`, `)`, `\"`, `\\`, `|`, `•`, `*`, `~`, start with \
             `·` or `,`, or use controls (1-40 characters after trimming)"
        ),
        Err(error) => match error {
            crate::native::task_relations::label::LabelError::Empty => {
                "relation label is empty after trimming".to_string()
            }
            crate::native::task_relations::label::LabelError::TooLong => {
                format!(
                    "relation label '{label}' is too long: keep labels to 40 \
                     characters after trimming and collapsing spaces"
                )
            }
            crate::native::task_relations::label::LabelError::InvalidStart => {
                format!(
                    "relation label '{label}' must not start with `·` or `,`"
                )
            }
            crate::native::task_relations::label::LabelError::ForbiddenCharacter => {
                format!(
                    "relation label '{label}' contains a forbidden character: \
                     labels may not contain `[`, `]`, `(`, `)`, `\"`, `\\`, \
                     `|`, `•`, `*`, `~`, or controls"
                )
            }
        },
    }
}
