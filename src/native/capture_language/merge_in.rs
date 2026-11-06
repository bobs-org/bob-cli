//! Shared span-aware merge-in recognizer for `<` capture lines.
//!
//! A merge-in creates one new task and folds existing tasks into it:
//! `T < @a+x @b+y`. Both execution (`draft.rs`) and editor (`editor_parse.rs`)
//! must claim the same parent lines before target-run fan-out.

use super::markers::is_route_token;
use super::model::{RawLine, Token};
use super::task_sources as shared;
use super::tokens::is_parent_task_selector_token;

/// One complete merge-in source with absolute byte offsets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MergeInSource {
    pub(crate) marker: String,
    pub(crate) route: String,
    pub(crate) block_id: String,
    pub(crate) start: usize,
    pub(crate) end: usize,
}

/// The standalone `<` arrow with absolute byte offsets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct MergeInArrow {
    pub(crate) start: usize,
    pub(crate) end: usize,
}

/// Structured draft rewrite attached to an invalid merge-in diagnostic.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MergeInRewrite {
    pub(crate) start: usize,
    pub(crate) end: usize,
    pub(crate) replacement: String,
    pub(crate) label: String,
}

/// Pure scan outcome for one parent line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum MergeInScan {
    Unclaimed,
    Escaped {
        backslash_span: (usize, usize),
        arrow_span: (usize, usize),
    },
    Incomplete {
        sources: Vec<MergeInSource>,
        arrow: MergeInArrow,
        span: (usize, usize),
        message: String,
        partial: Option<(usize, usize)>,
    },
    Valid {
        sources: Vec<MergeInSource>,
        arrow: MergeInArrow,
    },
    Invalid {
        code: &'static str,
        message: String,
        span: (usize, usize),
        rewrite: Option<MergeInRewrite>,
    },
}

pub(crate) const MERGE_IN_ARROW_SPACING_CODE: &str = "merge_in_arrow_spacing";
pub(crate) const MERGE_IN_EXTRA_TEXT_CODE: &str = "merge_in_extra_text";
pub(crate) const MERGE_IN_NEEDS_NEW_TASK_CODE: &str = "merge_in_needs_new_task";
pub(crate) const INCOMPLETE_MERGE_IN_CODE: &str = "incomplete_merge_in";
pub(crate) const MERGE_IN_SOURCE_ALSO_MARKED_CODE: &str =
    "merge_in_source_also_marked";
pub(crate) const MERGE_IN_UNAVAILABLE_CODE: &str = "merge_in_unavailable";

pub(crate) const MERGE_IN_ARROW_SPACING_MESSAGE: &str =
    "put spaces around <: New task < @a+x";
pub(crate) const MERGE_IN_EXTRA_TEXT_MESSAGE: &str =
    "everything about the new task goes before <; only @note+id tasks follow it";
pub(crate) const INCOMPLETE_MERGE_IN_MESSAGE: &str =
    "pick tasks to merge in after <, or write \\< to keep it";
pub(crate) const MERGE_IN_UNAVAILABLE_MESSAGE: &str =
    "merging into a new task with < needs a newer bob";

pub(crate) fn merge_in_source_from_shared(
    source: shared::TaskSource,
) -> MergeInSource {
    MergeInSource {
        marker: source.marker,
        route: source.route,
        block_id: source.block_id,
        start: source.start,
        end: source.end,
    }
}

pub(crate) fn shared_source_from_merge_in(
    source: &MergeInSource,
) -> shared::TaskSource {
    shared::TaskSource {
        marker: source.marker.clone(),
        route: source.route.clone(),
        block_id: source.block_id.clone(),
        start: source.start,
        end: source.end,
    }
}

