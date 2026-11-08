//! Draft splitting, authored-line classification, and global declarations.

use super::editor_parse::*;
use super::item::*;
use super::line::*;
use super::markers::*;
use super::model::*;
use crate::native::url_routing::is_url_list_line;

/// Split `raw` into physical lines on LF, CRLF, and bare CR alike, so pasted
/// Windows and classic-Mac text behaves exactly like LF text. Byte offsets
/// index the original, un-normalized `raw` string. A trailing line
/// terminator does not produce an extra empty final line; an empty `raw`
/// produces zero lines.
pub(crate) fn split_physical_lines(raw: &str) -> Vec<RawLine<'_>> {
    let bytes = raw.as_bytes();
    let mut lines = Vec::new();
    let mut start = 0usize;
    let mut index = 0usize;

    while index < bytes.len() {
        match bytes[index] {
            b'\n' => {
                lines.push(RawLine {
                    text: &raw[start..index],
                    start,
                    end: index,
                });
                index += 1;
                start = index;
            }
            b'\r' => {
                lines.push(RawLine {
                    text: &raw[start..index],
                    start,
                    end: index,
                });
                index += 1;
                if bytes.get(index) == Some(&b'\n') {
                    index += 1;
                }
                start = index;
            }
            _ => index += 1,
        }
    }

    if start < raw.len() {
        lines.push(RawLine {
            text: &raw[start..],
            start,
            end: raw.len(),
        });
    }

    lines
}

/// Split a draft into declaration-only `@@` lines and the real capture
/// items. A declaration-only line is removed before blank-line item
/// splitting, so it neither becomes an item nor separates adjacent body
/// lines. Item ranges and line numbers always refer back to the complete
/// original draft. A parent line holding a whitespace-separated
/// session-operator chain (`+2 x`) is split into one single-token item per
/// operator; see [`session_chain_tokens`].
pub(crate) fn split_capture_draft(raw: &str) -> CaptureDraft<'_> {
    let lines = split_physical_lines(raw);
    let mut declarations = Vec::new();
    let mut item_lines = Vec::new();

    for (index, line) in lines.iter().copied().enumerate() {
        let tokens = tokenize_line_with_spans(&line);
        if !tokens.is_empty()
            && tokens.iter().all(|token| token.text.starts_with("@@"))
        {
            declarations.extend(tokens.into_iter().map(|token| {
                GlobalDeclarationToken {
                    token,
                    line_number: index + 1,
                }
            }));
            continue;
        }

        item_lines.push(ItemLine {
            raw: line,
            line_number: index + 1,
        });
    }

    CaptureDraft {
        declarations,
        items: split_items_from_item_lines(&item_lines, raw, true),
    }
}

/// Split a draft without recognizing capture lists: headers stay ordinary
/// lines. Used for template rows and forced-destination parses, where
/// `@tokens` keep their literal meaning.
pub(crate) fn split_capture_draft_no_lists(raw: &str) -> CaptureDraft<'_> {
    let lines = split_physical_lines(raw);
    let mut declarations = Vec::new();
    let mut item_lines = Vec::new();

    for (index, line) in lines.iter().copied().enumerate() {
        let tokens = tokenize_line_with_spans(&line);
        if !tokens.is_empty()
            && tokens.iter().all(|token| token.text.starts_with("@@"))
        {
            declarations.extend(tokens.into_iter().map(|token| {
                GlobalDeclarationToken {
                    token,
                    line_number: index + 1,
                }
            }));
            continue;
        }

        item_lines.push(ItemLine {
            raw: line,
            line_number: index + 1,
        });
    }

    CaptureDraft {
        declarations,
        items: split_items_from_item_lines(&item_lines, raw, false),
    }
}

/// Split blank-line-separated item lines into [`CaptureItem`]s. A parent
/// line that is a session-operator chain yields one single-token item per
/// operator; see [`push_capture_item`]. When `allow_lists` is set, a
/// column-zero list header also starts a new item even with no blank line
/// before it, and a header-led block splits into one item per bullet.
pub(super) fn split_items_from_item_lines<'a>(
    lines: &[ItemLine<'a>],
    source: &'a str,
    allow_lists: bool,
) -> Vec<CaptureItem<'a>> {
    let mut items = Vec::new();
    let mut current: Vec<ItemLine<'_>> = Vec::new();

    for line in lines.iter().copied() {
        if line.raw.text.trim().is_empty() {
            push_capture_item(&mut items, &mut current, source, allow_lists);
            continue;
        }
        if allow_lists
            && !current.is_empty()
            && detect_list_header(&line.raw).is_some()
        {
            push_capture_item(&mut items, &mut current, source, allow_lists);
        }

        current.push(line);
    }

    push_capture_item(&mut items, &mut current, source, allow_lists);
    items
}

/// A lone `@route` / `@route#section` line that heads a capture list.
/// `exact` preserves a quoted selector's exact-match semantics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ListHeader {
    pub(crate) route: String,
    pub(crate) section: Option<String>,
    pub(crate) exact: bool,
    pub(crate) kind: ListKind,
}

/// Parse one complete list-header route token, recognizing a quoted
/// `#` selector that may hold spaces. Returns the route, section, exact
/// flag, and list kind. A multi-token line, an incomplete literal, or
/// trailing bytes after a closing quote are not a complete header.
fn parse_list_header_token(
    trimmed: &str,
) -> Option<(String, Option<String>, bool, ListKind)> {
    use super::pomodoro_literal::{
        expand_literal_token, lex_exact_name_after_hash, opens_exact_name,
    };
    if trimmed.starts_with("@@") || !trimmed.starts_with('@') {
        return None;
    }
    let first = trimmed.split_whitespace().next()?;
    let (marker, len) = expand_literal_token(trimmed, first);
    if len != trimmed.len() {
        return None;
    }
    let Some(hash) = marker.find('#') else {
        let route_part = marker.strip_prefix('@')?;
        if !super::markers::is_route_token(route_part) {
            return None;
        }
        return Some((
            route_part.to_ascii_lowercase(),
            None,
            false,
            ListKind::Tasks,
        ));
    };
    let route_part = marker[1..hash].to_string();
    if !super::markers::is_route_token(&route_part) {
        return None;
    }
    let after_hash = &marker[hash + 1..];
    if after_hash.is_empty() {
        return Some((
            route_part.to_ascii_lowercase(),
            None,
            false,
            ListKind::Notes,
        ));
    }
    if opens_exact_name(after_hash) {
        let lex = lex_exact_name_after_hash(after_hash, after_hash.len())?;
        if !lex.complete
            || lex.literal_len != after_hash.len()
            || lex.decoded.is_empty()
        {
            return None;
        }
        return Some((
            route_part.to_ascii_lowercase(),
            Some(lex.decoded),
            true,
            ListKind::Notes,
        ));
    }
    if after_hash.contains(char::is_whitespace) {
        return None;
    }
    Some((
        route_part.to_ascii_lowercase(),
        Some(after_hash.to_string()),
        false,
        ListKind::Notes,
    ))
}

/// Detect a list header: a column-zero line whose trimmed text is exactly
/// one complete route token (`@route`, `@route#section`,
/// `@route#"Literal"` with spaces, or `@route#`). `@@` declarations,
/// indented lines, and multi-token lines never qualify; anything else
/// keeps its existing meaning. Surrounding whitespace on the header line
/// is honored via trimming, while indented later lines never start lists.
pub(crate) fn detect_list_header(line: &RawLine<'_>) -> Option<ListHeader> {
    if line.text.len() != line.text.trim_start().len() {
        return None;
    }
    let trimmed = line.text.trim();
    if trimmed.is_empty() {
        return None;
    }
    let (route, section, exact, kind) = parse_list_header_token(trimmed)?;
    // Only task and bullet markers head lists; operators, links, toggles,
    // and project notes keep their existing meanings.
    match kind {
        ListKind::Tasks | ListKind::Notes => Some(ListHeader {
            route,
            section,
            exact,
            kind,
        }),
    }
}

/// Group a header-led block into its ownerless leading lines plus one
/// per-bullet block (each bullet plus the nested lines that follow it).
/// Placeholders never start a block and never reset the current bullet; any
/// other non-bullet line joins the current block (or the ownerless prefix
/// when no bullet precedes it) so the item parser reports its existing
/// orphan/nested/continuation diagnostic.
fn group_list_blocks<'a>(
    lines: &[ItemLine<'a>],
) -> (Vec<ItemLine<'a>>, Vec<Vec<ItemLine<'a>>>) {
    let mut ownerless = Vec::new();
    let mut bullets: Vec<Vec<ItemLine<'a>>> = Vec::new();
    let mut current: Option<Vec<ItemLine<'a>>> = None;
    for line in lines.iter().skip(1).copied() {
        match classify_authored_line(line.raw) {
            AuthoredLineClass::EmptyOrPlaceholder => continue,
            AuthoredLineClass::Item(authored)
                if authored.depth == AuthoredDepth::First =>
            {
                if let Some(block) = current.take() {
                    bullets.push(block);
                }
                current = Some(vec![line]);
            }
            AuthoredLineClass::Item(_) | AuthoredLineClass::Invalid => {
                if let Some(block) = current.as_mut() {
                    block.push(line);
                } else {
                    ownerless.push(line);
                }
            }
        }
    }
    if let Some(block) = current.take() {
        bullets.push(block);
    }
    (ownerless, bullets)
}

