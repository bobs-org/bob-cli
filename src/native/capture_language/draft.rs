//! Draft splitting, authored-line classification, and global declarations.

use super::editor_parse::*;
use super::item::*;
use super::line::*;
use super::markers::*;
use super::model::*;
use super::tokens::*;
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
/// session-operator chain (`+2 =x`) is split into one single-token item per
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
        items: split_items_from_item_lines(&item_lines, raw),
    }
}

/// Split blank-line-separated item lines into [`CaptureItem`]s. A parent
/// line that is a session-operator chain yields one single-token item per
/// operator; see [`push_capture_item`].
pub(super) fn split_items_from_item_lines<'a>(
    lines: &[ItemLine<'a>],
    source: &'a str,
) -> Vec<CaptureItem<'a>> {
    let mut items = Vec::new();
    let mut current: Vec<ItemLine<'_>> = Vec::new();

    for line in lines.iter().copied() {
        if line.raw.text.trim().is_empty() {
            push_capture_item(&mut items, &mut current, source);
            continue;
        }

        current.push(line);
    }

    push_capture_item(&mut items, &mut current, source);
    items
}

/// Return the line's whitespace-separated tokens when the parent line is
/// a session-operator chain: at least two tokens, every one passing
/// [`is_session_chain_token`]. A single token is never a chain, so the
/// existing single-token path stays byte-identical.
pub(super) fn session_chain_tokens<'a>(
    line: &RawLine<'a>,
) -> Option<Vec<Token<'a>>> {
    let tokens = tokenize_line_with_spans(line);
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

/// Whether a chain token is a whole-item `=x` close: any `=`-family close
/// token (`=x`, `=X`, `=x<N>…`, `=x#…`).
fn is_chain_close_token(token: &str) -> bool {
    matches!(
        super::item::session_equals_token(token),
        Some(super::item::EqualsToken::Close)
    )
}

/// Whether a token starts a new session: a whole-item start (`=`, `=<X>`,
/// `=#name`, `=~K` with its drop part) or a whole-item close (`=x…`).
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
/// lines attach to the line's `=x` item — or to the last `=x` when the line
/// closes twice — extending its `lines`, `end`, and `line_end`. With no `=x`
/// on the line, children still attach to the last token's item, so the last
/// token's family parser reports its existing shape error.
///
/// A parent line with an inline Work Log entry (`=x wired the lexer`)
/// splits into one item per lead token before the owner, then the owner
/// item carrying the entry, then one item per trail token. The owner item's
/// single [`ItemLine`] is the contiguous slice from the owner token's start
/// through the entry's last token, so the close parser sees exactly
/// `=x… <entry>`. Child lines attach to the owner, so mixing entry and
/// bullets reports the mixing error. With no siblings the result is one
/// item over the whole line, as today.
///
/// A chain whose `=x` is not last nests its item ranges: the close item's
/// range runs from its token through its last bullet, so it contains the
/// ranges of the later tokens on the parent line. An inline owner never
/// nests siblings: its range ends at the entry (or its last child). Item
/// ranges therefore nest or stay disjoint; they never partially overlap.
pub(super) fn push_capture_item<'a>(
    items: &mut Vec<CaptureItem<'a>>,
    current: &mut Vec<ItemLine<'a>>,
    source: &'a str,
) {
    let Some(first) = current.first().copied() else {
        return;
    };
    // A blank-line-free block of two or more URL-list lines splits into one
    // item per line. The test is purely lexical, so parse, preview, and
    // submit always agree; each item classifies on its own later.
    if current.len() >= 2
        && current.iter().all(|line| is_url_list_line(line.raw.text))
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
            });
        }
        return;
    }
    if let Some(tokens) = session_chain_tokens(&first.raw) {
        let rest: Vec<ItemLine<'a>> = current[1..].to_vec();
        let last_child = rest.last().copied();
        let owner = tokens
            .iter()
            .rposition(|token| is_chain_close_token(token.text))
            .unwrap_or(tokens.len() - 1);
        for (position, token) in tokens.iter().copied().enumerate() {
            let is_owner = position == owner;
            let token_line = ItemLine {
                raw: RawLine {
                    text: token.text,
                    start: token.start,
                    end: token.end,
                },
                line_number: first.line_number,
            };
            if is_owner {
                let mut lines = vec![token_line];
                lines.extend(rest.iter().copied());
                let (end, line_end) = match last_child {
                    Some(child) => (child.raw.end, child.line_number),
                    None => (token.end, first.line_number),
                };
                items.push(CaptureItem {
                    source,
                    index: items.len(),
                    start: token.start,
                    end,
                    line_start: first.line_number,
                    line_end,
                    lines,
                });
            } else {
                items.push(CaptureItem {
                    source,
                    index: items.len(),
                    start: token.start,
                    end: token.end,
                    line_start: first.line_number,
                    line_end: first.line_number,
                    lines: vec![token_line],
                });
            }
        }
        current.clear();
        return;
    }
    let parent_tokens: Vec<Token<'_>> = tokenize_with_spans(first.raw.text)
        .into_iter()
        .map(|token| Token {
            text: token.text,
            start: token.start + first.raw.start,
            end: token.end + first.raw.start,
        })
        .collect();
    if let Some(split) = split_inline_close_tokens(&parent_tokens) {
        let lead_before = &parent_tokens[..split.owner_index];
        let trail = &parent_tokens[split.trail_start..];
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
            });
            return;
        }
        let rest: Vec<ItemLine<'a>> = current[1..].to_vec();
        let last_child = rest.last().copied();
        for token in lead_before.iter().copied() {
            let token_line = ItemLine {
                raw: RawLine {
                    text: token.text,
                    start: token.start,
                    end: token.end,
                },
                line_number: first.line_number,
            };
            items.push(CaptureItem {
                source,
                index: items.len(),
                start: token.start,
                end: token.end,
                line_start: first.line_number,
                line_end: first.line_number,
                lines: vec![token_line],
            });
        }
        let owner = parent_tokens[split.owner_index];
        let entry_last = parent_tokens[split.trail_start - 1];
        let owner_rel_start = owner.start - first.raw.start;
        let entry_rel_end = entry_last.end - first.raw.start;
        let owner_text = &first.raw.text[owner_rel_start..entry_rel_end];
        let owner_line = ItemLine {
            raw: RawLine {
                text: owner_text,
                start: owner.start,
                end: entry_last.end,
            },
            line_number: first.line_number,
        };
        {
            let mut lines = vec![owner_line];
            lines.extend(rest.iter().copied());
            let (end, line_end) = match last_child {
                Some(child) => (child.raw.end, child.line_number),
                None => (entry_last.end, first.line_number),
            };
            items.push(CaptureItem {
                source,
                index: items.len(),
                start: owner.start,
                end,
                line_start: first.line_number,
                line_end,
                lines,
            });
        }
        for token in trail.iter().copied() {
            let token_line = ItemLine {
                raw: RawLine {
                    text: token.text,
                    start: token.start,
                    end: token.end,
                },
                line_number: first.line_number,
            };
            items.push(CaptureItem {
                source,
                index: items.len(),
                start: token.start,
                end: token.end,
                line_start: first.line_number,
                line_end: first.line_number,
                lines: vec![token_line],
            });
        }
        current.clear();
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
pub(crate) fn parse_capture_text_with_clip_control(
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

    let item_outcomes = draft
        .items
        .iter()
        .map(|item| {
            parse_capture_item(item, forced_route, forced_section, &options)
        })
        .collect::<Result<Vec<_>, _>>()?;
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
    if forced_route.is_none()
        && let Some(global) = &global
    {
        let inheritable = outcome.parsed.route.is_none()
            && matches!(outcome.parsed.kind, CaptureKind::Task);
        inherit_global_destination(&mut outcome.parsed, global);
        finish_item_dependency_target(&mut outcome.parsed, inheritable)?;
    } else {
        finish_item_dependency_target(&mut outcome.parsed, false)?;
    }
    Ok(outcome.parsed)
}

