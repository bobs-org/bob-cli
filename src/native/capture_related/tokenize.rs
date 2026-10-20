//! Bob-aware terms with display-title spans.

use std::sync::LazyLock;

use regex::Regex;

use super::super::{capture_block_ids, capture_pomodoro_close, note_tasks};

const STOPWORDS: &[&str] = &[
    "a", "an", "and", "as", "at", "be", "by", "for", "from", "in", "into",
    "is", "it", "its", "my", "of", "on", "or", "our", "so", "that", "the",
    "their", "this", "to", "via", "w", "with", "your",
];

static INLINE_FIELD_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\[[A-Za-z][A-Za-z0-9_-]*::\s*[^\]]*\]")
        .expect("valid inline field regex")
});
static MARKDOWN_LINK_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"!?\[[^\]]*\]\([^)]*\)").expect("valid markdown link regex")
});
static URL_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"https?://\S+").expect("valid url regex"));
static TAG_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"#[A-Za-z][A-Za-z0-9_/-]*").expect("valid tag regex")
});
static CARET_ID_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\^[A-Za-z0-9-]+").expect("valid caret-id regex")
});

/// One stemmed term with its UTF-8 span in the display title.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Token {
    pub(super) term: String,
    pub(super) start: usize,
    pub(super) end: usize,
    pub(super) weight: f64,
}

/// Agenda-clean title: fields and `#task` stripped, wikilinks reduced
/// to their alias (or basename).
pub(super) fn display_title(
    raw: &str,
    global_filter: &str,
    block_id: Option<&str>,
) -> String {
    let cleaned = note_tasks::clean_description(raw, global_filter, block_id);
    let without_task = capture_pomodoro_close::close_task_text(&cleaned);
    capture_block_ids::reduce_wikilinks(&without_task)
}