/// Push one header-led block as one [`CaptureItem`] per nonempty list
/// bullet plus at most one ownerless prefix item. Each per-bullet item holds
/// zero-copy slices of the original draft (the bullet's body slice plus its
/// nested lines) with a shared [`ListContext`], so every span, range, and
/// diagnostic offset stays exact. A header with no bullets and no stray
/// lines becomes one header-only item carrying `count: 0`.
fn push_list_items<'a>(
    items: &mut Vec<CaptureItem<'a>>,
    current: &mut Vec<ItemLine<'a>>,
    source: &'a str,
    header: ListHeader,
) {
    let block_lines = std::mem::take(current);
    let first = block_lines.first().expect("header-led block");
    let (ownerless, bullets) = group_list_blocks(&block_lines);
    let base = ListContext {
        kind: header.kind,
        route: header.route,
        section: header.section,
        header_line: first.line_number,
        position: 0,
        count: bullets.len(),
        exact: header.exact,
        header_start: first.raw.start,
        header_end: first.raw.end,
        ownerless: false,
    };
    let mut emitted_ownerless = false;
    if !ownerless.is_empty() {
        let last = ownerless.last().expect("nonempty ownerless");
        let first_ownerless = ownerless.first().expect("nonempty ownerless");
        items.push(CaptureItem {
            source,
            index: items.len(),
            start: first_ownerless.raw.start,
            end: last.raw.end,
            line_start: first_ownerless.line_number,
            line_end: last.line_number,
            lines: ownerless,
            list: Some(ListContext {
                ownerless: true,
                ..base.clone()
            }),
        });
        emitted_ownerless = true;
    }
    for (position, bullet) in bullets.into_iter().enumerate() {
        let (start, line_start, parent, end, line_end) = {
            let first_bullet = bullet.first().expect("nonempty bullet");
            let last_bullet = bullet.last().expect("nonempty bullet");
            let authored = match classify_authored_line(first_bullet.raw) {
                AuthoredLineClass::Item(authored) => authored,
                _ => continue,
            };
            let parent = ItemLine {
                raw: RawLine {
                    text: authored.body,
                    start: authored.body_start,
                    end: first_bullet.raw.end,
                },
                line_number: first_bullet.line_number,
            };
            (
                first_bullet.raw.start,
                first_bullet.line_number,
                parent,
                last_bullet.raw.end,
                last_bullet.line_number,
            )
        };
        let mut lines = Vec::with_capacity(bullet.len());
        lines.push(parent);
        lines.extend(bullet.into_iter().skip(1));
        items.push(CaptureItem {
            source,
            index: items.len(),
            start,
            end,
            line_start,
            line_end,
            lines,
            list: Some(ListContext {
                position: position + 1,
                ..base.clone()
            }),
        });
    }
    if base.count == 0 && !emitted_ownerless {
        items.push(CaptureItem {
            source,
            index: items.len(),
            start: first.raw.start,
            end: first.raw.end,
            line_start: first.line_number,
            line_end: first.line_number,
            lines: vec![*first],
            list: Some(base),
        });
    }
}

/// Merge spaced exact-name past tokens (`x=3#"DEEP WORK"`) that
/// `masked_tokens` split on whitespace. Only `x=` tokens with an
/// unterminated literal in their first whitespace piece expand through the
/// closing quote plus glued `!` and `=X` suffixes (up to the next
/// whitespace), preserving absolute offsets. A retired `@D` after the
/// literal stays claimed with the token so it teaches its rewrite. The
/// following inline entry or next session token is never swallowed.
fn merge_past_literal_tokens<'a>(
    raw: &RawLine<'a>,
    mut tokens: Vec<Token<'a>>,
) -> Vec<Token<'a>> {
    use super::pomodoro_literal::{
        lex_exact_name_after_hash, opens_exact_name,
    };
    let mut index = 0;
    while index < tokens.len() {
        let first = tokens[index].text;
        if !first.starts_with("x=") {
            index += 1;
            continue;
        }
        let Some(hash_rel) = first.find('#') else {
            index += 1;
            continue;
        };
        let after_hash_in_first = &first[hash_rel + 1..];
        if !opens_exact_name(after_hash_in_first) {
            index += 1;
            continue;
        }
        // Already whole when the literal closes inside the first piece.
        if lex_exact_name_after_hash(
            after_hash_in_first,
            after_hash_in_first.len(),
        )
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
        let Some(hash_in_from) = from.find('#') else {
            index += 1;
            continue;
        };
        let after_hash_full = &from[hash_in_from + 1..];
        let Some(lex) =
            lex_exact_name_after_hash(after_hash_full, after_hash_full.len())
        else {
            index += 1;
            continue;
        };
        let literal_end = hash_in_from + 1 + lex.literal_len;
        let after_literal = from.get(literal_end..).unwrap_or("");
        let suffix_len = after_literal
            .find(|c: char| c.is_ascii_whitespace())
            .unwrap_or(after_literal.len());
        let expanded_len = literal_end + suffix_len;
        if expanded_len <= first.len() {
            index += 1;
            continue;
        }
        let expanded = from.get(..expanded_len).unwrap_or(first);
        let consumed_end = tokens[index].start + expanded_len;
        let mut end = index + 1;
        while end < tokens.len() && tokens[end].start < consumed_end {
            end += 1;
        }
        tokens[index] = Token {
            text: expanded,
            start: tokens[index].start,
            end: tokens[index].start + expanded_len,
        };
        tokens.drain(index + 1..end);
        index += 1;
    }
    tokens
}

/// Masked whitespace tokens plus merged past literals, the view
/// [`session_chain_tokens`] uses to decide whether a line is a chain.
pub(super) fn line_session_view_tokens<'a>(
    line: &RawLine<'a>,
) -> Vec<Token<'a>> {
    merge_past_literal_tokens(line, super::type_marker::masked_tokens(line))
}

/// Return the line's whitespace-separated tokens when the parent line is
/// a session-operator chain: at least two tokens, every one passing
/// [`is_session_chain_token`]. A single token is never a chain, so the
/// existing single-token path stays byte-identical. Past tokens with spaced
/// exact names stay whole via [`merge_past_literal_tokens`].
pub(super) fn session_chain_tokens<'a>(
    line: &RawLine<'a>,
) -> Option<Vec<Token<'a>>> {
    let tokens = line_session_view_tokens(line);
    if tokens.len() >= 2
        && tokens
            .iter()
            .all(|token| is_session_chain_token(token.text))
    {
        Some(tokens)
    } else {
        None
    }
}

/// Whether a chain token is a whole-item close: a bare `x`, `x<N>…`,
/// `x#…`, a past `x=` token, or a retired `=x`/`=*`/`=!` spelling that
/// teaches the rewrite. Past counts as a close so it owns inline entries
/// and child bullets.
fn is_chain_close_token(token: &str) -> bool {
    matches!(
        super::item::session_equals_token(token),
        Some(super::item::EqualsToken::Close)
            | Some(super::item::EqualsToken::Past { .. })
    )
}

/// Whether a token starts a new session: a whole-item start (`=`, `=<X>`,
/// `=name`, `=<X>name`, `=<X>#name` when needed, `=~K` with its drop part)
/// or a whole-item close (`x…`).
/// Adjust/shift tokens (`+1`, `-`, `--`) never start a session, so they
/// stay entry text unless a start or close precedes them in the trail.
fn is_trail_head_token(token: &str) -> bool {
    super::item::session_equals_token(token).is_some()
}

/// One inline-close split: the lead (maximal session-token prefix), its
/// last whole-item close (the owner), and the trail (maximal
/// session-token suffix trimmed to begin with a start or close token).
/// The entry is every token after the owner and before the trail.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct InlineCloseSplit {
    pub(crate) lead_len: usize,
    pub(crate) owner_index: usize,
    pub(crate) trail_start: usize,
}

