//! Exact-name Pomodoro literal: `#"NAME"` and `#“NAME”`.
//!
//! Every capture position that takes a Pomodoro name after `#` lexes this
//! one literal through [`lex_exact_name_after_hash`], so one rule holds
//! everywhere. The bare `=name` spelling never takes quotes.
//!
//! Quote precedent: `&"Shopping List":bar`. Straight `"…"` and curly
//! `“…”` quotes are both accepted; bob always emits straight quotes.
//! `\"` and `\\` are the only escapes. Valid names cannot contain either
//! character, but the decoder still honors the escapes. The token extent
//! runs through the closing quote; normal terminators apply after it. A
//! literal without its closing quote is an incomplete editing state, never
//! a mistake.

/// What [`lex_exact_name_after_hash`] found after the `#`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ExactNameLex {
    /// Decoded inner text: `\"` → `"`, `\\` → `\`, every other backslash
    /// stays literal.
    pub(crate) decoded: String,
    /// Byte length of the literal including both quotes. When incomplete,
    /// the length of the unterminated remainder (opening quote through the
    /// end of the scanned text).
    pub(crate) literal_len: usize,
    /// False when no closing quote was found: an incomplete editing state.
    pub(crate) complete: bool,
}

/// Lex an exact-name literal at the start of `after_hash` (the text just
/// after `#`). `None` when `after_hash` does not start with `"` or `“`.
/// `limit` bounds the scan: callers pass the enclosing token/line remainder
/// so an unterminated literal never runs past it.
pub(crate) fn lex_exact_name_after_hash(
    after_hash: &str,
    limit: usize,
) -> Option<ExactNameLex> {
    let mut chars = after_hash.char_indices();
    let (_, open) = chars.next()?;
    let close = match open {
        '"' => '"',
        '“' => '”',
        _ => return None,
    };
    let bounded_len = after_hash.len().min(limit);
    let text = &after_hash[..bounded_len];
    let mut decoded = String::new();
    let mut index = open.len_utf8();
    while index < text.len() {
        let rest = &text[index..];
        let character = rest.chars().next().expect("char boundary");
        if character == '\\'
            && let Some(next) = rest[character.len_utf8()..].chars().next()
            && matches!(next, '"' | '\\')
        {
            decoded.push(next);
            index += character.len_utf8() + next.len_utf8();
            continue;
        }
        if character == close || (open == '“' && character == '"') {
            let literal_len = index + character.len_utf8();
            return Some(ExactNameLex {
                decoded,
                literal_len,
                complete: true,
            });
        }
        decoded.push(character);
        index += character.len_utf8();
    }
    Some(ExactNameLex {
        decoded,
        literal_len: text.len(),
        complete: false,
    })
}

/// True when the text just after `#` opens an exact-name literal.
pub(crate) fn opens_exact_name(after_hash: &str) -> bool {
    after_hash.starts_with('"') || after_hash.starts_with('“')
}

/// Display name for an `x#…` close-hash teaching error. A literal
/// decodes from the full line (the whitespace-delimited token is
/// truncated); anything else returns the raw text after `#`.
pub(crate) fn decode_close_hash_name(first: &str, full_line: &str) -> String {
    let raw = first
        .find('#')
        .map(|hash| &first[hash + 1..])
        .unwrap_or_default();
    if !opens_exact_name(raw) {
        return raw.to_string();
    }
    let full_after = full_line
        .find('#')
        .map(|hash| &full_line[hash + 1..])
        .unwrap_or(raw);
    lex_exact_name_after_hash(full_after, full_after.len())
        .map(|lex| canonicalize_exact_name(&lex.decoded).unwrap_or(lex.decoded))
        .unwrap_or_else(|| raw.to_string())
}

/// Decode a (possibly cursor-truncated) literal inner text: `\"` → `"`,
/// `\\` → `\`; every other backslash stays literal. Mirrors the
/// [`lex_exact_name_after_hash`] decoder for prefixes.
pub(crate) fn decode_partial_literal_text(inner: &str) -> String {
    let mut decoded = String::with_capacity(inner.len());
    let mut chars = inner.chars();
    while let Some(character) = chars.next() {
        if character == '\\' {
            match chars.next() {
                Some(next @ ('"' | '\\')) => decoded.push(next),
                Some(next) => {
                    decoded.push('\\');
                    decoded.push(next);
                }
                None => decoded.push('\\'),
            }
            continue;
        }
        decoded.push(character);
    }
    decoded
}

/// Canonical name for a decoded exact-name literal: collapse whitespace,
/// uppercase, and validate as a Pomodoro name. `None` when the literal
/// names nothing valid.
pub(crate) fn canonicalize_exact_name(decoded: &str) -> Option<String> {
    let name = super::line::normalize_task_text(decoded).to_ascii_uppercase();
    crate::native::capture_task_sections::is_pomodoro_name(&name)
        .then_some(name)
}