/// Scan one parent line for a merge-in directive.
pub(crate) fn scan_merge_in_line(
    parent: &RawLine<'_>,
    source: &str,
    _has_children: bool,
) -> MergeInScan {
    let trimmed = parent.text.trim_start();
    if trimmed.is_empty() {
        return MergeInScan::Unclaimed;
    }
    if trimmed.starts_with("@@") {
        return MergeInScan::Unclaimed;
    }
    if trimmed.starts_with('>') {
        return MergeInScan::Unclaimed;
    }
    let tokens = absolutize_and_merge(parent);
    if tokens.is_empty() {
        return MergeInScan::Unclaimed;
    }
    // Any fully protected line stays prose.
    if tokens.iter().all(|token| {
        crate::native::capture_links::is_protected_markup_range(
            source,
            token.start,
            token.end,
        )
    }) {
        return MergeInScan::Unclaimed;
    }
    // Left-to-right: first `\<` escapes, first claimable `<` claims.
    for (index, token) in tokens.iter().enumerate() {
        if token.text == "\\<" {
            if crate::native::capture_links::is_protected_markup_range(
                source,
                token.start,
                token.end,
            ) {
                continue;
            }
            return MergeInScan::Escaped {
                backslash_span: (token.start, token.start + 1),
                arrow_span: (token.start + 1, token.end),
            };
        }
        if token.text != "<" {
            continue;
        }
        if crate::native::capture_links::is_protected_markup_range(
            source,
            token.start,
            token.end,
        ) {
            continue;
        }
        if !is_claimable_following(&tokens, index, source) {
            continue;
        }
        return scan_with_arrow(&tokens, index, parent, source);
    }
    try_glued_arrow(&tokens, parent, source)
}

fn absolutize_and_merge<'a>(parent: &RawLine<'a>) -> Vec<Token<'a>> {
    let rel = super::model::tokenize_with_spans(parent.text);
    let mut abs_tokens: Vec<Token<'_>> = Vec::new();
    for token in rel {
        abs_tokens.push(Token {
            text: token.text,
            start: parent.start + token.start,
            end: parent.start + token.end,
        });
    }
    super::pomodoro_literal::merge_leading_literal_token(parent, abs_tokens)
}