/// Split a parent line's tokens into lead/owner/entry/trail for an inline
/// Work Log entry. Returns `None` when the line holds no inline entry:
/// fewer than two tokens, an all-session chain (today's chain path), an
/// empty lead, or no whole-item close in the lead. Both the execution and
/// editor paths share this split through [`split_capture_draft`], so they
/// cannot disagree. The entry is never empty when `Some`: the token after
/// the lead is not a session token.
pub(crate) fn split_inline_close_tokens(
    tokens: &[Token<'_>],
) -> Option<InlineCloseSplit> {
    if tokens.len() < 2 {
        return None;
    }
    if tokens
        .iter()
        .all(|token| is_session_chain_token(token.text))
    {
        return None;
    }
    let lead_len = tokens
        .iter()
        .take_while(|token| is_session_chain_token(token.text))
        .count();
    if lead_len == 0 {
        return None;
    }
    let owner_index = tokens[..lead_len]
        .iter()
        .rposition(|token| is_chain_close_token(token.text))?;
    let suffix_len = tokens
        .iter()
        .rev()
        .take_while(|token| is_session_chain_token(token.text))
        .count();
    let mut trail_start = tokens.len() - suffix_len;
    while trail_start < tokens.len()
        && !is_trail_head_token(tokens[trail_start].text)
    {
        trail_start += 1;
    }
    if owner_index + 1 >= trail_start {
        return None;
    }
    Some(InlineCloseSplit {
        lead_len,
        owner_index,
        trail_start,
    })
}

/// Push one blank-line-separated item. When the parent line is a session
/// chain, push one single-token item per operator instead: each synthetic
/// item holds a single [`ItemLine`] over that token's absolute range on the
/// physical line, with sequential `index` numbering across the draft. Child
/// lines attach to the line's `x` item — or to the last `x` when the line
/// closes twice — extending its `lines`, `end`, and `line_end`. With no `x`
/// on the line, children still attach to the last token's item, so the last
/// token's family parser reports its existing shape error.
///
/// A parent line with an inline Work Log entry (`x wired the lexer`)
/// splits into one item per lead token before the owner, then the owner
/// item carrying the entry, then one item per trail token. The owner item's
/// single [`ItemLine`] is the contiguous slice from the owner token's start
/// through the entry's last token, so the close parser sees exactly
/// `x… <entry>`. Child lines attach to the owner, so mixing entry and
/// bullets reports the mixing error. With no siblings the result is one
/// item over the whole line, as today.
///
/// A chain whose `x` is not last nests its item ranges: the close item's
/// range runs from its token through its last bullet, so it contains the
/// ranges of the later tokens on the parent line. An inline owner never
/// nests siblings: its range ends at the entry (or its last child). Item
/// ranges therefore nest or stay disjoint; they never partially overlap.
pub(super) fn push_capture_item<'a>(
    items: &mut Vec<CaptureItem<'a>>,
    current: &mut Vec<ItemLine<'a>>,
    source: &'a str,
    allow_lists: bool,
) {
    let Some(first) = current.first().copied() else {
        return;
    };
    // A blank-line-free block of two or more URL-list lines splits into one
    // item per line. The test is purely lexical, so parse, preview, and
    // submit always agree; each item classifies on its own later.
    if current.len() >= 2
        && current.iter().all(|line| {
            is_url_list_line(&super::type_marker::masked_line(&line.raw))
        })
    {
        for line in std::mem::take(current) {
            items.push(CaptureItem {
                source,
                index: items.len(),
                start: line.raw.start,
                end: line.raw.end,
                line_start: line.line_number,
                line_end: line.line_number,
                lines: vec![line],
                list: None,
            });
        }
        return;
    }
    if let Some(tokens) = session_chain_tokens(&first.raw) {
        let rest: Vec<ItemLine<'a>> = current[1..].to_vec();
        let last_child = rest.last().copied();
        let type_markers =
            super::type_marker::scan_line(first.raw.text, first.raw.start)
                .into_iter()
                .filter(|occurrence| occurrence.is_marker())
                .collect::<Vec<_>>();
        let leading_marker_start = type_markers
            .iter()
            .filter(|marker| marker.span().end <= tokens[0].start)
            .map(|marker| marker.span().start)
            .min();
        let trailing_marker_end = type_markers
            .iter()
            .filter(|marker| marker.span().start >= tokens.last().unwrap().end)
            .map(|marker| marker.span().end)
            .max();
        let owner = tokens
            .iter()
            .rposition(|token| is_chain_close_token(token.text))
            .unwrap_or(tokens.len() - 1);
        for (position, token) in tokens.iter().copied().enumerate() {
            let is_owner = position == owner;
            let start = if position == 0 {
                leading_marker_start.unwrap_or(token.start)
            } else {
                token.start
            };
            let end = if position + 1 == tokens.len() {
                trailing_marker_end.unwrap_or(token.end)
            } else {
                token.end
            };
            let token_line = ItemLine {
                raw: RawLine {
                    text: &source[start..end],
                    start,
                    end,
                },
                line_number: first.line_number,
            };
            if is_owner {
                let mut lines = vec![token_line];
                lines.extend(rest.iter().copied());
                let (base_end, line_end) = match last_child {
                    Some(child) => (child.raw.end, child.line_number),
                    None => (token.end, first.line_number),
                };
                let item_end = base_end.max(end).max(token.end);
                items.push(CaptureItem {
                    source,
                    index: items.len(),
                    start,
                    end: item_end,
                    line_start: first.line_number,
                    line_end,
                    lines,
                    list: None,
                });
            } else {
                items.push(CaptureItem {
                    source,
                    index: items.len(),
                    start,
                    end,
                    line_start: first.line_number,
                    line_end: first.line_number,
                    lines: vec![token_line],
                    list: None,
                });
            }
        }
        current.clear();
        return;
    }
    let parent_tokens = merge_past_literal_tokens(
        &first.raw,
        super::type_marker::masked_tokens(&first.raw),
    );
    if let Some(split) = split_inline_close_tokens(&parent_tokens) {
        let lead_before = &parent_tokens[..split.owner_index];
        let trail = &parent_tokens[split.trail_start..];
        let type_markers =
            super::type_marker::scan_line(first.raw.text, first.raw.start)
                .into_iter()
                .filter(|occurrence| occurrence.is_marker())
                .collect::<Vec<_>>();
        let leading_marker_start = type_markers
            .iter()
            .filter(|marker| marker.span().end <= parent_tokens[0].start)
            .map(|marker| marker.span().start)
            .min();
        let trailing_marker_end = type_markers
            .iter()
            .filter(|marker| {
                marker.span().start >= parent_tokens.last().unwrap().end
            })
            .map(|marker| marker.span().end)
            .max();
        // No siblings: one item over the whole line, as today. The close
        // parser lexes the tail as the inline entry.
        if lead_before.is_empty() && trail.is_empty() {
            let last = current.last().copied().expect("nonempty item");
            items.push(CaptureItem {
                source,
                index: items.len(),
                start: first.raw.start,
                end: last.raw.end,
                line_start: first.line_number,
                line_end: last.line_number,
                lines: std::mem::take(current),
                list: None,
            });
            return;
        }
        let rest: Vec<ItemLine<'a>> = current[1..].to_vec();
        let last_child = rest.last().copied();
        for (position, token) in lead_before.iter().copied().enumerate() {
            let start = if position == 0 {
                leading_marker_start.unwrap_or(token.start)
            } else {
                token.start
            };
            let token_line = ItemLine {
                raw: RawLine {
                    text: &source[start..token.end],
                    start,
                    end: token.end,
                },
                line_number: first.line_number,
            };
            items.push(CaptureItem {
                source,
                index: items.len(),
                start,
                end: token.end,
                line_start: first.line_number,
                line_end: first.line_number,
                lines: vec![token_line],
                list: None,
            });
        }
        let owner = parent_tokens[split.owner_index];
        let entry_last = parent_tokens[split.trail_start - 1];
        let owner_start = if lead_before.is_empty() {
            leading_marker_start.unwrap_or(owner.start)
        } else {
            owner.start
        };
        let owner_line_end = if trail.is_empty() {
            trailing_marker_end.unwrap_or(entry_last.end)
        } else {
            entry_last.end
        };
        let owner_rel_start = owner_start - first.raw.start;
        let entry_rel_end = owner_line_end - first.raw.start;
        let owner_text = &first.raw.text[owner_rel_start..entry_rel_end];
        let owner_line = ItemLine {
            raw: RawLine {
                text: owner_text,
                start: owner_start,
                end: owner_line_end,
            },
            line_number: first.line_number,
        };
        {
            let mut lines = vec![owner_line];
            lines.extend(rest.iter().copied());
            let (end, line_end) = match last_child {
                Some(child) => {
                    (child.raw.end.max(owner_line_end), child.line_number)
                }
                None => (owner_line_end, first.line_number),
            };
            items.push(CaptureItem {
                source,
                index: items.len(),
                start: owner_start,
                end,
                line_start: first.line_number,
                line_end,
                lines,
                list: None,
            });
        }
        for (position, token) in trail.iter().copied().enumerate() {
            let end = if position + 1 == trail.len() {
                trailing_marker_end.unwrap_or(token.end)
            } else {
                token.end
            };
            let token_line = ItemLine {
                raw: RawLine {
                    text: &source[token.start..end],
                    start: token.start,
                    end,
                },
                line_number: first.line_number,
            };
            items.push(CaptureItem {
                source,
                index: items.len(),
                start: token.start,
                end,
                line_start: first.line_number,
                line_end: first.line_number,
                lines: vec![token_line],
                list: None,
            });
        }
        current.clear();
        return;
    }
    if allow_lists && let Some(header) = detect_list_header(&first.raw) {
        push_list_items(items, current, source, header);
        return;
    }
    let last = current.last().copied().expect("nonempty item");
    items.push(CaptureItem {
        source,
        index: items.len(),
        start: first.raw.start,
        end: last.raw.end,
        line_start: first.line_number,
        line_end: last.line_number,
        lines: std::mem::take(current),
        list: None,
    });
}

/// One authored continuation line after its list marker has been stripped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct AuthoredLine<'a> {
    pub(crate) body: &'a str,
    pub(crate) body_start: usize,
    pub(crate) depth: AuthoredDepth,
}

/// The shared physical-line classifier for authored capture children.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AuthoredLineClass<'a> {
    EmptyOrPlaceholder,
    Item(AuthoredLine<'a>),
    Invalid,
}

/// Classify one physical continuation line. A real item is either a
/// column-zero `-`/`*`/`+` bullet or the same bullet prefixed by exactly two
/// ASCII spaces. Whitespace-only rows are batch separators before strict
/// item parsing reaches this classifier; when classified directly, they and
/// marker-only placeholder rows are harmless. Every other nonempty shape is
/// invalid.
pub(crate) fn classify_authored_line<'a>(
    line: RawLine<'a>,
) -> AuthoredLineClass<'a> {
    if line.text.trim().is_empty() {
        return AuthoredLineClass::EmptyOrPlaceholder;
    }

    if let Some((body_offset, body)) = strip_bullet_marker_at(line.text, 0) {
        if body.trim().is_empty() {
            return AuthoredLineClass::EmptyOrPlaceholder;
        }
        return AuthoredLineClass::Item(AuthoredLine {
            body,
            body_start: line.start + body_offset,
            depth: AuthoredDepth::First,
        });
    }

    if let Some((body_offset, body)) = strip_bullet_marker_at(line.text, 2) {
        if body.trim().is_empty() {
            return AuthoredLineClass::EmptyOrPlaceholder;
        }
        return AuthoredLineClass::Item(AuthoredLine {
            body,
            body_start: line.start + body_offset,
            depth: AuthoredDepth::Nested,
        });
    }

    if is_marker_only_placeholder(line.text) {
        return AuthoredLineClass::EmptyOrPlaceholder;
    }

    AuthoredLineClass::Invalid
}