/// Extend a whitespace-delimited link token through an exact-name
/// literal's closing quote. `parent` is the full trimmed parent text and
/// `first` its first whitespace-delimited token. Returns the marker text
/// to parse (which may span whitespace when the literal holds spaces) and
/// the byte length of that marker inside `parent`.
///
/// An unterminated literal extends through the end of `parent`: the
/// caller then reports its incomplete state, never a shape error.
pub(crate) fn expand_literal_token<'a>(
    parent: &'a str,
    first: &'a str,
) -> (&'a str, usize) {
    debug_assert!(
        parent.starts_with(first),
        "expand_literal_token needs the leading token"
    );
    let Some(hash) = first.find('#') else {
        return (first, first.len());
    };
    let after_hash_in_first = &first[hash + 1..];
    if !opens_exact_name(after_hash_in_first) {
        return (first, first.len());
    }
    // The literal may already close inside the first token.
    let remaining_in_first = first.len() - (hash + 1);
    if let Some(lex) =
        lex_exact_name_after_hash(after_hash_in_first, remaining_in_first)
        && lex.complete
    {
        return (first, first.len());
    }
    // Otherwise the closing quote lives in later whitespace-separated
    // text (or is missing): scan the whole parent remainder. `first` is
    // always a prefix of `parent`, so the `#` offset carries over.
    let after_hash = &parent[hash + 1..];
    match lex_exact_name_after_hash(after_hash, after_hash.len()) {
        Some(lex) => {
            let end = hash + 1 + lex.literal_len;
            (parent.get(..end).unwrap_or(parent), end)
        }
        None => (first, first.len()),
    }
}

/// Merge a spaced exact-name literal's continuation tokens into its
/// marker token, so `@route:id#"SASE V18"` lexes whole despite whitespace
/// splitting. `raw` is the physical line the `tokens` were cut from. Every
/// `@`/`^` marker token with an unterminated literal merges; other tokens
/// keep their spans. Tokens join back to the identical body text, so only
/// marker classification sees a difference.
pub(crate) fn merge_leading_literal_token<'a>(
    raw: &super::model::RawLine<'a>,
    mut tokens: Vec<super::model::Token<'a>>,
) -> Vec<super::model::Token<'a>> {
    let mut index = 0;
    while index < tokens.len() {
        let first = tokens[index].text;
        // Only route markers take `#` Pomodoro names; quoted-note
        // modifiers (`&"…"`) and `@@` declarations keep their own quote
        // rules.
        if !(first.starts_with('@') || first.starts_with('^'))
            || first.starts_with("@@")
        {
            index += 1;
            continue;
        }
        let Some(hash) = first.find('#') else {
            index += 1;
            continue;
        };
        let after_hash = &first[hash + 1..];
        if !opens_exact_name(after_hash) {
            index += 1;
            continue;
        }
        if lex_exact_name_after_hash(after_hash, after_hash.len())
            .is_some_and(|lex| lex.complete)
        {
            index += 1;
            continue;
        }
        let rel_start = tokens[index].start.saturating_sub(raw.start);
        let from = raw.text.get(rel_start..).unwrap_or(first);
        if !from.starts_with(first) {
            index += 1;
            continue;
        }
        let (expanded, len) = expand_literal_token(from, first);
        if len <= first.len() {
            index += 1;
            continue;
        }
        let consumed_end = raw.start + rel_start + len;
        let mut end = index + 1;
        while end < tokens.len() && tokens[end].start < consumed_end {
            end += 1;
        }
        tokens[index] = super::model::Token {
            text: expanded,
            start: tokens[index].start,
            end: tokens[index].start + len,
        };
        tokens.drain(index + 1..end);
        index += 1;
    }
    tokens
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lex_reports_straight_and_curly_literals() {
        let straight =
            lex_exact_name_after_hash("\"DEEP WORK\" rest", 17).expect("lex");
        assert_eq!(straight.decoded, "DEEP WORK");
        assert_eq!(straight.literal_len, "\"DEEP WORK\"".len());
        assert!(straight.complete);

        let curly =
            lex_exact_name_after_hash("“SASE V18” rest", "“SASE V18”".len())
                .expect("lex");
        assert_eq!(curly.decoded, "SASE V18");
        assert!(curly.complete);
    }

    #[test]
    fn lex_decodes_only_quote_and_backslash_escapes() {
        let lex =
            lex_exact_name_after_hash("\"a\\\"b\\\\c\\d\"", 12).expect("lex");
        assert_eq!(lex.decoded, "a\"b\\c\\d");
        assert!(lex.complete);
    }

    #[test]
    fn lex_reports_unterminated_as_incomplete() {
        let lex = lex_exact_name_after_hash("\"DEEP WORK", 10).expect("lex");
        assert!(!lex.complete);
        assert_eq!(lex.decoded, "DEEP WORK");
        assert_eq!(lex.literal_len, 10);
    }

    #[test]
    fn lex_rejects_non_quote_openers() {
        assert_eq!(lex_exact_name_after_hash("bugs", 4), None);
    }

    #[test]
    fn expand_covers_spaced_literals() {
        let parent = "@r:id#\"SASE V18\" extra";
        let (marker, len) = expand_literal_token(parent, "@r:id#\"SASE");
        assert_eq!(marker, "@r:id#\"SASE V18\"");
        assert_eq!(len, marker.len());
    }
}