pub(crate) fn parse_capture_draft_with_clip_control(
    raw_text: &str,
    forced_route: Option<&str>,
    forced_section: Option<&str>,
    options: &CaptureParseOptions<'_>,
) -> Result<ParsedCaptureDraft, String> {
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

    let mut item_outcomes = draft
        .items
        .iter()
        .map(|item| {
            parse_capture_item(item, forced_route, forced_section, &options)
                .map_err(|message| {
                    format!(
                        "capture item {} starting on line {}: {message}",
                        item.index + 1,
                        item.line_start
                    )
                })
        })
        .collect::<Result<Vec<_>, _>>()?;

    let mut declarations = draft.declarations;
    for outcome in &item_outcomes {
        declarations.extend(outcome.declarations.iter().copied());
    }
    let global = resolve_global_declaration_strict(&declarations)?;
    let warnings = capture_shadow_warnings(&item_outcomes);

    let mut items = Vec::with_capacity(item_outcomes.len());
    for mut outcome in item_outcomes.drain(..) {
        if forced_route.is_none()
            && let Some(global) = &global
        {
            let inheritable = outcome.parsed.route.is_none()
                && matches!(outcome.parsed.kind, CaptureKind::Task);
            inherit_global_destination(&mut outcome.parsed, global);
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
        items.push(ParsedCaptureItem {
            index: outcome.index,
            start: outcome.start,
            end: outcome.end,
            line_start: outcome.line_start,
            line_end: outcome.line_end,
            parsed: outcome.parsed,
        });
    }

    Ok(ParsedCaptureDraft {
        global,
        items,
        warnings,
    })
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
    if rest.ends_with('!') {
        return Err(EXPLICIT_TOGGLE_GLOBAL_ERROR.to_string());
    }
    if rest.contains('#') || rest.contains('^') || rest.contains(':') {
        return Err(unsupported_global_destination_error(token));
    }
    match rest.split_once('+') {
        Some((route, block_id)) => {
            if route.is_empty() || !is_route_token(route) {
                return Err(if route.is_empty() {
                    GLOBAL_DESTINATION_SHAPE_ERROR.to_string()
                } else {
                    GLOBAL_DESTINATION_ROUTE_ERROR.to_string()
                });
            }
            if block_id.is_empty() {
                return Err(format!(
                    "global destination requires a block ID: @@<route>+<block-id> (run 'bob capture-tasks -r {}' to list task block IDs)",
                    route.to_ascii_lowercase()
                ));
            }
            if !is_block_id(block_id) {
                return Err(GLOBAL_DESTINATION_BLOCK_ID_ERROR.to_string());
            }
            Ok(ParsedGlobalDestination {
                start,
                end,
                line,
                route: route.to_ascii_lowercase(),
                block_id: Some(block_id.to_string()),
                kind: CaptureKind::SubBullet {
                    target: SubBulletTarget::BlockId(block_id.to_string()),
                    section: None,
                },
            })
        }
        None => {
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
                block_id: None,
                kind: CaptureKind::Task,
            })
        }
    }
}

pub(super) fn inherit_global_destination(
    parsed: &mut ParsedCaptureText,
    global: &ParsedGlobalDestination,
) {
    if parsed.route.is_some() || !matches!(parsed.kind, CaptureKind::Task) {
        return;
    }
    parsed.route = Some(global.route.clone());
    parsed.kind = global.kind.clone();
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