/// Recognize a list marker at a fixed byte prefix and return the raw body
/// after the contiguous separator run. `prefix_len == 2` accepts exactly two
/// leading spaces; one, three, a tab, or deeper indentation all fail.
pub(super) fn strip_bullet_marker_at(
    line_text: &str,
    prefix_len: usize,
) -> Option<(usize, &str)> {
    let prefix = line_text.as_bytes().get(..prefix_len)?;
    if prefix_len == 2 && prefix != b"  " {
        return None;
    }
    if prefix_len == 0 && line_text.as_bytes().first() == Some(&b' ') {
        return None;
    }

    let marker = *line_text.as_bytes().get(prefix_len)?;
    if !matches!(marker, b'-' | b'*' | b'+') {
        return None;
    }
    let separator_index = prefix_len + 1;
    let separator = *line_text.as_bytes().get(separator_index)?;
    if !matches!(separator, b' ' | b'\t') {
        return None;
    }

    let bytes = line_text.as_bytes();
    let mut end = separator_index + 1;
    while end < bytes.len() && matches!(bytes[end], b' ' | b'\t') {
        end += 1;
    }
    Some((end, &line_text[end..]))
}

pub(super) fn is_marker_only_placeholder(line_text: &str) -> bool {
    marker_only_placeholder_after_prefix(line_text, 0)
        || marker_only_placeholder_after_prefix(line_text, 1)
        || marker_only_placeholder_after_prefix(line_text, 2)
}

pub(super) fn marker_only_placeholder_after_prefix(
    line_text: &str,
    prefix_len: usize,
) -> bool {
    let bytes = line_text.as_bytes();
    let Some(prefix) = bytes.get(..prefix_len) else {
        return false;
    };
    if !prefix.iter().all(|byte| *byte == b' ') {
        return false;
    }
    let Some(marker) = bytes.get(prefix_len) else {
        return false;
    };
    matches!(marker, b'-' | b'*' | b'+')
        && bytes[prefix_len + 1..]
            .iter()
            .all(|byte| matches!(byte, b' ' | b'\t'))
}

#[cfg(test)]
pub(crate) fn parse_capture_text_with_options(
    raw_text: &str,
    forced_route: Option<&str>,
    forced_section: Option<&str>,
    options: &CaptureParseOptions<'_>,
) -> Result<ParsedCaptureText, String> {
    let draft = split_capture_draft(raw_text);
    if draft.items.is_empty() {
        let global = resolve_global_declaration_strict(&draft.declarations)?;
        return Err(if global.is_some() {
            missing_capture_item_error()
        } else {
            missing_text_error()
        });
    }
    let options = options.with_global(!draft.declarations.is_empty());

    // Singleton parsing claims transfers and merge-ins exactly like the
    // draft parser but keeps ordinary items on the unfanned singleton parser.
    let mut item_outcomes = Vec::new();
    for item in &draft.items {
        if let Some(outcomes) =
            parse_transfer_claim(item, forced_route, forced_section, &options)
                .map_err(|error| error.message)?
        {
            item_outcomes.extend(outcomes);
            continue;
        }
        match parse_merge_claim(item, forced_route, forced_section, &options)
            .map_err(|error| error.message)?
        {
            Some(outcomes) => item_outcomes.extend(outcomes),
            None => item_outcomes.push(
                parse_capture_item(
                    item,
                    forced_route,
                    forced_section,
                    &options,
                    None,
                )
                .map_err(|error| error.message)?,
            ),
        }
    }
    let mut declarations = draft.declarations;
    for outcome in &item_outcomes {
        declarations.extend(outcome.declarations.iter().copied());
    }
    let global = resolve_global_declaration_strict(&declarations)?;

    if item_outcomes.len() > 1 {
        return Err(
            "capture text contains multiple blank-line-separated items"
                .to_string(),
        );
    }

    let mut outcome = item_outcomes.into_iter().next().expect("one item");
    // A plain `@@route` upgrades a default (routeless) Ref to its global
    // parent; an explicit local `@route` or forced `-r` keeps local precedence.
    if let Some(global) = &global
        && matches!(outcome.parsed.kind, CaptureKind::Ref(_))
        && outcome.parsed.route.is_none()
    {
        outcome.parsed.route = Some(global.route.clone());
    }
    if forced_route.is_none()
        && let Some(global) = &global
    {
        let inheritable = outcome.parsed.route.is_none()
            && matches!(outcome.parsed.kind, CaptureKind::Task);
        inherit_global_destination(&mut outcome.parsed, global);
        if let Some(error) =
            super::type_marker::not_applicable_error(&outcome.parsed)
        {
            return Err(error);
        }
        finish_item_dependency_target(&mut outcome.parsed, inheritable)?;
    } else {
        finish_item_dependency_target(&mut outcome.parsed, false)?;
    }
    Ok(outcome.parsed)
}

/// A template-aware parse failure with its stable JSON code, when the
/// failure is a template refusal. `code` is `None` for ordinary capture
/// errors, which stay generic usage errors.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TemplateCodedError {
    pub(crate) code: Option<&'static str>,
    pub(crate) message: String,
    pub(crate) item: Option<usize>,
    pub(crate) span: Option<(usize, usize)>,
}

impl TemplateCodedError {
    fn generic(message: String) -> Self {
        Self {
            code: None,
            message,
            item: None,
            span: None,
        }
    }

    pub(crate) fn coded(code: &'static str, message: String) -> Self {
        Self {
            code: Some(code),
            message,
            item: None,
            span: None,
        }
    }
}

impl From<CaptureItemParseError> for TemplateCodedError {
    fn from(error: CaptureItemParseError) -> Self {
        Self {
            code: error.code,
            message: error.message,
            item: error.item,
            span: error.span,
        }
    }
}

#[cfg(test)]
pub(crate) fn parse_capture_draft_with_options(
    raw_text: &str,
    forced_route: Option<&str>,
    forced_section: Option<&str>,
    options: &CaptureParseOptions<'_>,
) -> Result<ParsedCaptureDraft, String> {
    parse_capture_draft_with_options_coded(
        raw_text,
        forced_route,
        forced_section,
        options,
    )
    .map_err(|error| error.message)
}

pub(crate) fn parse_capture_draft_with_options_coded(
    raw_text: &str,
    forced_route: Option<&str>,
    forced_section: Option<&str>,
    options: &CaptureParseOptions<'_>,
) -> Result<ParsedCaptureDraft, TemplateCodedError> {
    let analysis = super::template::analyze(raw_text);
    if analysis.directive.is_some() {
        if let Some(block) = analysis.block.as_ref()
            && block.definition_lines(raw_text).iter().any(|line| {
                detect_list_header(&RawLine {
                    text: line,
                    start: 0,
                    end: line.len(),
                })
                .is_some()
            })
        {
            return Err(TemplateCodedError::coded(
                "template_list",
                template_list_error(),
            ));
        }
        if let Some(first_issue) = analysis.issues.first() {
            if let Some(row) =
                super::template::row_number_for_issue(&analysis, first_issue)
                && let Some(execution) = first_leading_execution_error(
                    raw_text,
                    &analysis,
                    row,
                    forced_route,
                    forced_section,
                    options,
                )
            {
                return Err(execution);
            }
            return Err(TemplateCodedError::coded(
                first_issue.code(),
                first_issue.message(),
            ));
        }
        let Some(expansion) =
            super::template::expand(raw_text).map_err(|issue| {
                TemplateCodedError::coded(issue.code(), issue.message())
            })?
        else {
            // A missing definition is already reported as a language
            // issue above; reaching here means an ordinary draft.
            return parse_ordinary_draft(
                raw_text,
                forced_route,
                forced_section,
                options,
            )
            .map_err(TemplateCodedError::from);
        };
        return parse_template_expansion(
            expansion,
            forced_route,
            forced_section,
            options,
        );
    }
    if let Some(expansion) =
        super::template::expand(raw_text).map_err(|issue| {
            TemplateCodedError::coded(issue.code(), issue.message())
        })?
    {
        return parse_template_expansion(
            expansion,
            forced_route,
            forced_section,
            options,
        );
    }
    parse_ordinary_draft(raw_text, forced_route, forced_section, options)
        .map_err(TemplateCodedError::from)
}