fn is_claimable_following(
    tokens: &[Token<'_>],
    arrow_index: usize,
    source: &str,
) -> bool {
    let Some(next) = tokens.get(arrow_index + 1) else {
        return true;
    };
    is_claimable_token(next, source)
}

fn is_claimable_token(token: &Token<'_>, source: &str) -> bool {
    let text = token.text;
    // `+` / `+query` parent-task selector.
    if text.starts_with('+') {
        return is_parent_task_selector_token(token, source);
    }
    let Some(rest) = text.strip_prefix('@') else {
        return false;
    };
    if rest.is_empty() {
        // Route-prefix typing state `< @`.
        return true;
    }
    // Complete source `@route+id`.
    if shared::parse_complete_source_text(text).is_some() {
        return true;
    }
    // Bare route `@route`.
    if is_route_token(rest)
        && !rest.contains('+')
        && !rest.contains(':')
        && !rest.contains('^')
        && !rest.contains('#')
        && !rest.contains('!')
        && !rest.contains('@')
    {
        return true;
    }
    // Partial `@route+`.
    if rest.ends_with('+') {
        let route = rest.trim_end_matches('+');
        if !route.is_empty() && is_route_token(route) {
            return true;
        }
    }
    // Source-like `@` token containing `+`, `:`, or `^`.
    if rest.contains('+') || rest.contains(':') || rest.contains('^') {
        return true;
    }
    false
}

fn is_complete_source_token(token: &Token<'_>) -> bool {
    shared::parse_complete_source_text(token.text).is_some()
}

fn is_bare_route_token(text: &str) -> bool {
    let Some(rest) = text.strip_prefix('@') else {
        return false;
    };
    if rest.is_empty()
        || rest.contains('+')
        || rest.contains(':')
        || rest.contains('^')
        || rest.contains('#')
        || rest.contains('!')
        || rest.contains('@')
    {
        return false;
    }
    is_route_token(rest)
}

fn is_partial_route_plus_token(text: &str) -> bool {
    let Some(rest) = text.strip_prefix('@') else {
        return false;
    };
    if !rest.ends_with('+') {
        return false;
    }
    let route = rest.trim_end_matches('+');
    !route.is_empty() && is_route_token(route) && !route.contains('+')
}

fn scan_with_arrow(
    tokens: &[Token<'_>],
    arrow_index: usize,
    parent: &RawLine<'_>,
    source: &str,
) -> MergeInScan {
    let arrow_token = &tokens[arrow_index];
    let arrow = MergeInArrow {
        start: arrow_token.start,
        end: arrow_token.end,
    };
    let after = &tokens[arrow_index + 1..];
    if after.is_empty() {
        return MergeInScan::Incomplete {
            sources: Vec::new(),
            arrow,
            span: (arrow.start, arrow.end),
            message: INCOMPLETE_MERGE_IN_MESSAGE.to_string(),
            partial: None,
        };
    }
    // Single `@` typing state.
    if after.len() == 1 && after[0].text == "@" {
        let span = (after[0].start, after[0].end);
        return MergeInScan::Incomplete {
            sources: Vec::new(),
            arrow,
            span,
            message: INCOMPLETE_MERGE_IN_MESSAGE.to_string(),
            partial: Some(span),
        };
    }
    let mut sources: Vec<MergeInSource> = Vec::new();
    let mut index = 0;
    while index < after.len() {
        let token = &after[index];
        let is_last = index + 1 == after.len();
        // A second `<` is extra text.
        if token.text == "<" || token.text == "\\<" || token.text.contains('<')
        {
            let start = token.start;
            let end = after.last().map(|t| t.end).unwrap_or(start);
            return MergeInScan::Invalid {
                code: MERGE_IN_EXTRA_TEXT_CODE,
                message: MERGE_IN_EXTRA_TEXT_MESSAGE.to_string(),
                span: (start, end),
                rewrite: Some(extra_text_rewrite(
                    parent,
                    arrow_index,
                    &sources,
                )),
            };
        }
        if crate::native::capture_links::is_protected_markup_range(
            source,
            token.start,
            token.end,
        ) {
            // Protected token after the arrow: abandon this arrow and look
            // for a later unprotected clause.
            return MergeInScan::Unclaimed;
        }
        if is_complete_source_token(token) {
            match shared::parse_source_token(token) {
                Ok(parsed) => sources.push(merge_in_source_from_shared(parsed)),
                Err(error) => {
                    return MergeInScan::Invalid {
                        code: match error.code {
                            shared::SOURCE_SUFFIX_CODE => {
                                shared::SOURCE_SUFFIX_CODE
                            }
                            _ => shared::SOURCE_MARKER_CODE,
                        },
                        message: error.message,
                        span: error.span,
                        rewrite: None,
                    };
                }
            }
            index += 1;
            continue;
        }
        // Final partials preserve complete sources already read.
        if is_last
            && (is_bare_route_token(token.text)
                || is_partial_route_plus_token(token.text)
                || token.text.starts_with('+')
                    && is_parent_task_selector_token(token, source))
        {
            let span = (token.start, token.end);
            return MergeInScan::Incomplete {
                sources,
                arrow,
                span,
                message: INCOMPLETE_MERGE_IN_MESSAGE.to_string(),
                partial: Some(span),
            };
        }
        // `@route:id` / `@route^id` marker reuse.
        if token.text.starts_with('@')
            && (token.text.contains(':') || token.text.contains('^'))
        {
            match shared::parse_source_token(token) {
                Ok(_) => {}
                Err(error) => {
                    return MergeInScan::Invalid {
                        code: error.code,
                        message: error.message,
                        span: error.span,
                        rewrite: None,
                    };
                }
            }
        }
        // `#drawer` / `!` suffix reuse.
        if token.text.starts_with('@')
            && (token.text.contains('#') || token.text.ends_with('!'))
        {
            match shared::parse_source_token(token) {
                Ok(_) => {}
                Err(error) => {
                    return MergeInScan::Invalid {
                        code: error.code,
                        message: error.message,
                        span: error.span,
                        rewrite: None,
                    };
                }
            }
        }
        // Anything else after the sources is extra text.
        let start = token.start;
        let end = after.last().map(|t| t.end).unwrap_or(start);
        return MergeInScan::Invalid {
            code: MERGE_IN_EXTRA_TEXT_CODE,
            message: MERGE_IN_EXTRA_TEXT_MESSAGE.to_string(),
            span: (start, end),
            rewrite: Some(extra_text_rewrite(parent, arrow_index, &sources)),
        };
    }
    // All tokens consumed as complete sources.
    if sources.is_empty() {
        // Arrow followed only by invalid tokens would have returned above;
        // reaching here with no sources means the single following token
        // was claimable but neither complete nor partial (e.g. marker).
        // Re-run lexical classification for a precise code.
        let token = &after[0];
        match shared::parse_source_token(token) {
            Ok(_) => {}
            Err(error) => {
                return MergeInScan::Invalid {
                    code: error.code,
                    message: error.message,
                    span: error.span,
                    rewrite: None,
                };
            }
        }
        return MergeInScan::Unclaimed;
    }
    // Duplicate check on lowercased routes.
    let shared_sources: Vec<shared::TaskSource> =
        sources.iter().map(shared_source_from_merge_in).collect();
    if let Some(error) = shared::find_lexical_duplicate(&shared_sources) {
        return MergeInScan::Invalid {
            code: error.code,
            message: error.message,
            span: error.span,
            rewrite: None,
        };
    }
    MergeInScan::Valid { sources, arrow }
}

fn extra_text_rewrite(
    parent: &RawLine<'_>,
    arrow_index: usize,
    sources: &[MergeInSource],
) -> MergeInRewrite {
    let tokens = super::model::tokenize_with_spans(parent.text);
    let abs: Vec<(usize, usize, &str)> = tokens
        .iter()
        .map(|token| {
            (
                parent.start + token.start,
                parent.start + token.end,
                token.text,
            )
        })
        .collect();
    // Reconstruct: left part before arrow + extra tokens before `<` +
    // ordered sources. A stray second arrow is dropped.
    let arrow_abs = if arrow_index < abs.len() {
        abs[arrow_index].0
    } else {
        parent.start
    };
    let left_end_rel = arrow_abs.saturating_sub(parent.start);
    let left_raw = parent.text.get(..left_end_rel).unwrap_or("");
    // Find source token texts to retain in order.
    let mut retained: Vec<String> =
        sources.iter().map(|source| source.marker.clone()).collect();
    // Collect extra tokens: tokens after the sources that are not `<`
    // and not retained sources.
    let mut extras: Vec<String> = Vec::new();
    for (_, _, text) in abs.iter().skip(arrow_index + 1 + sources.len()) {
        if *text == "<" || *text == "\\<" {
            continue;
        }
        if retained.iter().any(|marker| marker == text) {
            continue;
        }
        // Tokens containing `<` that are stray arrows: keep their
        // non-arrow text when separable, else drop the arrow.
        if text.contains('<') {
            let cleaned = text.replace('<', "").replace("\\", "");
            if !cleaned.trim().is_empty() {
                extras.push(cleaned.trim().to_string());
            }
            continue;
        }
        extras.push((*text).to_string());
    }
    // Also handle extra tokens interleaved before all sources were
    // consumed: tokens after arrow that are not complete sources are
    // extras, but `sources` only holds complete ones. Re-derive extras
    // from the parent tokens after the arrow excluding retained sources.
    if extras.is_empty() {
        for (_, _, text) in abs.iter().skip(arrow_index + 1) {
            if *text == "<" {
                continue;
            }
            if retained.iter().any(|marker| marker == text) {
                continue;
            }
            if text.contains('<') {
                continue;
            }
            // Complete sources already retained; anything else is extra.
            if shared::parse_complete_source_text(text).is_some() {
                if !retained.iter().any(|marker| marker == text) {
                    retained.push((*text).to_string());
                }
                continue;
            }
            extras.push((*text).to_string());
        }
        // `retained` may now double-count; dedupe preserving order.
        let mut seen = std::collections::HashSet::new();
        retained.retain(|marker| seen.insert(marker.clone()));
        // Extras that are actually retained sources should not move.
        extras.retain(|extra| !retained.iter().any(|marker| marker == extra));
    }
    let left_trimmed = left_raw.trim_end().to_string();
    let mut replacement = left_trimmed;
    if !extras.is_empty() {
        if !replacement.is_empty() {
            replacement.push(' ');
        }
        replacement.push_str(&extras.join(" "));
    }
    if !retained.is_empty() {
        if !replacement.is_empty() {
            replacement.push(' ');
        }
        replacement.push_str("< ");
        replacement.push_str(&retained.join(" "));
    }
    // Preserve leading indentation of the parent line.
    let leading_len = parent.text.len() - parent.text.trim_start().len();
    let leading = parent.text.get(..leading_len).unwrap_or("");
    if !replacement.starts_with(leading) && !leading.is_empty() {
        replacement = format!("{leading}{}", replacement.trim_start());
    }
    MergeInRewrite {
        start: parent.start,
        end: parent.end,
        replacement,
        label: "Move before <".to_string(),
    }
}

fn try_glued_arrow(
    tokens: &[Token<'_>],
    parent: &RawLine<'_>,
    source: &str,
) -> MergeInScan {
    for token in tokens {
        if token.text == "<" || token.text == "\\<" {
            continue;
        }
        let Some(off) = token.text.find('<') else {
            continue;
        };
        // Skip escapes and quoted literals around the `<`.
        if off > 0 && token.text.as_bytes().get(off - 1) == Some(&b'\\') {
            continue;
        }
        if inside_quoted_literal(token.text, off) {
            continue;
        }
        // Autolinks stay prose.
        if token.text.starts_with("<http") || token.text.contains("://") {
            continue;
        }
        let left = &token.text[..off];
        let right = &token.text[off + 1..];
        // `<=`, `=>` (right `=`), `<<`, `<3` stay prose.
        if let Some(next) = right.chars().next()
            && (next == '='
                || next == '<'
                || next == '>'
                || next.is_ascii_digit())
        {
            continue;
        }
        // Only claim glued near-source shapes: the glued `<` must touch
        // a source-like token.
        let right_is_source = right.starts_with('@') && {
            let rest = &right[1..];
            rest.contains('+') || rest.contains(':') || rest.contains('^')
        } || right.starts_with('+');
        let next_is_source = {
            let idx = tokens
                .iter()
                .position(|t| t.start == token.start && t.end == token.end);
            idx.and_then(|i| tokens.get(i + 1)).is_some_and(|next| {
                next.text.starts_with('@') && {
                    let rest = next.text.strip_prefix('@').unwrap_or("");
                    rest.contains('+')
                        || rest.contains(':')
                        || rest.contains('^')
                } || next.text.starts_with('+')
            })
        };
        let left_empty = left.is_empty();
        let right_empty = right.is_empty();
        let glued_to_source =
            right_is_source || (right_empty && next_is_source);
        if !glued_to_source {
            // `text< @a+x`: token `text<` with empty right, next source.
            if !(!left_empty && right_empty && next_is_source) {
                continue;
            }
        }
        if crate::native::capture_links::is_protected_markup_range(
            source,
            token.start + off,
            token.start + off + 1,
        ) {
            continue;
        }
        let abs = token.start + off;
        return MergeInScan::Invalid {
            code: MERGE_IN_ARROW_SPACING_CODE,
            message: MERGE_IN_ARROW_SPACING_MESSAGE.to_string(),
            span: (abs, abs + 1),
            rewrite: Some(spacing_rewrite(parent, abs)),
        };
    }
    MergeInScan::Unclaimed
}

fn inside_quoted_literal(text: &str, off: usize) -> bool {
    let Some(hash) = text.find('#') else {
        return false;
    };
    if off <= hash {
        return false;
    }
    let after = &text[hash + 1..];
    if !super::pomodoro_literal::opens_exact_name(after) {
        return false;
    }
    let Some(lex) =
        super::pomodoro_literal::lex_exact_name_after_hash(after, after.len())
    else {
        return false;
    };
    let lit_start = hash + 1;
    let lit_end = lit_start + lex.literal_len;
    off >= lit_start && off < lit_end
}

fn spacing_rewrite(parent: &RawLine<'_>, arrow_abs: usize) -> MergeInRewrite {
    // Splice spaces around the glued `<` by replacing the whole parent
    // line with a spaced reconstruction. Byte-exact and re-parseable.
    let rel = arrow_abs.saturating_sub(parent.start);
    let before = parent.text.get(..rel).unwrap_or("");
    let after = parent.text.get(rel + 1..).unwrap_or("");
    let before_trimmed = before.trim_end().to_string();
    let after_trimmed = after.trim_start().to_string();
    let mut replacement = before_trimmed;
    if !replacement.is_empty() {
        replacement.push(' ');
    }
    replacement.push('<');
    if !after_trimmed.is_empty() {
        replacement.push(' ');
        replacement.push_str(&after_trimmed);
    }
    let leading_len = parent.text.len() - parent.text.trim_start().len();
    let leading = parent.text.get(..leading_len).unwrap_or("");
    if !replacement.starts_with(leading) && !leading.is_empty() {
        replacement = format!("{leading}{}", replacement.trim_start());
    }
    MergeInRewrite {
        start: parent.start,
        end: parent.end,
        replacement,
        label: "Add spaces".to_string(),
    }
}