/// Casefolded title with punctuation, tags, fields, URLs, and emoji
/// removed. Used for the `same_title` band.
pub(super) fn normalize_title(title: &str) -> String {
    let reduced = capture_block_ids::reduce_wikilinks(title);
    let stripped = strip_non_title_markup(&reduced);
    let mut folded = String::new();
    for ch in stripped.chars() {
        if is_emoji(ch) || is_punctuation(ch) {
            if !folded.ends_with(' ') {
                folded.push(' ');
            }
        } else {
            for lower in ch.to_lowercase() {
                folded.push(lower);
            }
        }
    }
    folded.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub(super) fn tokenize(display: &str) -> Vec<Token> {
    let mut tokens = Vec::new();
    let mut index = 0;
    while index < display.len() {
        let rest = &display[index..];
        let first = rest.as_bytes()[0];
        if first == b'`'
            && let Some((end, inner_start, inner_end)) =
                code_span_at(display, index)
        {
            push_atomic(&mut tokens, display, inner_start, inner_end);
            index = end;
            continue;
        }
        if first == b'<'
            && let Some((end, inner_start, inner_end)) =
                chord_at(display, index)
        {
            push_atomic(&mut tokens, display, inner_start, inner_end);
            index = end;
            continue;
        }
        if (first == b'[' || rest.starts_with("!["))
            && let Some((end, text_start, text_end)) =
                markdown_link_at(display, index)
        {
            if text_start < text_end {
                let inner = tokenize(&display[text_start..text_end]);
                for mut token in inner {
                    token.start += text_start;
                    token.end += text_start;
                    tokens.push(token);
                }
            }
            index = end;
            continue;
        }
        if matches!(first, b'[' | b'#' | b'^' | b'h')
            && let Some(end) = skip_markup(display, index)
        {
            index = end;
            continue;
        }
        let ch = rest.chars().next().expect("index in range");
        if ch.is_alphanumeric() {
            let start = index;
            let mut end = index + ch.len_utf8();
            while end < display.len() {
                let next =
                    display[end..].chars().next().expect("index in range");
                if next.is_alphanumeric() || next == '_' || next == '-' {
                    end += next.len_utf8();
                } else {
                    break;
                }
            }
            push_ident_tokens(&mut tokens, display, start, end);
            index = end;
        } else {
            index += ch.len_utf8();
        }
    }
    tokens
}

pub(super) fn unique_term_count(tokens: &[Token]) -> usize {
    let mut seen = Vec::new();
    for token in tokens {
        if !seen.iter().any(|term| term == &token.term) {
            seen.push(token.term.clone());
        }
    }
    seen.len()
}

fn push_atomic(
    tokens: &mut Vec<Token>,
    display: &str,
    start: usize,
    end: usize,
) {
    if start >= end || end > display.len() {
        return;
    }
    let term = display[start..end].to_lowercase();
    if term.is_empty() {
        return;
    }
    tokens.push(Token {
        term,
        start,
        end,
        weight: 2.0,
    });
}

fn push_ident_tokens(
    tokens: &mut Vec<Token>,
    display: &str,
    start: usize,
    end: usize,
) {
    let ident = &display[start..end];
    for (piece, piece_start, piece_end) in split_ident(ident, start) {
        let lower = piece.to_lowercase();
        if is_stopword(&lower) {
            continue;
        }
        let stemmed = stem(&lower);
        if stemmed.len() < 2 {
            continue;
        }
        tokens.push(Token {
            term: stemmed,
            start: piece_start,
            end: piece_end,
            weight: 1.0,
        });
    }
}

fn split_ident(ident: &str, base: usize) -> Vec<(String, usize, usize)> {
    let mut pieces = Vec::new();
    let mut piece_start = 0;
    for (offset, ch) in ident.char_indices() {
        if ch == '_' || ch == '-' {
            if offset > piece_start {
                split_camel(
                    &ident[piece_start..offset],
                    base + piece_start,
                    &mut pieces,
                );
            }
            piece_start = offset + ch.len_utf8();
        }
    }
    if piece_start < ident.len() {
        split_camel(&ident[piece_start..], base + piece_start, &mut pieces);
    }
    pieces
}

fn split_camel(
    word: &str,
    base: usize,
    pieces: &mut Vec<(String, usize, usize)>,
) {
    if word.is_empty() {
        return;
    }
    let chars: Vec<(usize, char)> = word.char_indices().collect();
    let mut starts = vec![0usize];
    for i in 1..chars.len() {
        let prev = chars[i - 1].1;
        let cur = chars[i].1;
        let next = chars.get(i + 1).map(|(_, ch)| *ch);
        if prev.is_lowercase() && cur.is_uppercase() {
            starts.push(chars[i].0);
        } else if prev.is_uppercase()
            && cur.is_uppercase()
            && next.is_some_and(char::is_lowercase)
        {
            starts.push(chars[i].0);
        }
    }
    starts.push(word.len());
    for window in starts.windows(2) {
        let start = window[0];
        let end = window[1];
        if start < end {
            pieces.push((
                word[start..end].to_string(),
                base + start,
                base + end,
            ));
        }
    }
}

fn stem(word: &str) -> String {
    let mut stem = word.to_string();
    if stem.len() > 4 && stem.ends_with("ies") {
        stem.truncate(stem.len() - 3);
        stem.push('y');
    } else if stem.len() > 5 && stem.ends_with("es") {
        stem.truncate(stem.len() - 2);
    } else if stem.len() > 4
        && stem.ends_with('s')
        && !stem.ends_with("ss")
        && !stem.ends_with("us")
        && !stem.ends_with("is")
        && !stem.ends_with("os")
        && !stem.ends_with("as")
    {
        stem.pop();
    }
    if stem.len() > 5 && stem.ends_with("ing") {
        stem.truncate(stem.len() - 3);
    } else if stem.len() > 4 && stem.ends_with("ed") {
        stem.truncate(stem.len() - 2);
    }
    if stem.len() < 2 {
        word.to_string()
    } else {
        stem
    }
}

fn is_stopword(word: &str) -> bool {
    STOPWORDS.contains(&word)
}

fn code_span_at(display: &str, index: usize) -> Option<(usize, usize, usize)> {
    if !display[index..].starts_with('`') {
        return None;
    }
    let rest = &display[index..];
    let fence_len = rest.chars().take_while(|ch| *ch == '`').count();
    if fence_len == 0 {
        return None;
    }
    let inner_start = index + fence_len;
    let closer = "`".repeat(fence_len);
    let relative = display[inner_start..].find(&closer)?;
    let inner_end = inner_start + relative;
    let end = inner_end + fence_len;
    Some((end, inner_start, inner_end))
}

fn chord_at(display: &str, index: usize) -> Option<(usize, usize, usize)> {
    if !display[index..].starts_with('<') {
        return None;
    }
    let close = display[index + 1..].find('>')?;
    let inner_start = index + 1;
    let inner_end = inner_start + close;
    let inner = &display[inner_start..inner_end];
    if !is_chord(inner) {
        return None;
    }
    Some((inner_end + 1, inner_start, inner_end))
}

fn is_chord(inner: &str) -> bool {
    !inner.is_empty()
        && inner.contains('+')
        && inner.chars().all(|ch| {
            ch.is_ascii_alphanumeric() || matches!(ch, '+' | '-' | '_')
        })
}

fn markdown_link_at(
    display: &str,
    index: usize,
) -> Option<(usize, usize, usize)> {
    let rest = &display[index..];
    let matched = MARKDOWN_LINK_RE.find(rest)?;
    if matched.start() != 0 {
        return None;
    }
    let whole = matched.as_str();
    let label_start = if whole.starts_with("![") { 2 } else { 1 };
    let label_end = whole.find(']')?;
    let label = &whole[label_start..label_end];
    let end = index + matched.end();
    if label.contains('💡') || label.chars().all(is_emoji) {
        return Some((end, index, index));
    }
    Some((end, index + label_start, index + label_end))
}

fn skip_markup(display: &str, index: usize) -> Option<usize> {
    let rest = &display[index..];
    if rest.starts_with("[[") {
        let close = rest[2..].find("]]")?;
        return Some(index + 2 + close + 2);
    }
    if rest.starts_with("http://") || rest.starts_with("https://") {
        let end = rest
            .find(|ch: char| ch.is_whitespace())
            .unwrap_or(rest.len());
        return Some(index + end);
    }
    if rest.starts_with('[') {
        let matched = INLINE_FIELD_RE.find(rest)?;
        if matched.start() == 0 {
            return Some(index + matched.end());
        }
    }
    if rest.starts_with('#') {
        let matched = TAG_RE.find(rest)?;
        if matched.start() == 0 {
            return Some(index + matched.end());
        }
    }
    if rest.starts_with('^') {
        let matched = CARET_ID_RE.find(rest)?;
        if matched.start() == 0 {
            return Some(index + matched.end());
        }
    }
    None
}

fn strip_non_title_markup(title: &str) -> String {
    if !title
        .bytes()
        .any(|byte| matches!(byte, b'[' | b'#' | b'^' | b':' | b'h' | b'!'))
    {
        return title.to_string();
    }
    let mut text = INLINE_FIELD_RE.replace_all(title, " ").into_owned();
    text = MARKDOWN_LINK_RE.replace_all(&text, " ").into_owned();
    text = URL_RE.replace_all(&text, " ").into_owned();
    text = TAG_RE.replace_all(&text, " ").into_owned();
    text = CARET_ID_RE.replace_all(&text, " ").into_owned();
    text
}

fn is_punctuation(ch: char) -> bool {
    ch.is_ascii_punctuation() || matches!(ch, '—' | '–' | '…' | '·')
}

fn is_emoji(ch: char) -> bool {
    matches!(ch, '\u{200D}' | '\u{FE0E}' | '\u{FE0F}' | '\u{20E3}')
        || (0x1F000..=0x1FAFF).contains(&(ch as u32))
        || (0x2600..=0x27BF).contains(&(ch as u32))
        || (0x2300..=0x23FF).contains(&(ch as u32))
        || (0x2B00..=0x2BFF).contains(&(ch as u32))
}