fn parse_ordinary_draft(
    raw_text: &str,
    forced_route: Option<&str>,
    forced_section: Option<&str>,
    options: &CaptureParseOptions<'_>,
) -> Result<ParsedCaptureDraft, CaptureItemParseError> {
    // Forced destinations keep `@tokens` literal, so no header is
    // recognized there.
    let allow_lists = options.recognize_lists
        && forced_route.is_none()
        && forced_section.is_none();
    let draft = if allow_lists {
        split_capture_draft(raw_text)
    } else {
        split_capture_draft_no_lists(raw_text)
    };
    if draft.items.is_empty() {
        let global = resolve_global_declaration_strict(&draft.declarations)?;
        return Err(if global.is_some() {
            missing_capture_item_error()
        } else {
            missing_text_error()
        }
        .into());
    }
    let options = options.with_global(!draft.declarations.is_empty());

    let mut item_outcomes = Vec::new();
    for item in &draft.items {
        let outcomes = parse_capture_item_members(
            item,
            forced_route,
            forced_section,
            &options,
        )
        .map_err(|error| capture_item_error_at(item, error))?;
        item_outcomes.extend(outcomes);
    }
    // A second `- ^^` anywhere in the draft names the first line, across
    // items. Per-item duplicates already refused above.
    {
        let mut first_choice_line: Option<usize> = None;
        for outcome in &item_outcomes {
            for sub_bullet in &outcome.parsed.sub_bullets {
                let Some(choice) = sub_bullet.move_choice.as_ref() else {
                    continue;
                };
                let line = raw_text[..choice.range.0.min(raw_text.len())]
                    .matches('\n')
                    .count()
                    + 1;
                if let Some(first) = first_choice_line {
                    let inner =
                        super::move_bullet::move_choice_duplicate_error(first);
                    return Err(CaptureItemParseError::coded(
                        "move_choice_duplicate",
                        format!(
                            "capture item {} starting on line {}: {}",
                            outcome.index + 1,
                            outcome.line_start,
                            inner,
                        ),
                    ));
                }
                first_choice_line = Some(line);
            }
        }
    }

    let mut declarations = draft.declarations;
    let mut seen_declarations = std::collections::HashSet::new();
    for outcome in &item_outcomes {
        for declaration in &outcome.declarations {
            let span = (declaration.token.start, declaration.token.end);
            if seen_declarations.insert(span) {
                declarations.push(*declaration);
            }
        }
    }
    let global = resolve_global_declaration_strict(&declarations)?;
    // A plain `@@route` upgrades default (routeless) Refs to the global
    // parent; explicit local `@route` and forced `-r` keep local precedence.
    if let Some(global_ref) = &global {
        for outcome in &mut item_outcomes {
            if matches!(outcome.parsed.kind, CaptureKind::Ref(_))
                && outcome.parsed.route.is_none()
            {
                outcome.parsed.route = Some(global_ref.route.clone());
            }
        }
    }
    let mut warnings = capture_shadow_warnings(&item_outcomes);
    let mut seen_warnings = std::collections::HashSet::new();
    warnings.retain(|warning| seen_warnings.insert(warning.clone()));

    check_list_duplicate_ids(&item_outcomes)?;
    let mut items = Vec::with_capacity(item_outcomes.len());
    for mut outcome in item_outcomes.drain(..) {
        if forced_route.is_none()
            && let Some(global) = &global
        {
            let inheritable = outcome.parsed.route.is_none()
                && matches!(outcome.parsed.kind, CaptureKind::Task);
            inherit_global_destination(&mut outcome.parsed, global);
            if let Some(error) =
                super::type_marker::not_applicable_error(&outcome.parsed)
            {
                return Err(CaptureItemParseError {
                    code: Some("type_marker_not_applicable"),
                    message: format!(
                        "capture item {} starting on line {}: {error}",
                        outcome.index + 1,
                        outcome.line_start
                    ),
                    item: None,
                    span: None,
                });
            }
            finish_item_dependency_target(&mut outcome.parsed, inheritable)
                .map_err(|message| {
                    format!(
                        "capture item {} starting on line {}: {message}",
                        outcome.index + 1,
                        outcome.line_start
                    )
                })?;
        } else {
            finish_item_dependency_target(&mut outcome.parsed, false).map_err(
                |message| {
                    format!(
                        "capture item {} starting on line {}: {message}",
                        outcome.index + 1,
                        outcome.line_start
                    )
                },
            )?;
        }
        let ref_source = if matches!(outcome.parsed.kind, CaptureKind::Ref(_)) {
            if forced_route.is_some()
                || outcome.local_destination_marker.is_some()
            {
                Some(crate::native::capture_language::RefParentSource::Explicit)
            } else if outcome.parsed.route.is_some() {
                Some(crate::native::capture_language::RefParentSource::Global)
            } else {
                Some(crate::native::capture_language::RefParentSource::Default)
            }
        } else {
            None
        };
        let list = outcome.list.clone();
        items.push(ParsedCaptureItem {
            index: outcome.index,
            start: outcome.start,
            end: outcome.end,
            line_start: outcome.line_start,
            line_end: outcome.line_end,
            parsed: outcome.parsed,
            list,
            ref_source,
            template_row: None,
            fanout: outcome.fanout,
        });
    }
    // Past stacks: maximal runs of consecutive Past items, across same-line
    // chains and blank-line boundaries. Every other item interrupts runs.
    // Invalid items already returned errors above, so every Past here is
    // valid; still, only Past kinds join runs. Pin diagnostics are usage
    // errors on the offending item.
    {
        let members: Vec<super::past::PastStackMember<'_>> = items
            .iter()
            .map(|it| match &it.parsed.kind {
                CaptureKind::PomodoroPast { spec } => {
                    super::past::PastStackMember {
                        blocks: Some(spec.blocks),
                        edge: spec.segment.as_ref().map(|s| s.edge),
                        segment_raw: spec
                            .segment
                            .as_ref()
                            .map(|s| s.raw.as_str()),
                        raw_token: spec.raw.as_str(),
                    }
                }
                _ => super::past::PastStackMember {
                    blocks: None,
                    edge: None,
                    segment_raw: None,
                    raw_token: "",
                },
            })
            .collect();
        let (stacks, diagnostics) = super::past::compute_past_stacks(&members);
        for (it, st) in items.iter_mut().zip(stacks) {
            if let CaptureKind::PomodoroPast { spec } = &mut it.parsed.kind {
                spec.stack = st;
            }
        }
        if let Some((idx, message)) = diagnostics.into_iter().next() {
            let item = &items[idx];
            return Err(CaptureItemParseError::coded(
                "invalid_pomodoro_past",
                format!(
                    "capture item {} starting on line {}: {}",
                    item.index + 1,
                    item.line_start,
                    message
                ),
            ));
        }
    }

    Ok(ParsedCaptureDraft {
        global,
        items,
        warnings,
        template: None,
    })
}

fn capture_item_error_at(
    item: &CaptureItem<'_>,
    error: CaptureItemParseError,
) -> CaptureItemParseError {
    CaptureItemParseError {
        code: error.code,
        message: format!(
            "capture item {} starting on line {}: {}",
            item.index + 1,
            item.line_start,
            error.message
        ),
        item: error.span.map(|_| item.index + 1),
        span: error.span,
    }
}

/// Parse a selected run by reusing the ordinary singleton parser for each
/// byte-preserving member view. Every emitted item retains the source item's
/// identity and receives its own fanout context.
fn parse_capture_item_members<'a>(
    item: &CaptureItem<'a>,
    forced_route: Option<&str>,
    forced_section: Option<&str>,
    options: &CaptureParseOptions<'_>,
) -> Result<Vec<ParsedCaptureItemOutcome<'a>>, CaptureItemParseError> {
    if let Some(outcomes) =
        parse_transfer_claim(item, forced_route, forced_section, options)?
    {
        return Ok(outcomes);
    }
    if let Some(outcomes) =
        parse_merge_claim(item, forced_route, forced_section, options)?
    {
        return Ok(outcomes);
    }
    parse_capture_item_members_legacy(
        item,
        forced_route,
        forced_section,
        options,
    )
}

/// Claim a transfer directive before target-run fan-out. Returns `None`
/// when the shared scanner leaves the parent line unclaimed (or a forced
/// destination keeps the line literal), so callers continue with ordinary
/// item parsing.
fn parse_transfer_claim<'a>(
    item: &CaptureItem<'a>,
    forced_route: Option<&str>,
    forced_section: Option<&str>,
    options: &CaptureParseOptions<'_>,
) -> Result<Option<Vec<ParsedCaptureItemOutcome<'a>>>, CaptureItemParseError> {
    if forced_route.is_none()
        && forced_section.is_none()
        && !options.explicit_destination
    {
        let Some(parent) = item.lines.first().map(|line| line.raw) else {
            return Err(missing_text_error().into());
        };
        let scan = super::transfer::scan_transfer_line(
            &parent,
            item.source,
            item.lines.len() > 1,
        );
        if !matches!(&scan, super::transfer::TransferScan::Unclaimed) {
            if let Some(list) = item.list.as_ref() {
                let header = list
                    .section
                    .as_deref()
                    .map(|section| format!("@{}#{section}", list.route))
                    .unwrap_or_else(|| format!("@{}", list.route));
                let marker = parent.text.trim();
                let suggestion = format!(
                    "give it its own item (a blank line, then `{marker}`)"
                );
                return Err(CaptureItemParseError::coded(
                    "list_bullet_marker",
                    super::project_tasks::list_bullet_marker_error(
                        &header,
                        &format!("{}.md", list.route),
                        marker,
                        &suggestion,
                    ),
                ));
            }
            match scan {
                super::transfer::TransferScan::Unclaimed => unreachable!(),
                super::transfer::TransferScan::Escaped {
                    backslash_span,
                    ..
                } => {
                    let mut escaped = item.source.to_string();
                    escaped
                        .replace_range(backslash_span.0..backslash_span.1, " ");
                    let view = capture_item_source_view(item, &escaped);
                    let outcomes = parse_capture_item_members_legacy(
                        &view,
                        forced_route,
                        forced_section,
                        options,
                    )?;
                    return Ok(Some(
                        outcomes
                            .into_iter()
                            .map(|outcome| {
                                rebind_capture_item_declarations(
                                    outcome,
                                    item.source,
                                )
                            })
                            .collect(),
                    ));
                }
                super::transfer::TransferScan::Incomplete {
                    need,
                    message,
                    ..
                } => {
                    let code = match need {
                        super::transfer::TransferNeed::Destination => {
                            "incomplete_transfer"
                        }
                        super::transfer::TransferNeed::Task => {
                            "incomplete_transfer_task"
                        }
                        super::transfer::TransferNeed::BlockId => {
                            "incomplete_transfer_block_id"
                        }
                        super::transfer::TransferNeed::Drawer => {
                            "incomplete_transfer_drawer"
                        }
                        super::transfer::TransferNeed::Section => {
                            "incomplete_transfer_section"
                        }
                    };
                    return Err(CaptureItemParseError::coded(code, message));
                }
                super::transfer::TransferScan::Invalid {
                    code,
                    message,
                    ..
                } => {
                    return Err(CaptureItemParseError::coded(code, message));
                }
                super::transfer::TransferScan::Valid {
                    sources,
                    arrow,
                    destination,
                } => {
                    let route = destination.route.clone();
                    let parsed = ParsedCaptureText {
                        body: String::new(),
                        clip: None,
                        route: Some(route),
                        kind: CaptureKind::Transfer {
                            sources,
                            arrow,
                            destination,
                        },
                        scheduled_offset: None,
                        priority_level: None,
                        sub_bullets: Vec::new(),
                        dependencies: Vec::new(),
                        blocked_tasks: Vec::new(),
                        relations: Vec::new(),
                        dependency_target: None,
                        type_marker: None,
                        merge_in: None,
                    };
                    return Ok(Some(vec![parsed_capture_item_outcome(
                        item,
                        parsed,
                        Vec::new(),
                        None,
                    )]));
                }
            }
        }
    }
    Ok(None)
}

/// Claim a merge-in directive before target-run fan-out. Returns `None`
/// when the shared scanner leaves the parent line unclaimed (or a forced
/// destination keeps the line literal), so callers continue with ordinary
/// item parsing.
fn parse_merge_claim<'a>(
    item: &CaptureItem<'a>,
    forced_route: Option<&str>,
    forced_section: Option<&str>,
    options: &CaptureParseOptions<'_>,
) -> Result<Option<Vec<ParsedCaptureItemOutcome<'a>>>, CaptureItemParseError> {
    if forced_route.is_none()
        && forced_section.is_none()
        && !options.explicit_destination
    {
        let Some(parent) = item.lines.first().map(|line| line.raw) else {
            return Err(missing_text_error().into());
        };
        let scan = super::merge_in::scan_merge_in_line(
            &parent,
            item.source,
            item.lines.len() > 1,
        );
        match scan {
            super::merge_in::MergeInScan::Unclaimed => return Ok(None),
            super::merge_in::MergeInScan::Escaped {
                backslash_span,
                arrow_span,
            } => {
                let mut masked = item.source.as_bytes().to_vec();
                let from = backslash_span.0.min(masked.len());
                let arrow_start = arrow_span.0.min(masked.len());
                // Mask the escape backslash plus the entire tail with spaces
                // so the prefix parses alone; offsets are preserved.
                for byte in masked.iter_mut().skip(from) {
                    *byte = b' ';
                }
                let masked_text = String::from_utf8(masked)
                    .unwrap_or_else(|_| item.source.to_string());
                let view = capture_item_source_view(item, &masked_text);
                let outcomes = parse_capture_item_members_legacy(
                    &view,
                    forced_route,
                    forced_section,
                    options,
                )?;
                // Restore the literal tail (`< ...` without the backslash)
                // into the body in place.
                let tail_literal = item
                    .source
                    .get(arrow_start..parent.end)
                    .unwrap_or("")
                    .trim_start()
                    .to_string();
                let mut rebound = Vec::new();
                for outcome in outcomes {
                    let mut parsed = outcome.parsed;
                    if !tail_literal.is_empty() {
                        if parsed.body.trim().is_empty() {
                            parsed.body = tail_literal.clone();
                        } else {
                            parsed.body = format!(
                                "{} {}",
                                parsed.body.trim_end(),
                                tail_literal
                            );
                        }
                    }
                    let mut fresh = rebind_capture_item_declarations(
                        ParsedCaptureItemOutcome {
                            index: outcome.index,
                            start: outcome.start,
                            end: outcome.end,
                            line_start: outcome.line_start,
                            line_end: outcome.line_end,
                            parsed,
                            declarations: outcome.declarations,
                            local_destination_marker: outcome
                                .local_destination_marker,
                            list: outcome.list,
                            fanout: outcome.fanout,
                        },
                        item.source,
                    );
                    // The escaped tail never routes or fans out.
                    fresh.parsed.merge_in = None;
                    rebound.push(fresh);
                }
                return Ok(Some(rebound));
            }
            super::merge_in::MergeInScan::Incomplete { message, .. } => {
                return Err(CaptureItemParseError::coded(
                    super::merge_in::INCOMPLETE_MERGE_IN_CODE,
                    message,
                ));
            }
            super::merge_in::MergeInScan::Invalid { code, message, .. } => {
                return Err(CaptureItemParseError::coded(code, message));
            }
            super::merge_in::MergeInScan::Valid { sources, arrow } => {
                return parse_merge_valid(
                    item,
                    forced_route,
                    forced_section,
                    options,
                    parent,
                    sources,
                    arrow,
                );
            }
        }
    }
    Ok(None)
}

fn masked_source_for_merge(source: &str, from: usize, to: usize) -> String {
    let mut bytes = source.as_bytes().to_vec();
    let end = to.min(bytes.len());
    let start = from.min(end);
    for byte in bytes.iter_mut().skip(start).take(end - start) {
        *byte = b' ';
    }
    String::from_utf8(bytes).unwrap_or_else(|_| source.to_string())
}

fn parse_merge_valid<'a>(
    item: &CaptureItem<'a>,
    forced_route: Option<&str>,
    forced_section: Option<&str>,
    options: &CaptureParseOptions<'_>,
    parent: RawLine<'_>,
    sources: Vec<super::merge_in::MergeInSource>,
    arrow: super::merge_in::MergeInArrow,
) -> Result<Option<Vec<ParsedCaptureItemOutcome<'a>>>, CaptureItemParseError> {
    // An empty left side names no new task.
    let left_prefix_len = arrow.start.saturating_sub(parent.start);
    if parent
        .text
        .get(..left_prefix_len)
        .unwrap_or("")
        .trim()
        .is_empty()
    {
        return Err(CaptureItemParseError::coded(
            super::merge_in::MERGE_IN_NEEDS_NEW_TASK_CODE,
            merge_needs_new_task_message(&parent, &sources, MergeNeeds::Empty),
        ));
    }
    // Mask the clause bytes (arrow through end of parent line) and parse
    // the left side with the ordinary parser. Scope the masked view so the
    // local borrow ends before the rebound outcome leaves this function.
    let mut outcome = {
        let masked =
            masked_source_for_merge(item.source, arrow.start, parent.end);
        let view = capture_item_source_view(item, &masked);
        let left_outcomes = parse_capture_item_members_legacy(
            &view,
            forced_route,
            forced_section,
            options,
        )?;
        if left_outcomes.len() != 1 {
            return Err(CaptureItemParseError::coded(
                super::merge_in::MERGE_IN_NEEDS_NEW_TASK_CODE,
                merge_needs_new_task_message(
                    &parent,
                    &sources,
                    MergeNeeds::TargetRun,
                ),
            ));
        }
        let outcome = left_outcomes.into_iter().next().expect("one left");
        // Rebind to original offsets before context checks so spans index
        // the exact draft.
        rebind_capture_item_declarations(outcome, item.source)
    };
    // Lexical `&`/`*`/`~`/`~~` source matches refuse before kind checks.
    if let Some((message, span)) =
        merge_source_also_marked_message(&outcome.parsed, &sources)
    {
        let mut error = CaptureItemParseError::coded(
            super::merge_in::MERGE_IN_SOURCE_ALSO_MARKED_CODE,
            message,
        );
        error.span = Some(span);
        return Err(error);
    }
    let needs = classify_merge_left(&outcome.parsed);
    match needs {
        MergeNeeds::Allowed => {
            let clause = super::model::MergeInClause {
                arrow: (arrow.start, arrow.end),
                sources: sources
                    .into_iter()
                    .map(|source| super::model::MergeInSource {
                        marker: source.marker,
                        route: source.route,
                        block_id: source.block_id,
                        start: source.start,
                        end: source.end,
                    })
                    .collect(),
            };
            outcome.parsed.merge_in = Some(clause);
            Ok(Some(vec![outcome]))
        }
        _ => Err(CaptureItemParseError::coded(
            super::merge_in::MERGE_IN_NEEDS_NEW_TASK_CODE,
            merge_needs_new_task_message(&parent, &sources, needs),
        )),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MergeNeeds {
    Allowed,
    Empty,
    StartClose,
    Project,
    Existing,
    NoTask,
    TargetRun,
}

fn classify_merge_left(parsed: &ParsedCaptureText) -> MergeNeeds {
    // `parsed` comes from the masked view (the clause is blanked), so an
    // empty body with no route, clip, or markers means nothing precedes `<`.
    let is_empty = parsed.body.trim().is_empty()
        && parsed.route.is_none()
        && parsed.clip.is_none()
        && parsed.sub_bullets.is_empty()
        && parsed.dependencies.is_empty()
        && parsed.blocked_tasks.is_empty()
        && parsed.relations.is_empty()
        && parsed.type_marker.is_none()
        && matches!(
            parsed.kind,
            CaptureKind::Task | CaptureKind::Bullet { .. }
        );
    if is_empty {
        return MergeNeeds::Empty;
    }
    match &parsed.kind {
        CaptureKind::Task | CaptureKind::TaskWithBlockId { .. } => {
            MergeNeeds::Allowed
        }
        CaptureKind::Pomodoro { start, close, .. } => {
            if start.is_none() && close.is_none() {
                MergeNeeds::Allowed
            } else {
                MergeNeeds::StartClose
            }
        }
        CaptureKind::ProjectNote { .. } => MergeNeeds::Project,
        CaptureKind::PomodoroLink { .. }
        | CaptureKind::TaskToggle { .. }
        | CaptureKind::TaskComplete { .. }
        | CaptureKind::SubBullet { .. } => MergeNeeds::Existing,
        CaptureKind::Transfer { .. } => MergeNeeds::Existing,
        CaptureKind::PomodoroAdjust { .. }
        | CaptureKind::PomodoroShift { .. }
        | CaptureKind::PomodoroSplit { .. }
        | CaptureKind::PomodoroMerge { .. }
        | CaptureKind::PomodoroClose { .. }
        | CaptureKind::PomodoroPast { .. }
        | CaptureKind::PomodoroStart { .. } => MergeNeeds::NoTask,
        CaptureKind::Ref(_)
        | CaptureKind::Bullet { .. }
        | CaptureKind::PomodoroNote
        | CaptureKind::NotePull { .. } => MergeNeeds::NoTask,
    }
}

fn merge_needs_new_task_message(
    parent: &RawLine<'_>,
    sources: &[super::merge_in::MergeInSource],
    needs: MergeNeeds,
) -> String {
    const PREFIX: &str = "< merges into the task this line creates";
    match needs {
        MergeNeeds::Empty => {
            format!("{PREFIX}; name the new task before <")
        }
        MergeNeeds::StartClose => {
            format!(
                "{PREFIX}; the new task must stay open to take in other tasks; start or close it in a later item"
            )
        }
        MergeNeeds::Project => {
            format!(
                "{PREFIX}; a new project keeps tasks as tasks: list them as - ^id bullets to move them in"
            )
        }
        MergeNeeds::Existing | MergeNeeds::TargetRun => {
            // Offer `Use >` only for the one-destination-token left side.
            let left = parent_left_trimmed(parent);
            if super::transfer::is_single_destination_token(&left) {
                let sources_text = sources
                    .iter()
                    .map(|source| source.marker.as_str())
                    .collect::<Vec<_>>()
                    .join(" ");
                format!("{PREFIX}; {left} already exists. Write {sources_text} > {left}")
            } else {
                format!("{PREFIX}; this line names an existing task")
            }
        }
        MergeNeeds::NoTask => {
            format!("{PREFIX}; this line creates no task")
        }
        MergeNeeds::Allowed => unreachable!(),
    }
}

fn parent_left_trimmed(parent: &RawLine<'_>) -> String {
    // Left side up to the first `<` claim position; the scanner already
    // validated it, so split at the first standalone `<`.
    let mut left_end = parent.text.len();
    for (index, _) in parent.text.char_indices() {
        if parent.text[index..].starts_with('<') {
            // Check standalone: whitespace or start before, whitespace or
            // end after (or claimable following handles the rest).
            let before_ok = index == 0
                || parent.text[..index]
                    .chars()
                    .next_back()
                    .is_some_and(|character| character.is_whitespace());
            if before_ok {
                left_end = index;
                break;
            }
        }
    }
    parent.text[..left_end].trim().to_string()
}

fn merge_source_also_marked_message(
    parsed: &ParsedCaptureText,
    sources: &[super::merge_in::MergeInSource],
) -> Option<(String, (usize, usize))> {
    let mut targets: Vec<(
        String,
        String,
        &'static str,
        Option<(usize, usize)>,
    )> = Vec::new();
    for dependency in &parsed.dependencies {
        targets.push((
            dependency.note.to_ascii_lowercase(),
            dependency.block_id.clone(),
            "prerequisite",
            dependency.span,
        ));
    }
    for blocked in &parsed.blocked_tasks {
        targets.push((
            blocked.note.to_ascii_lowercase(),
            blocked.block_id.clone(),
            "dependent",
            blocked.span,
        ));
    }
    for relation in &parsed.relations {
        let role = match relation.direction {
            super::editor_model::RelationDirection::To => "relation",
            super::editor_model::RelationDirection::From => "relation",
        };
        targets.push((
            relation.note.to_ascii_lowercase(),
            relation.block_id.clone(),
            role,
            relation.span,
        ));
    }
    for source in sources {
        let route = source.route.to_ascii_lowercase();
        if let Some((_, _, _, span)) =
            targets.iter().find(|(note, block_id, _, _)| {
                *note == route && *block_id == source.block_id
            })
        {
            return Some((
                "that source merges into the new task; it can't also be its prerequisite/dependent/relation"
                    .to_string(),
                span.unwrap_or((source.start, source.end)),
            ));
        }
    }
    None
}

fn parse_capture_item_members_legacy<'a>(
    item: &CaptureItem<'a>,
    forced_route: Option<&str>,
    forced_section: Option<&str>,
    options: &CaptureParseOptions<'_>,
) -> Result<Vec<ParsedCaptureItemOutcome<'a>>, CaptureItemParseError> {
    let Some(run) =
        select_target_run_for_item(item, forced_route, forced_section)?
    else {
        return Ok(vec![parse_capture_item(
            item,
            forced_route,
            forced_section,
            options,
            None,
        )?]);
    };
    let count = run.selection.members.len();
    let mut outcomes = Vec::with_capacity(count);
    for (member_index, member) in run.selection.members.iter().enumerate() {
        let masked_source =
            super::target_runs::masked_source_for_member_with_terminals(
                item.source,
                &run.selection.members,
                member_index,
                &run.terminal_spans,
            );
        let view = capture_item_source_view(item, &masked_source);
        let parsed = parse_capture_item(
            &view,
            forced_route,
            forced_section,
            options,
            Some(&run),
        )?;
        let mut outcome = rebind_capture_item_declarations(parsed, item.source);
        outcome.fanout = Some(FanoutContext {
            line: run.line,
            position: member_index + 1,
            count,
            marker: member.marker_text.clone(),
        });
        outcomes.push(outcome);
    }
    Ok(outcomes)
}

fn capture_item_source_view<'a>(
    item: &CaptureItem<'_>,
    source: &'a str,
) -> CaptureItem<'a> {
    let lines = item
        .lines
        .iter()
        .map(|line| ItemLine {
            raw: RawLine {
                text: &source[line.raw.start..line.raw.end],
                start: line.raw.start,
                end: line.raw.end,
            },
            line_number: line.line_number,
        })
        .collect();
    CaptureItem {
        source,
        index: item.index,
        start: item.start,
        end: item.end,
        line_start: item.line_start,
        line_end: item.line_end,
        lines,
        list: item.list.clone(),
    }
}

fn rebind_capture_item_declarations<'a>(
    outcome: ParsedCaptureItemOutcome<'_>,
    source: &'a str,
) -> ParsedCaptureItemOutcome<'a> {
    let declarations = outcome
        .declarations
        .into_iter()
        .map(|declaration| {
            let start = declaration.token.start;
            let end = declaration.token.end;
            GlobalDeclarationToken {
                token: Token {
                    text: source.get(start..end).unwrap_or(""),
                    start,
                    end,
                },
                line_number: declaration.line_number,
            }
        })
        .collect();
    ParsedCaptureItemOutcome {
        index: outcome.index,
        start: outcome.start,
        end: outcome.end,
        line_start: outcome.line_start,
        line_end: outcome.line_end,
        parsed: outcome.parsed,
        declarations,
        local_destination_marker: outcome.local_destination_marker,
        list: outcome.list,
        fanout: outcome.fanout,
    }
}

/// Refuse two equal task IDs inside one capture list. Ordinary items keep
/// their plan-time duplicate handling against staged content; list bullets
/// fail up front so one bad bullet cannot depend on parse order.
fn check_list_duplicate_ids(
    outcomes: &[ParsedCaptureItemOutcome<'_>],
) -> Result<(), CaptureItemParseError> {
    let mut seen: Vec<(usize, usize, String, usize, usize)> = Vec::new();
    for outcome in outcomes {
        let Some(list) = outcome
            .list
            .as_ref()
            .filter(|list| !list.ownerless && list.kind == ListKind::Tasks)
        else {
            continue;
        };
        let block_id = match &outcome.parsed.kind {
            CaptureKind::Pomodoro { block_id, .. } => Some(block_id),
            CaptureKind::TaskWithBlockId { block_id } => Some(block_id),
            _ => None,
        };
        let Some(block_id) = block_id else {
            continue;
        };
        if let Some((_, _, _, first_line, _)) = seen.iter().find(|seen| {
            seen.0 == list.header_start
                && seen.1 == list.header_end
                && seen.2 == *block_id
        }) {
            let first_line = *first_line;
            return Err(CaptureItemParseError::coded(
                "duplicate_project_task_id",
                format!(
                    "capture item {} starting on line {}: {}",
                    outcome.index + 1,
                    outcome.line_start,
                    super::project_tasks::duplicate_project_task_id_error(
                        block_id,
                        first_line,
                        outcome.line_start,
                    )
                ),
            ));
        }
        seen.push((
            list.header_start,
            list.header_end,
            block_id.clone(),
            outcome.line_start,
            outcome.index,
        ));
    }
    Ok(())
}

fn parse_template_expansion(
    expansion: super::template::TemplateExpansion,
    forced_route: Option<&str>,
    forced_section: Option<&str>,
    options: &CaptureParseOptions<'_>,
) -> Result<ParsedCaptureDraft, TemplateCodedError> {
    let instance_options = template_instance_options(options);
    let mut items = Vec::with_capacity(expansion.instances.len());

    for instance in &expansion.instances {
        let parsed_items = parse_single_template_instance(
            instance,
            forced_route,
            forced_section,
            &instance_options,
        )?;
        for (outcome_parsed, ref_source, fanout) in parsed_items {
            items.push(ParsedCaptureItem {
                index: instance.row - 1,
                start: instance.row_range.start,
                end: instance.row_range.end,
                line_start: instance.line,
                line_end: instance.line,
                parsed: outcome_parsed,
                list: None,
                ref_source,
                template_row: Some(TemplateRowSummary {
                    row: instance.row,
                    line: instance.line,
                }),
                fanout,
            });
        }
    }

    Ok(ParsedCaptureDraft {
        global: None,
        items,
        warnings: Vec::new(),
        template: Some(expansion.summary),
    })
}

fn template_instance_options<'a>(
    options: &CaptureParseOptions<'a>,
) -> CaptureParseOptions<'a> {
    CaptureParseOptions {
        has_global_destination: false,
        ..*options
    }
}

fn parse_single_template_instance(
    instance: &super::template::TemplateInstance,
    forced_route: Option<&str>,
    forced_section: Option<&str>,
    instance_options: &CaptureParseOptions<'_>,
) -> Result<
    Vec<(
        ParsedCaptureText,
        Option<RefParentSource>,
        Option<FanoutContext>,
    )>,
    TemplateCodedError,
> {
    let lines = split_physical_lines(&instance.text)
        .into_iter()
        .enumerate()
        .map(|(index, raw)| ItemLine {
            raw,
            line_number: instance.template_line_start + index,
        })
        .collect::<Vec<_>>();
    // Template rows are never split by headers.
    let draft_items =
        split_items_from_item_lines(&lines, &instance.text, false);
    if draft_items
        .first()
        .and_then(|item| item.lines.first())
        .and_then(|line| detect_list_header(&line.raw))
        .is_some()
    {
        return Err(TemplateCodedError::coded(
            "template_list",
            template_row_error(
                instance.row,
                instance.line,
                template_list_error(),
            ),
        ));
    }
    if draft_items.len() != 1 {
        return Err(TemplateCodedError::coded(
            "template_session_operator",
            template_session_error(instance.row, instance.line),
        ));
    }
    let outcomes = parse_capture_item_members(
        &draft_items[0],
        forced_route,
        forced_section,
        instance_options,
    )
    .map_err(|error| TemplateCodedError {
        code: error.code,
        message: template_row_error(instance.row, instance.line, error.message),
        // Expanded template bytes do not map to a contiguous range in the
        // user's template draft.
        item: None,
        span: None,
    })?;
    let mut parsed_items = Vec::with_capacity(outcomes.len());
    for mut outcome in outcomes {
        if outcome.parsed.clip.is_some() {
            return Err(TemplateCodedError::coded(
                "template_clipboard",
                template_clipboard_error(instance.row, instance.line),
            ));
        }
        if is_template_session_kind(&outcome.parsed.kind) {
            return Err(TemplateCodedError::coded(
                "template_session_operator",
                template_session_error(instance.row, instance.line),
            ));
        }
        finish_item_dependency_target(&mut outcome.parsed, false).map_err(
            |message| {
                TemplateCodedError::generic(template_row_error(
                    instance.row,
                    instance.line,
                    message,
                ))
            },
        )?;

        let ref_source = if matches!(outcome.parsed.kind, CaptureKind::Ref(_)) {
            if forced_route.is_some()
                || outcome.local_destination_marker.is_some()
            {
                Some(RefParentSource::Explicit)
            } else if outcome.parsed.route.is_some() {
                Some(RefParentSource::Global)
            } else {
                Some(RefParentSource::Default)
            }
        } else {
            None
        };
        let fanout = outcome.fanout.map(|mut fanout| {
            // A template item is one authored source row regardless of
            // where its expanded marker landed.
            fanout.line = instance.line;
            fanout
        });
        parsed_items.push((outcome.parsed, ref_source, fanout));
    }
    Ok(parsed_items)
}

fn first_leading_execution_error(
    raw_text: &str,
    analysis: &super::template::TemplateAnalysis,
    upto_row: usize,
    forced_route: Option<&str>,
    forced_section: Option<&str>,
    options: &CaptureParseOptions<'_>,
) -> Option<TemplateCodedError> {
    let instance_options = template_instance_options(options);
    for instance in
        super::template::leading_instances(raw_text, analysis, upto_row)
    {
        if let Err(error) = parse_single_template_instance(
            &instance,
            forced_route,
            forced_section,
            &instance_options,
        ) {
            return Some(error);
        }
    }
    None
}

fn template_row_error(row: usize, line: usize, message: String) -> String {
    format!("template row {row} (line {line}): {message}")
}

fn template_clipboard_error(row: usize, line: usize) -> String {
    template_row_error(
        row,
        line,
        "a template can't capture the clipboard: `%c` would repeat once per row"
            .to_string(),
    )
}

fn template_session_error(row: usize, line: usize) -> String {
    template_row_error(
        row,
        line,
        "a template can't drive the Pomodoro session; capture `=`, `x`, `+N`, and `++N` on their own"
            .to_string(),
    )
}

/// A list header has no meaning in a `%t` template: the body is a
/// definition and rows are never split by headers.
pub(crate) fn template_list_error() -> String {
    "a list header cannot appear in a %t template body; capture the list outside the template"
        .to_string()
}

fn is_template_session_kind(kind: &CaptureKind) -> bool {
    match kind {
        CaptureKind::PomodoroAdjust { .. }
        | CaptureKind::PomodoroShift { .. }
        | CaptureKind::PomodoroSplit { .. }
        | CaptureKind::PomodoroMerge { .. }
        | CaptureKind::PomodoroStart { .. }
        | CaptureKind::PomodoroPast { .. }
        | CaptureKind::PomodoroClose { .. } => true,
        CaptureKind::Pomodoro { start, close, .. }
        | CaptureKind::PomodoroLink { start, close, .. } => {
            start.is_some() || close.is_some()
        }
        _ => false,
    }
}

pub(super) fn resolve_global_declaration_strict(
    declarations: &[GlobalDeclarationToken<'_>],
) -> Result<Option<ParsedGlobalDestination>, String> {
    let Some(first) = declarations.first() else {
        return Ok(None);
    };
    if let Some(second) = declarations.get(1) {
        return Err(duplicate_global_destination_error(
            first.line_number,
            second.line_number,
        ));
    }
    parse_global_destination_token(
        first.token.text,
        first.token.start,
        first.token.end,
        first.line_number,
    )
    .map(Some)
}

pub(super) fn duplicate_global_destination_error(
    first_line: usize,
    second_line: usize,
) -> String {
    format!(
        "duplicate global destination declaration on line {second_line}; first declaration is on line {first_line}"
    )
}

pub(super) fn capture_shadow_warnings(
    item_outcomes: &[ParsedCaptureItemOutcome<'_>],
) -> Vec<String> {
    let mut warnings = Vec::new();
    for outcome in item_outcomes {
        let Some(local_marker) = outcome.local_destination_marker.as_deref()
        else {
            continue;
        };
        for declaration in &outcome.declarations {
            warnings.push(global_destination_shadowed_warning(
                local_marker,
                declaration.token.text,
            ));
        }
    }
    warnings
}

pub(super) fn global_destination_shadowed_warning(
    local_marker: &str,
    declaration: &str,
) -> String {
    format!(
        "this item's {local_marker} marker overrides the {declaration} destination it declares; move {declaration} to an item without a local marker, or delete {local_marker}"
    )
}

pub(super) fn parse_global_destination_token(
    token: &str,
    start: usize,
    end: usize,
    line: usize,
) -> Result<ParsedGlobalDestination, String> {
    let rest = token
        .strip_prefix("@@")
        .ok_or_else(|| GLOBAL_DESTINATION_SHAPE_ERROR.to_string())?;
    if rest.contains('+')
        || rest.contains('#')
        || rest.contains('^')
        || rest.contains(':')
    {
        return Err(unsupported_global_destination_error());
    }
    if rest.ends_with('!') {
        return Err(EXPLICIT_TOGGLE_GLOBAL_ERROR.to_string());
    }
    if rest.is_empty() {
        return Err(GLOBAL_DESTINATION_SHAPE_ERROR.to_string());
    }
    if !is_route_token(rest) {
        return Err(GLOBAL_DESTINATION_ROUTE_ERROR.to_string());
    }
    Ok(ParsedGlobalDestination {
        start,
        end,
        line,
        route: rest.to_ascii_lowercase(),
    })
}

pub(super) fn inherit_global_destination(
    parsed: &mut ParsedCaptureText,
    global: &ParsedGlobalDestination,
) {
    if parsed.route.is_some() || !matches!(parsed.kind, CaptureKind::Task) {
        return;
    }
    parsed.route = Some(global.route.clone());
}

pub(super) fn global_declarations_from_tokens<'a>(
    tokens: Vec<Token<'a>>,
    line_number: usize,
) -> Vec<GlobalDeclarationToken<'a>> {
    tokens
        .into_iter()
        .map(|token| GlobalDeclarationToken { token, line_number })
        .collect()
}

/// Remove every `@@...` token from `tokens`, returning them in source order.
pub(super) fn take_global_declarations<T: ParseToken>(
    tokens: &mut Vec<T>,
) -> Vec<T> {
    let mut declarations = Vec::new();
    let mut index = 0;
    while index < tokens.len() {
        if tokens[index].text().starts_with("@@") {
            declarations.push(tokens.remove(index));
        } else {
            index += 1;
        }
    }
    declarations
}
