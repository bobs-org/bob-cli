//! The capture grammar shared by `bob capture` and `bob capture-parse`.
//!
//! This module owns every position-agnostic classification rule for capture
//! text: draft-wide `@@` declarations, whitespace normalization, terminal
//! marker extraction, and `@token` routing. `capture.rs` layers execution
//! (files, clipboard, note mutation) on top of it, and `capture_parse.rs`
//! layers a span-aware, read-only editor view on the same functions. There
//! is exactly one grammar here; the editor path never re-implements token
//! classification.
//!
//! Fallible functions return `Result<T, String>` because this module has no
//! file I/O and therefore no use for `capture.rs`'s `CaptureError` kinds.
//! `capture.rs` wraps the returned message in `CaptureError::usage(...)`, so
//! the message text is the single source of truth for both callers.

use std::num::NonZeroUsize;

use serde::Serialize;

use super::{capture_clip, collect_done};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CaptureKind {
    Task,
    TaskWithBlockId {
        block_id: String,
    },
    Bullet {
        section_prefix: Option<String>,
        exact: bool,
    },
    Pomodoro {
        block_id: String,
    },
    SubBullet {
        target: SubBulletTarget,
        section: Option<TaskSectionSelector>,
    },
    PomodoroNote,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SubBulletTarget {
    BlockId(String),
    Ref { line: usize, digest: String },
}

/// Typed `@route+block-id#section` selector. A typed token is always
/// prefix-capable (`exact: false`); the forced picker option sets `exact`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TaskSectionSelector {
    pub(crate) text: String,
    pub(crate) exact: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ParsedCaptureText {
    pub(crate) body: String,
    pub(crate) clip: Option<ClipRequest>,
    pub(crate) route: Option<String>,
    pub(crate) kind: CaptureKind,
    pub(crate) scheduled_offset: Option<u64>,
    pub(crate) priority_level: Option<u64>,
    /// Normalized authored-child bodies plus their semantic depth, in source
    /// order, with their source marker and item-wide markers already removed.
    /// Empty when the item was an ordinary single-line capture.
    pub(crate) sub_bullets: Vec<AuthoredSubBullet>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AuthoredDepth {
    First,
    Nested,
}

impl AuthoredDepth {
    pub(crate) fn level(self) -> u8 {
        match self {
            Self::First => 1,
            Self::Nested => 2,
        }
    }

    pub(crate) fn indent_units(self) -> usize {
        usize::from(self.level())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AuthoredSubBullet {
    pub(crate) body: String,
    pub(crate) depth: AuthoredDepth,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct TerminalMarkers {
    pub(crate) clip: Option<ClipRequest>,
    pub(crate) scheduled_offset: Option<u64>,
    pub(crate) priority_level: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ClipRequest {
    Current { header: Option<String> },
    History { count: NonZeroUsize },
}

pub(crate) struct RouteToken {
    route: Option<String>,
    kind: CaptureKind,
}

/// One whitespace-free token with UTF-8 byte offsets into the original,
/// un-normalized input. Spans are half-open `[start, end)` and always land
/// on `char` boundaries because the scanner walks `char_indices`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Token<'a> {
    pub(crate) text: &'a str,
    pub(crate) start: usize,
    pub(crate) end: usize,
}

/// A token the marker walker can classify. `&str` tokens carry no position,
/// so the flat execution path discards the span list while the editor path
/// receives real byte ranges from [`Token`].
pub(crate) trait ParseToken {
    fn text(&self) -> &str;

    fn span(&self) -> Option<(usize, usize)> {
        None
    }
}

impl ParseToken for &str {
    fn text(&self) -> &str {
        self
    }
}

impl ParseToken for Token<'_> {
    fn text(&self) -> &str {
        self.text
    }

    fn span(&self) -> Option<(usize, usize)> {
        Some((self.start, self.end))
    }
}

/// Split the original input into maximal runs of non-whitespace characters,
/// recording each run's UTF-8 byte range. Whitespace classification matches
/// [`normalize_task_text`], so both paths agree on token boundaries.
pub(crate) fn tokenize_with_spans(raw: &str) -> Vec<Token<'_>> {
    let mut tokens = Vec::new();
    let mut start: Option<usize> = None;

    for (index, character) in raw.char_indices() {
        if character.is_whitespace() {
            if let Some(begin) = start.take() {
                tokens.push(Token {
                    text: &raw[begin..index],
                    start: begin,
                    end: index,
                });
            }
        } else if start.is_none() {
            start = Some(index);
        }
    }

    if let Some(begin) = start {
        tokens.push(Token {
            text: &raw[begin..],
            start: begin,
            end: raw.len(),
        });
    }

    tokens
}

/// One physical line of raw capture input, with UTF-8 byte offsets into the
/// original, un-normalized text. A physical line's `text` never includes
/// its terminator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RawLine<'a> {
    pub(crate) text: &'a str,
    pub(crate) start: usize,
    pub(crate) end: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ItemLine<'a> {
    pub(crate) raw: RawLine<'a>,
    pub(crate) line_number: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CaptureItem<'a> {
    pub(crate) index: usize,
    pub(crate) start: usize,
    pub(crate) end: usize,
    pub(crate) line_start: usize,
    pub(crate) line_end: usize,
    pub(crate) lines: Vec<ItemLine<'a>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ParsedCaptureItem {
    pub(crate) index: usize,
    pub(crate) start: usize,
    pub(crate) end: usize,
    pub(crate) line_start: usize,
    pub(crate) line_end: usize,
    pub(crate) parsed: ParsedCaptureText,
}

/// Draft-wide `@@` declarations plus the real capture items.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CaptureDraft<'a> {
    pub(crate) declarations: Vec<GlobalDeclarationToken<'a>>,
    pub(crate) items: Vec<CaptureItem<'a>>,
}

/// One `@@...` declaration token plus the original physical line it came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct GlobalDeclarationToken<'a> {
    pub(crate) token: Token<'a>,
    pub(crate) line_number: usize,
}

/// A strict, executable global destination inherited by items with no local
/// route/mode marker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ParsedGlobalDestination {
    pub(crate) start: usize,
    pub(crate) end: usize,
    pub(crate) line: usize,
    pub(crate) route: String,
    pub(crate) block_id: Option<String>,
    pub(crate) kind: CaptureKind,
}

impl ParsedGlobalDestination {
    pub(crate) fn mode_label(&self) -> &'static str {
        match self.kind {
            CaptureKind::SubBullet { .. } => "sub_bullet",
            _ => "task",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ParsedCaptureDraft {
    pub(crate) global: Option<ParsedGlobalDestination>,
    pub(crate) items: Vec<ParsedCaptureItem>,
    pub(crate) warnings: Vec<String>,
}

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
/// original draft.
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
        items: split_items_from_item_lines(&item_lines),
    }
}

fn split_items_from_item_lines<'a>(
    lines: &[ItemLine<'a>],
) -> Vec<CaptureItem<'a>> {
    let mut items = Vec::new();
    let mut current: Vec<ItemLine<'_>> = Vec::new();

    for line in lines.iter().copied() {
        if line.raw.text.trim().is_empty() {
            push_capture_item(&mut items, &mut current);
            continue;
        }

        current.push(line);
    }

    push_capture_item(&mut items, &mut current);
    items
}

fn push_capture_item<'a>(
    items: &mut Vec<CaptureItem<'a>>,
    current: &mut Vec<ItemLine<'a>>,
) {
    let Some(first) = current.first().copied() else {
        return;
    };
    let last = current.last().copied().expect("nonempty item");
    items.push(CaptureItem {
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
fn strip_bullet_marker_at(
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

fn is_marker_only_placeholder(line_text: &str) -> bool {
    marker_only_placeholder_after_prefix(line_text, 0)
        || marker_only_placeholder_after_prefix(line_text, 1)
        || marker_only_placeholder_after_prefix(line_text, 2)
}

fn marker_only_placeholder_after_prefix(
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
    parse_clip_markers: bool,
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

    let item_outcomes = draft
        .items
        .iter()
        .map(|item| {
            parse_capture_item(
                item,
                forced_route,
                forced_section,
                parse_clip_markers,
            )
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
        inherit_global_destination(&mut outcome.parsed, global);
    }
    Ok(outcome.parsed)
}

pub(crate) fn parse_capture_draft_with_clip_control(
    raw_text: &str,
    forced_route: Option<&str>,
    forced_section: Option<&str>,
    parse_clip_markers: bool,
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

    let mut item_outcomes = draft
        .items
        .iter()
        .map(|item| {
            parse_capture_item(
                item,
                forced_route,
                forced_section,
                parse_clip_markers,
            )
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

    let items = item_outcomes
        .drain(..)
        .map(|mut outcome| {
            if forced_route.is_none()
                && let Some(global) = &global
            {
                inherit_global_destination(&mut outcome.parsed, global);
            }
            ParsedCaptureItem {
                index: outcome.index,
                start: outcome.start,
                end: outcome.end,
                line_start: outcome.line_start,
                line_end: outcome.line_end,
                parsed: outcome.parsed,
            }
        })
        .collect();

    Ok(ParsedCaptureDraft {
        global,
        items,
        warnings,
    })
}

fn resolve_global_declaration_strict(
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

fn duplicate_global_destination_error(
    first_line: usize,
    second_line: usize,
) -> String {
    format!(
        "duplicate global destination declaration on line {second_line}; first declaration is on line {first_line}"
    )
}

fn capture_shadow_warnings(
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

fn global_destination_shadowed_warning(
    local_marker: &str,
    declaration: &str,
) -> String {
    format!(
        "this item's {local_marker} marker overrides the {declaration} destination it declares; move {declaration} to an item without a local marker, or delete {local_marker}"
    )
}

fn parse_global_destination_token(
    token: &str,
    start: usize,
    end: usize,
    line: usize,
) -> Result<ParsedGlobalDestination, String> {
    let rest = token
        .strip_prefix("@@")
        .ok_or_else(|| GLOBAL_DESTINATION_SHAPE_ERROR.to_string())?;
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

fn inherit_global_destination(
    parsed: &mut ParsedCaptureText,
    global: &ParsedGlobalDestination,
) {
    if parsed.route.is_some() || !matches!(parsed.kind, CaptureKind::Task) {
        return;
    }
    parsed.route = Some(global.route.clone());
    parsed.kind = global.kind.clone();
}

struct ParsedCaptureItemOutcome<'a> {
    index: usize,
    start: usize,
    end: usize,
    line_start: usize,
    line_end: usize,
    parsed: ParsedCaptureText,
    declarations: Vec<GlobalDeclarationToken<'a>>,
    local_destination_marker: Option<String>,
}

fn parse_capture_item<'a>(
    item: &CaptureItem<'a>,
    forced_route: Option<&str>,
    forced_section: Option<&str>,
    parse_clip_markers: bool,
) -> Result<ParsedCaptureItemOutcome<'a>, String> {
    let Some((parent_line, child_lines)) = item.lines.split_first() else {
        return Err(missing_text_error());
    };
    let detect_route = forced_route.is_none();
    let mut declarations = Vec::new();

    let parent_normalized = normalize_task_text(parent_line.raw.text);
    if parent_normalized.is_empty() {
        return Err(missing_text_error());
    }
    let parent_tokens = tokenize_line_with_spans(&parent_line.raw);
    let parent_outcome =
        resolve_line(parent_tokens, true, detect_route, parse_clip_markers)?;
    declarations.extend(global_declarations_from_tokens(
        parent_outcome.declarations,
        parent_line.line_number,
    ));
    if parent_outcome.body.is_empty() {
        return Err(missing_text_error());
    }

    let mut aggregate = AggregateMarkers::default();
    aggregate.absorb(parent_outcome.markers, parent_outcome.route)?;

    let mut sub_bullets = Vec::new();
    let mut has_first_level_owner = false;
    for line in child_lines {
        let line_number = line.line_number;
        let authored = match classify_authored_line(line.raw) {
            AuthoredLineClass::EmptyOrPlaceholder => continue,
            AuthoredLineClass::Invalid => {
                return Err(invalid_child_line_error(line_number));
            }
            AuthoredLineClass::Item(authored) => authored,
        };
        if authored.depth == AuthoredDepth::Nested && !has_first_level_owner {
            return Err(orphaned_nested_bullet_error(line_number));
        }
        let child_line = RawLine {
            text: authored.body,
            start: authored.body_start,
            end: line.raw.end,
        };
        let tokens = tokenize_line_with_spans(&child_line);
        let outcome =
            resolve_line(tokens, false, detect_route, parse_clip_markers)?;
        declarations.extend(global_declarations_from_tokens(
            outcome.declarations,
            line_number,
        ));
        if outcome.body.is_empty() {
            return Err(empty_child_after_markers_error(line_number));
        }
        aggregate.absorb(outcome.markers, outcome.route)?;
        sub_bullets.push(AuthoredSubBullet {
            body: outcome.body,
            depth: authored.depth,
        });
        if authored.depth == AuthoredDepth::First {
            has_first_level_owner = true;
        }
    }

    if let Some(section) = forced_section {
        let Some(route) = forced_route else {
            return Err("--section requires --route".to_string());
        };
        if section.trim().is_empty() {
            return Err("--section must not be empty".to_string());
        }
        let route = normalize_forced_route(route)?;
        return Ok(parsed_capture_item_outcome(
            item,
            ParsedCaptureText {
                body: parent_outcome.body,
                clip: aggregate.clip,
                route: Some(route),
                kind: CaptureKind::Bullet {
                    section_prefix: Some(section.to_string()),
                    exact: true,
                },
                scheduled_offset: aggregate.scheduled_offset,
                priority_level: aggregate.priority_level,
                sub_bullets,
            },
            declarations,
            aggregate
                .route
                .as_ref()
                .map(|route| route.marker_text.clone()),
        ));
    }

    if let Some(route) = forced_route {
        let route = normalize_forced_route(route)?;
        return Ok(parsed_capture_item_outcome(
            item,
            ParsedCaptureText {
                body: parent_outcome.body,
                clip: aggregate.clip,
                route: Some(route),
                kind: CaptureKind::Task,
                scheduled_offset: aggregate.scheduled_offset,
                priority_level: aggregate.priority_level,
                sub_bullets,
            },
            declarations,
            aggregate
                .route
                .as_ref()
                .map(|route| route.marker_text.clone()),
        ));
    }

    let local_destination_marker = aggregate
        .route
        .as_ref()
        .map(|route| route.marker_text.clone());
    let (route, kind) = match aggregate.route {
        Some(line_route) => (line_route.token.route, line_route.token.kind),
        None => (None, CaptureKind::Task),
    };
    if matches!(kind, CaptureKind::PomodoroNote) {
        if aggregate.scheduled_offset.is_some() {
            return Err(pomodoro_note_schedule_conflict_error());
        }
        if aggregate.priority_level.is_some() {
            return Err(pomodoro_note_priority_conflict_error());
        }
    }
    Ok(parsed_capture_item_outcome(
        item,
        ParsedCaptureText {
            body: parent_outcome.body,
            clip: aggregate.clip,
            route,
            kind,
            scheduled_offset: aggregate.scheduled_offset,
            priority_level: aggregate.priority_level,
            sub_bullets,
        },
        declarations,
        local_destination_marker,
    ))
}

fn parsed_capture_item_outcome<'a>(
    item: &CaptureItem<'a>,
    parsed: ParsedCaptureText,
    declarations: Vec<GlobalDeclarationToken<'a>>,
    local_destination_marker: Option<String>,
) -> ParsedCaptureItemOutcome<'a> {
    ParsedCaptureItemOutcome {
        index: item.index,
        start: item.start,
        end: item.end,
        line_start: item.line_start,
        line_end: item.line_end,
        parsed,
        declarations,
        local_destination_marker,
    }
}

fn global_declarations_from_tokens<'a>(
    tokens: Vec<Token<'a>>,
    line_number: usize,
) -> Vec<GlobalDeclarationToken<'a>> {
    tokens
        .into_iter()
        .map(|token| GlobalDeclarationToken { token, line_number })
        .collect()
}

/// One physical line's resolved item-wide markers and (when a route was
/// recognized on this line) its route/mode token. `body` is the line's
/// remaining text after every recognized marker is removed; it is empty
/// exactly when the line held no non-marker tokens.
struct LineOutcome<'a> {
    body: String,
    markers: TerminalMarkers,
    route: Option<LineRoute>,
    declarations: Vec<Token<'a>>,
}

struct LineRoute {
    token: RouteToken,
    marker_text: String,
}

/// Remove every `@@...` token from `tokens`, returning them in source order.
fn take_global_declarations<T: ParseToken>(tokens: &mut Vec<T>) -> Vec<T> {
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

/// Resolve one physical line's whitespace tokens exactly like the original
/// single-line grammar resolved the whole draft. `leading` allows a
/// first-token route to win and is only ever set for the parent line, which
/// is the only line that preserves the established leading-route form.
/// `detect_route` is false whenever `--route`/`--section` already fixed the
/// route, in which case every `@...`-shaped token stays literal on every
/// line, exactly like the single-line forced-route path did.
fn resolve_line<'a>(
    mut tokens: Vec<Token<'a>>,
    leading: bool,
    detect_route: bool,
    parse_clip_markers: bool,
) -> Result<LineOutcome<'a>, String> {
    let declarations = take_global_declarations(&mut tokens);
    let (markers, _) =
        extract_terminal_markers(&mut tokens, parse_clip_markers);
    if tokens.is_empty() {
        return Ok(LineOutcome {
            body: String::new(),
            markers,
            route: None,
            declarations,
        });
    }

    let token_texts = tokens.iter().map(|token| token.text).collect::<Vec<_>>();
    reject_legacy_bullet_markers(&token_texts, detect_route)?;

    // The bare `#` Pomodoro-note marker claims the route/mode slot from the
    // final token position, exactly like an `@route` token does, but it
    // never has a partially-typed form and never coexists with one.
    if tokens
        .last()
        .is_some_and(|token| is_pomodoro_note_marker(token.text))
    {
        if !detect_route {
            return Err(pomodoro_note_forced_route_conflict_error());
        }
        let marker_text = tokens.last().expect("last token").text.to_string();
        tokens.pop();
        if tokens.is_empty() {
            return Err(missing_text_error());
        }
        if (leading
            && tokens
                .first()
                .is_some_and(|token| is_route_marker(token.text)))
            || tokens
                .last()
                .is_some_and(|token| is_route_marker(token.text))
        {
            return Err(pomodoro_note_route_conflict_error());
        }
        return Ok(LineOutcome {
            body: join_parse_tokens(&tokens),
            markers,
            route: Some(LineRoute {
                token: RouteToken {
                    route: None,
                    kind: CaptureKind::PomodoroNote,
                },
                marker_text,
            }),
            declarations,
        });
    }

    if !detect_route {
        return Ok(LineOutcome {
            body: join_parse_tokens(&tokens),
            markers,
            route: None,
            declarations,
        });
    }

    // Leading route wins: when the first token is a route token followed by
    // body text, route by it and do not inspect later route-looking tokens.
    if leading && let Some(token) = parse_terminal_route_token(tokens[0].text)?
    {
        let rest = &tokens[1..];
        if rest.is_empty() {
            if !matches!(token.kind, CaptureKind::Task) {
                return Err(missing_text_error());
            }
            // A bare `@foo` with no body stays literal task text.
        } else {
            if rest.iter().any(|token| token.text == "#") {
                return Err(pomodoro_note_route_conflict_error());
            }
            return Ok(LineOutcome {
                body: join_parse_tokens(rest),
                markers,
                route: Some(LineRoute {
                    token,
                    marker_text: tokens[0].text.to_string(),
                }),
                declarations,
            });
        }
    }

    validate_special_terminal_markers_line(&token_texts, leading)?;

    // Otherwise a trailing route token routes the body that precedes it.
    if let Some((last, rest)) = tokens.split_last()
        && !rest.is_empty()
        && let Some(token) = parse_terminal_route_token(last.text)?
    {
        if rest.iter().any(|token| token.text == "#") {
            return Err(pomodoro_note_route_conflict_error());
        }
        return Ok(LineOutcome {
            body: join_parse_tokens(rest),
            markers,
            route: Some(LineRoute {
                token,
                marker_text: last.text.to_string(),
            }),
            declarations,
        });
    }

    Ok(LineOutcome {
        body: join_parse_tokens(&tokens),
        markers,
        route: None,
        declarations,
    })
}

fn join_parse_tokens<T: ParseToken>(tokens: &[T]) -> String {
    tokens
        .iter()
        .map(|token| token.text())
        .collect::<Vec<_>>()
        .join(" ")
}

/// Accumulate the four item-wide marker slots (route/mode, schedule,
/// priority, clipboard) across every physical line. Each slot may be set by
/// at most one line; a second line that resolves the same slot is ambiguous.
#[derive(Default)]
struct AggregateMarkers {
    clip: Option<ClipRequest>,
    scheduled_offset: Option<u64>,
    priority_level: Option<u64>,
    route: Option<LineRoute>,
}

impl AggregateMarkers {
    fn absorb(
        &mut self,
        markers: TerminalMarkers,
        route: Option<LineRoute>,
    ) -> Result<(), String> {
        if let Some(clip) = markers.clip {
            if self.clip.is_some() {
                return Err(duplicate_marker_error("clipboard marker (%)"));
            }
            self.clip = Some(clip);
        }
        if let Some(offset) = markers.scheduled_offset {
            if self.scheduled_offset.is_some() {
                return Err(duplicate_marker_error("schedule marker (s:<N>)"));
            }
            self.scheduled_offset = Some(offset);
        }
        if let Some(level) = markers.priority_level {
            if self.priority_level.is_some() {
                return Err(duplicate_marker_error("priority marker (p:<N>)"));
            }
            self.priority_level = Some(level);
        }
        if let Some(route) = route {
            if self.route.is_some() {
                return Err(duplicate_marker_error(
                    "route/mode marker (@route or #)",
                ));
            }
            self.route = Some(route);
        }
        Ok(())
    }
}

fn duplicate_marker_error(kind: &str) -> String {
    format!(
        "a {kind} may appear on only one line of the capture; found a \
second one"
    )
}

fn invalid_child_line_error(line_number: usize) -> String {
    format!(
        "capture line {line_number} must be a column-zero bullet or a \
two-space nested bullet using \"-\", \"*\", or \"+\" followed by a space or \
tab, or be left blank"
    )
}

fn empty_child_after_markers_error(line_number: usize) -> String {
    format!(
        "capture line {line_number} has no text left after its capture \
markers were removed"
    )
}

fn orphaned_nested_bullet_error(line_number: usize) -> String {
    format!(
        "capture line {line_number} is a nested bullet but has no preceding \
first-level authored bullet to attach to"
    )
}

pub(crate) fn missing_text_error() -> String {
    "task text is required; pass TEXT or pipe it on stdin".to_string()
}

fn missing_capture_item_error() -> String {
    MISSING_CAPTURE_ITEM_ERROR.to_string()
}

fn unsupported_global_destination_error(token: &str) -> String {
    format!("{GLOBAL_DESTINATION_SHAPE_ERROR}; {token} is not supported")
}

fn legacy_marker_error() -> String {
    "bullet section markers must be appended to an @route token; use \
@foo#bar instead of #bar @foo"
        .to_string()
}

fn pomodoro_note_route_conflict_error() -> String {
    "the '#' Pomodoro-note marker cannot be combined with an @route marker"
        .to_string()
}

fn pomodoro_note_forced_route_conflict_error() -> String {
    "the '#' Pomodoro-note marker cannot be combined with --route".to_string()
}

fn pomodoro_note_schedule_conflict_error() -> String {
    "the '#' Pomodoro-note marker cannot be combined with 's:<N>'".to_string()
}

fn pomodoro_note_priority_conflict_error() -> String {
    "the '#' Pomodoro-note marker cannot be combined with 'p:<N>'".to_string()
}

pub(crate) fn normalize_task_text(raw_text: &str) -> String {
    raw_text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Canonical whitespace-free selector slug for task-section and Pomodoro-name
/// third components: trim, collapse internal whitespace to one space,
/// ASCII-lowercase, then replace each remaining space with `-`.
pub(crate) fn selector_slug(text: &str) -> String {
    let mut result = String::new();
    for word in text.split_whitespace() {
        if !result.is_empty() {
            result.push('-');
        }
        result.push_str(&word.to_ascii_lowercase());
    }
    result
}

/// Parse one whitespace-free token as an `@route` token, returning `None` when
/// it does not begin with `@` or its route part is not a valid route name. A
/// `#` suffix selects bullet mode and is split off before route validation.
fn parse_route_token(token: &str) -> Option<RouteToken> {
    let rest = token.strip_prefix('@')?;
    let (route_part, bullet) = match rest.split_once('#') {
        Some((route, prefix)) => {
            let marker = (!prefix.is_empty()).then(|| prefix.to_string());
            (route, Some(marker))
        }
        None => (rest, None),
    };
    is_route_token(route_part).then(|| RouteToken {
        route: Some(route_part.to_ascii_lowercase()),
        kind: match bullet {
            Some(section_prefix) => CaptureKind::Bullet {
                section_prefix,
                exact: false,
            },
            None => CaptureKind::Task,
        },
    })
}

fn parse_terminal_route_token(
    token: &str,
) -> Result<Option<RouteToken>, String> {
    if is_sub_bullet_marker_candidate(token) {
        return parse_sub_bullet_route_token(token).map(Some);
    }
    if is_task_block_id_marker_candidate(token) {
        return parse_task_block_id_route_token(token).map(Some);
    }
    if is_retired_double_colon_marker_candidate(token) {
        return Err(RETIRED_DOUBLE_COLON_ERROR.to_string());
    }
    if is_pomodoro_marker_candidate(token) {
        return parse_pomodoro_route_token(token).map(Some);
    }
    Ok(parse_route_token(token))
}

fn parse_sub_bullet_route_token(token: &str) -> Result<RouteToken, String> {
    let marker = token
        .strip_prefix('@')
        .ok_or_else(|| SUB_BULLET_SHAPE_ERROR.to_string())?;
    let Some((route, rest)) = marker.split_once('+') else {
        return Err(SUB_BULLET_SHAPE_ERROR.to_string());
    };
    let (block_id, section) = match rest.split_once('#') {
        Some((block_id, section)) => (block_id, Some(section)),
        None => (rest, None),
    };
    if route.is_empty() {
        return Err(SUB_BULLET_SHAPE_ERROR.to_string());
    }
    if !is_route_token(route) {
        return Err(SUB_BULLET_ROUTE_ERROR.to_string());
    }
    if block_id.is_empty() {
        return Err(if section.is_some() {
            format!(
                "sub-bullet capture requires a block ID before the task section: @<route>+<block-id>#<section> (run 'bob capture-tasks -r {}' to list task block IDs)",
                route.to_ascii_lowercase()
            )
        } else {
            format!(
                "sub-bullet capture requires a block ID: @<route>+<block-id> (run 'bob capture-tasks -r {}' to list task block IDs)",
                route.to_ascii_lowercase()
            )
        });
    }
    if !is_block_id(block_id) {
        return Err(SUB_BULLET_BLOCK_ID_ERROR.to_string());
    }
    let section = match section {
        None => None,
        Some("") => {
            return Err(format!(
                "sub-bullet capture requires a task section: @<route>+<block-id>#<section> (run 'bob capture-task-sections -r {} -i {}' to list task sections)",
                route.to_ascii_lowercase(),
                block_id
            ));
        }
        Some(selector) if !is_selector_component(selector) => {
            return Err(SUB_BULLET_SECTION_ERROR.to_string());
        }
        Some(selector) => Some(TaskSectionSelector {
            text: selector.to_string(),
            exact: false,
        }),
    };

    Ok(RouteToken {
        route: Some(route.to_ascii_lowercase()),
        kind: CaptureKind::SubBullet {
            target: SubBulletTarget::BlockId(block_id.to_string()),
            section,
        },
    })
}

/// Return whether one already-whitespace-free selector component is typeable.
///
/// This is the shared third-component grammar for `@route+id#section` and
/// `@route:id#pomodoro`: ASCII alphanumerics plus `& ' ( ) , . / -`.
pub(crate) fn is_selector_component(value: &str) -> bool {
    !value.is_empty() && value.bytes().all(is_selector_byte)
}

fn is_selector_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric()
        || matches!(
            byte,
            b'&' | b'\'' | b'(' | b')' | b',' | b'.' | b'/' | b'-'
        )
}

fn is_sub_bullet_marker_candidate(token: &str) -> bool {
    let Some(marker) = token.strip_prefix('@') else {
        return false;
    };
    if token.starts_with("@!") {
        return false;
    }
    let Some(plus) = marker.find('+') else {
        return false;
    };
    marker
        .find([':', '#', '^'])
        .is_none_or(|separator| plus < separator)
}

fn parse_task_block_id_route_token(token: &str) -> Result<RouteToken, String> {
    let marker = token.strip_prefix('@').ok_or_else(|| {
        "task block-ID capture markers must use @<route>^<block-id>".to_string()
    })?;
    let Some((route, block_id)) = marker.split_once('^') else {
        return Err(
            "task block-ID capture markers must use @<route>^<block-id>"
                .to_string(),
        );
    };
    if !is_route_token(route) {
        return Err(TASK_BLOCK_ID_ROUTE_ERROR.to_string());
    }
    if !is_block_id(block_id) {
        return Err(TASK_BLOCK_ID_ERROR.to_string());
    }

    Ok(RouteToken {
        route: Some(route.to_ascii_lowercase()),
        kind: CaptureKind::TaskWithBlockId {
            block_id: block_id.to_string(),
        },
    })
}

/// Return whether a terminal token belongs to the ordinary task-with-ID
/// marker grammar. A caret that follows `#` remains part of a bullet
/// section prefix, a caret that follows `+` remains part of the
/// sub-bullet block-ID component, and a caret that follows `:` remains
/// part of a Pomodoro block ID.
fn is_task_block_id_marker_candidate(token: &str) -> bool {
    let Some(marker) = token.strip_prefix('@') else {
        return false;
    };
    if token.starts_with("@!") {
        return false;
    }
    let Some(caret) = marker.find('^') else {
        return false;
    };
    marker
        .find(['#', ':', '+'])
        .is_none_or(|separator| caret < separator)
}

/// Return whether a terminal token is the retired double-colon
/// task-with-ID spelling. A `::` that follows `#`, `+`, or `^` stays
/// inside that earlier family; a single `:` that begins before `::`
/// stays in the Pomodoro family so this detector cannot steal
/// `@route:id` or misreport `@route::id` as a malformed Pomodoro marker.
fn is_retired_double_colon_marker_candidate(token: &str) -> bool {
    let Some(marker) = token.strip_prefix('@') else {
        return false;
    };
    if token.starts_with("@!") {
        return false;
    }
    let Some(double_colon) = marker.find("::") else {
        return false;
    };
    if marker
        .find(['#', '+', '^'])
        .is_some_and(|separator| separator < double_colon)
    {
        return false;
    }
    marker.find(':').is_none_or(|colon| colon >= double_colon)
}

fn parse_pomodoro_route_token(token: &str) -> Result<RouteToken, String> {
    let marker = token
        .strip_prefix("@!")
        .or_else(|| token.strip_prefix('@'))
        .ok_or_else(|| {
            "Pomodoro capture markers must use @<route>:<block-id>".to_string()
        })?;
    let Some((route, block_id)) = marker.split_once(':') else {
        return Err(
            "Pomodoro capture markers must use @<route>:<block-id>".to_string()
        );
    };
    if !is_route_token(route) {
        return Err(POMODORO_ROUTE_ERROR.to_string());
    }
    if !is_block_id(block_id) {
        return Err(POMODORO_BLOCK_ID_ERROR.to_string());
    }

    Ok(RouteToken {
        route: Some(route.to_ascii_lowercase()),
        kind: CaptureKind::Pomodoro {
            block_id: block_id.to_string(),
        },
    })
}

/// Return whether a terminal token belongs to the Pomodoro-marker grammar.
/// A colon that follows `#` remains part of an ordinary bullet section prefix.
fn is_pomodoro_marker_candidate(token: &str) -> bool {
    let Some(marker) =
        token.strip_prefix("@!").or_else(|| token.strip_prefix('@'))
    else {
        return false;
    };
    if token.starts_with("@!") {
        return true;
    }

    let colon = marker.find(':');
    let hash = marker.find('#');
    let caret = marker.find('^');
    let plus = marker.find('+');
    colon.is_some_and(|colon| {
        hash.is_none_or(|hash| colon < hash)
            && caret.is_none_or(|caret| colon < caret)
            && plus.is_none_or(|plus| colon < plus)
            && marker[..colon]
                .bytes()
                .any(|byte| byte.is_ascii_alphabetic())
    })
}

/// Catch an invalid sub-bullet/Pomodoro marker shape sitting at a position
/// [`resolve_line`]'s route detection would otherwise never inspect closely
/// enough to reject -- most importantly a lone invalid marker with no body
/// on the other side, which the leading/trailing route checks both skip.
/// `check_first` mirrors [`resolve_line`]'s `leading` flag: only the parent
/// line's first token can ever resolve a route, so only it is validated.
fn validate_special_terminal_markers_line(
    tokens: &[&str],
    check_first: bool,
) -> Result<(), String> {
    let first = check_first.then(|| tokens.first()).flatten();
    for token in first.into_iter().chain(tokens.last()) {
        if is_sub_bullet_marker_candidate(token) {
            parse_sub_bullet_route_token(token)?;
            continue;
        }
        if is_task_block_id_marker_candidate(token) {
            parse_task_block_id_route_token(token)?;
            continue;
        }
        if is_retired_double_colon_marker_candidate(token) {
            return Err(RETIRED_DOUBLE_COLON_ERROR.to_string());
        }
        if is_pomodoro_marker_candidate(token) {
            parse_pomodoro_route_token(token)?;
        }
    }
    Ok(())
}

pub(crate) fn is_block_id(value: &str) -> bool {
    !value.is_empty() && value.bytes().all(collect_done::is_block_id_byte)
}

/// Parse one whitespace-free token as a schedule offset (`s:<N>`), returning
/// the non-negative day count. Invalid or overflowing tokens stay literal.
pub(crate) fn parse_schedule_token(token: &str) -> Option<u64> {
    let digits = token.strip_prefix("s:")?;
    if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    digits.parse::<u64>().ok()
}

/// Parse one whitespace-free token as a priority level (`p:<N>`), returning the
/// 1-based level number. Non-digit or overflowing tokens stay literal.
pub(crate) fn parse_priority_token(token: &str) -> Option<u64> {
    let digits = token.strip_prefix("p:")?;
    if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    digits.parse::<u64>().ok()
}

fn parse_clip_token(token: &str) -> Option<ClipRequest> {
    let header = token.strip_prefix('%')?;
    if header.is_empty() {
        return Some(ClipRequest::Current { header: None });
    }
    if header.bytes().all(|byte| byte.is_ascii_digit()) {
        return header
            .parse::<usize>()
            .ok()
            .and_then(NonZeroUsize::new)
            .map(|count| ClipRequest::History { count });
    }
    capture_clip::is_valid_header(header).then(|| ClipRequest::Current {
        header: Some(header.to_string()),
    })
}

/// Remove schedule and clipboard markers from the terminal marker region.
/// Each marker kind is extracted at most once, in either order, on either
/// side of a trailing route token. A duplicate or non-marker stops parsing.
///
/// The returned span list records the byte range of every removed token that
/// carried one. `&str` tokens carry no position, so the flat execution path
/// always receives an empty list and discards it.
pub(crate) fn extract_terminal_markers<T: ParseToken>(
    tokens: &mut Vec<T>,
    parse_clip_markers: bool,
) -> (TerminalMarkers, Vec<(SpanKind, usize, usize)>) {
    let marker_like = |token: &str| {
        parse_schedule_token(token).is_some()
            || parse_priority_token(token).is_some()
            || (parse_clip_markers && parse_clip_token(token).is_some())
    };
    let route_index = if tokens
        .last()
        .is_some_and(|token| is_route_marker(token.text()))
    {
        Some(tokens.len() - 1)
    } else {
        let mut index = tokens.len();
        while index > 0 && marker_like(tokens[index - 1].text()) {
            index -= 1;
        }
        if index > 0 && is_route_marker(tokens[index - 1].text()) {
            Some(index - 1)
        } else {
            None
        }
    };
    let mut cursor = match route_index {
        Some(index) if index == tokens.len() - 1 => index,
        _ => tokens.len(),
    };
    let route_before_trailing_markers =
        route_index.is_some_and(|index| index < tokens.len() - 1);
    let mut markers = TerminalMarkers::default();
    let mut spans = Vec::new();
    let mut reached_route = false;

    while cursor > 0 {
        let index = cursor - 1;
        if route_index == Some(index) {
            reached_route = true;
            break;
        }
        let Some(kind) = extract_terminal_marker(
            tokens[index].text(),
            parse_clip_markers,
            &mut markers,
        ) else {
            break;
        };
        if let Some((start, end)) = tokens[index].span() {
            spans.push((kind, start, end));
        }
        tokens.remove(index);
        cursor -= 1;
    }

    if reached_route && route_before_trailing_markers {
        cursor = route_index.expect("reached route");
        while cursor > 0 {
            let index = cursor - 1;
            let Some(kind) = extract_terminal_marker(
                tokens[index].text(),
                parse_clip_markers,
                &mut markers,
            ) else {
                break;
            };
            if let Some((start, end)) = tokens[index].span() {
                spans.push((kind, start, end));
            }
            tokens.remove(index);
            cursor -= 1;
        }
    }

    (markers, spans)
}

/// Consume one terminal marker token, returning the span kind it produced or
/// `None` when the token is not a marker or repeats an already-seen kind.
fn extract_terminal_marker(
    token: &str,
    parse_clip_markers: bool,
    markers: &mut TerminalMarkers,
) -> Option<SpanKind> {
    if let Some(offset) = parse_schedule_token(token) {
        if markers.scheduled_offset.is_some() {
            return None;
        }
        markers.scheduled_offset = Some(offset);
        return Some(SpanKind::Schedule);
    }
    if let Some(number) = parse_priority_token(token) {
        if markers.priority_level.is_some() {
            return None;
        }
        markers.priority_level = Some(number);
        return Some(SpanKind::Priority);
    }
    if parse_clip_markers
        && let Some(clip) = parse_clip_token(token)
        && markers.clip.is_none()
    {
        markers.clip = Some(clip);
        return Some(SpanKind::Clipboard);
    }
    None
}

fn is_route_marker(token: &str) -> bool {
    is_pomodoro_note_marker(token)
        || parse_route_token(token).is_some()
        || (is_sub_bullet_marker_candidate(token)
            && parse_sub_bullet_route_token(token).is_ok())
        || (is_task_block_id_marker_candidate(token)
            && parse_task_block_id_route_token(token).is_ok())
        || (is_pomodoro_marker_candidate(token)
            && parse_pomodoro_route_token(token).is_ok())
}

/// The bare `#` token: a Pomodoro-note marker, recognized only in the
/// terminal marker region. `#<anything-else>` stays a distinct, retired
/// legacy bullet-marker shape (see [`reject_legacy_bullet_markers`]).
fn is_pomodoro_note_marker(token: &str) -> bool {
    token == "#"
}

/// Reject the retired standalone bullet marker forms so they fail clearly
/// instead of silently capturing literal `#...` text. The marker is honored
/// only when appended to an `@route` token (`@foo#bar`).
///
/// Two terminal positions are rejected: a final token that itself starts with
/// `#`, and (when `allow_route`) a final plain `@route` token preceded by a
/// `#...` token. A `#tag` anywhere else stays literal task text.
fn reject_legacy_bullet_markers(
    tokens: &[&str],
    allow_route: bool,
) -> Result<(), String> {
    let Some(&last) = tokens.last() else {
        return Ok(());
    };

    if last.starts_with('#') && !is_pomodoro_note_marker(last) {
        return Err(legacy_marker_error());
    }

    if allow_route
        && tokens.len() >= 2
        && tokens[tokens.len() - 2].starts_with('#')
        && !is_pomodoro_note_marker(tokens[tokens.len() - 2])
        && parse_route_token(last)
            .is_some_and(|token| matches!(token.kind, CaptureKind::Task))
    {
        return Err(legacy_marker_error());
    }

    Ok(())
}

fn normalize_forced_route(route: &str) -> Result<String, String> {
    if is_route_token(route) {
        return Ok(route.to_ascii_lowercase());
    }

    Err("--route must contain only A-Z, a-z, 0-9, '_' or '-'".to_string())
}

pub(crate) fn is_route_token(value: &str) -> bool {
    !value.is_empty()
        && value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-')
        })
}

const GLOBAL_DESTINATION_SHAPE_ERROR: &str =
    "global destination must be @@<route> or @@<route>+<block-id>";
const GLOBAL_DESTINATION_ROUTE_ERROR: &str =
    "global destination route must contain only A-Z, a-z, 0-9, '_' or '-'";
const GLOBAL_DESTINATION_BLOCK_ID_ERROR: &str =
    "global destination block ID must be non-empty and contain only A-Z, a-z, 0-9 or '-'";
const MISSING_CAPTURE_ITEM_ERROR: &str =
    "global destination declaration has no capture item; add a capture item to this draft";
const SUB_BULLET_SHAPE_ERROR: &str =
    "sub-bullet capture markers must use @<route>+<block-id> or @<route>+<block-id>#<section>";
const SUB_BULLET_ROUTE_ERROR: &str =
    "sub-bullet capture route must contain only A-Z, a-z, 0-9, '_' or '-'";
const SUB_BULLET_BLOCK_ID_ERROR: &str =
    "sub-bullet capture block ID must be non-empty and contain only A-Z, a-z, 0-9 or '-'";
const SUB_BULLET_SECTION_ERROR: &str =
    "sub-bullet capture section must contain only A-Z, a-z, 0-9 or & ' ( ) , . / -";
const TASK_BLOCK_ID_ROUTE_ERROR: &str =
    "task block-ID capture route must contain only A-Z, a-z, 0-9, '_' or '-'";
const TASK_BLOCK_ID_ERROR: &str =
    "task block-ID capture block ID must be non-empty and contain only A-Z, a-z, 0-9 or '-'";
const RETIRED_DOUBLE_COLON_ERROR: &str =
    "'@<route>::<block-id>' is no longer accepted; use '@<route>^<block-id>' to create an ordinary task with an authored block ID";
const POMODORO_ROUTE_ERROR: &str =
    "Pomodoro capture route must contain only A-Z, a-z, 0-9, '_' or '-'";
const POMODORO_BLOCK_ID_ERROR: &str =
    "Pomodoro capture block ID must be non-empty and contain only A-Z, a-z, 0-9, '_' or '-'";

// ---------------------------------------------------------------------------
// Editor-facing parse
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum SpanKind {
    Route,
    Section,
    TaskBlockIdRoute,
    TaskBlockId,
    PomodoroRoute,
    PomodoroBlockId,
    SubBulletRoute,
    SubBulletBlockId,
    SubBulletSection,
    GlobalRoute,
    GlobalSubBulletRoute,
    GlobalSubBulletBlockId,
    PomodoroNote,
    Schedule,
    Priority,
    Clipboard,
    InteractivePlaceholder,
    WikilinkDelimiter,
    WikilinkTarget,
    WikilinkHeading,
    WikilinkBlockId,
    WikilinkAlias,
}

impl SpanKind {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Route => "route",
            Self::Section => "section",
            Self::TaskBlockIdRoute => "task_block_id_route",
            Self::TaskBlockId => "task_block_id",
            Self::PomodoroRoute => "pomodoro_route",
            Self::PomodoroBlockId => "pomodoro_block_id",
            Self::SubBulletRoute => "sub_bullet_route",
            Self::SubBulletBlockId => "sub_bullet_block_id",
            Self::SubBulletSection => "sub_bullet_section",
            Self::GlobalRoute => "global_route",
            Self::GlobalSubBulletRoute => "global_sub_bullet_route",
            Self::GlobalSubBulletBlockId => "global_sub_bullet_block_id",
            Self::PomodoroNote => "pomodoro_note",
            Self::Schedule => "schedule",
            Self::Priority => "priority",
            Self::Clipboard => "clipboard",
            Self::InteractivePlaceholder => "interactive_placeholder",
            Self::WikilinkDelimiter => "wikilink_delimiter",
            Self::WikilinkTarget => "wikilink_target",
            Self::WikilinkHeading => "wikilink_heading",
            Self::WikilinkBlockId => "wikilink_block_id",
            Self::WikilinkAlias => "wikilink_alias",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub(crate) struct Span {
    pub(crate) start: usize,
    pub(crate) end: usize,
    pub(crate) kind: SpanKind,
}

/// The full documented severity vocabulary of the `capture-parse` JSON
/// contract. Today's grammar only raises errors; `warning` and `info` stay
/// reserved so the wire format does not change when it starts to.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Severity {
    Error,
    Warning,
    Info,
}

impl Severity {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Error => "error",
            Self::Warning => "warning",
            Self::Info => "info",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct Diagnostic {
    pub(crate) severity: Severity,
    /// Stable snake_case identifier for programmatic handling.
    pub(crate) code: &'static str,
    pub(crate) message: String,
    pub(crate) range: Option<(usize, usize)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum EditorMode {
    Task,
    Bullet,
    PomodoroTask,
    PomodoroNote,
    SubBullet,
    Incomplete,
}

impl EditorMode {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Task => "task",
            Self::Bullet => "bullet",
            Self::PomodoroTask => "pomodoro_task",
            Self::PomodoroNote => "pomodoro_note",
            Self::SubBullet => "sub_bullet",
            Self::Incomplete => "incomplete",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Need {
    Route,
    Section,
    BlockId,
    PomodoroId,
    Task,
    TaskSection,
}

impl Need {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Route => "route",
            Self::Section => "section",
            Self::BlockId => "block_id",
            Self::PomodoroId => "pomodoro_id",
            Self::Task => "task",
            Self::TaskSection => "task_section",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct EditorParse {
    pub(crate) body: String,
    pub(crate) mode: EditorMode,
    pub(crate) route: Option<String>,
    pub(crate) section: Option<String>,
    pub(crate) block_id: Option<String>,
    pub(crate) needs: Vec<Need>,
    pub(crate) spans: Vec<Span>,
    pub(crate) diagnostics: Vec<Diagnostic>,
    /// Normalized authored-child bodies plus semantic depth for every other
    /// valid, nonempty physical line, in source order. A malformed,
    /// orphaned, or empty-after-markers child line is reported as a
    /// diagnostic instead and excluded here.
    pub(crate) sub_bullets: Vec<AuthoredSubBullet>,
    pub(crate) items: Vec<EditorItemParse>,
    pub(crate) global_destination: Option<EditorGlobalDestination>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct EditorGlobalDestination {
    pub(crate) start: usize,
    pub(crate) end: usize,
    pub(crate) line: usize,
    pub(crate) mode: EditorMode,
    pub(crate) route: Option<String>,
    pub(crate) block_id: Option<String>,
    pub(crate) needs: Vec<Need>,
    pub(crate) inherit: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct EditorItemParse {
    pub(crate) index: usize,
    pub(crate) start: usize,
    pub(crate) end: usize,
    pub(crate) line_start: usize,
    pub(crate) line_end: usize,
    pub(crate) body: String,
    pub(crate) mode: EditorMode,
    pub(crate) route: Option<String>,
    pub(crate) section: Option<String>,
    pub(crate) block_id: Option<String>,
    pub(crate) needs: Vec<Need>,
    pub(crate) spans: Vec<Span>,
    pub(crate) diagnostics: Vec<Diagnostic>,
    pub(crate) sub_bullets: Vec<AuthoredSubBullet>,
    pub(crate) has_local_destination: bool,
    /// Every *complete* (non-incomplete) local destination marker this item
    /// owns, in source order, across every one of its lines. `rewrite_draft`
    /// uses the length of this list to tell "no local marker" from "one" from
    /// "more than one" (Rule A6), and the sole entry's span to build the
    /// absorb-local-marker edit (Rule A1).
    pub(crate) local_destination_markers: Vec<LocalDestinationMarker>,
}

/// One complete local destination marker token an item owns, with the span
/// the marker text (`@route`, `@route+block-id`, `@route#Section`,
/// `@route^block-id`, `@route:block-id`, or a trailing bare `#`) occupies in
/// the original draft.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LocalDestinationMarker {
    pub(crate) start: usize,
    pub(crate) end: usize,
    pub(crate) text: String,
    pub(crate) mode: EditorMode,
    pub(crate) route: Option<String>,
    pub(crate) block_id: Option<String>,
    pub(crate) section: Option<String>,
}

/// One `@...` token resolved for the editor. `requires_body` marks the plain
/// `@route` form, which only routes when body text sits on the other side --
/// the same rule `parse_capture_text_with_clip_control` applies.
#[derive(Debug, Clone, PartialEq, Eq)]
struct MarkerParse {
    mode: EditorMode,
    route: Option<String>,
    section: Option<String>,
    block_id: Option<String>,
    needs: Vec<Need>,
    spans: Vec<Span>,
    requires_body: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum TokenParse {
    Marker(MarkerParse),
    Invalid(Diagnostic),
}

/// Remap one physical line's own tokenizer output into the original
/// multi-line text's byte offsets, so every span an editor receives always
/// indexes the raw text the user is looking at.
fn tokenize_line_with_spans<'a>(line: &RawLine<'a>) -> Vec<Token<'a>> {
    tokenize_with_spans(line.text)
        .into_iter()
        .map(|token| Token {
            text: token.text,
            start: token.start + line.start,
            end: token.end + line.start,
        })
        .collect()
}

/// One physical line's marker resolution for the editor: its remaining body
/// text, the marker it resolved (if any), the terminal schedule/priority/
/// clipboard spans it carries, and any diagnostics raised along the way.
struct LineEditorParse<'a> {
    body: String,
    marker: Option<MarkerParse>,
    marker_text: Option<String>,
    declarations: Vec<Token<'a>>,
    terminal_spans: Vec<Span>,
    diagnostics: Vec<Diagnostic>,
    has_destination_marker: bool,
}

/// Resolve one line's already offset-tagged tokens exactly like
/// `parse_for_editor` resolved its single line before this module became
/// line-aware. `leading` allows a first-token route to win and must only be
/// set for the parent line.
fn parse_editor_line<'a>(
    mut tokens: Vec<Token<'a>>,
    leading: bool,
) -> LineEditorParse<'a> {
    let declarations = take_global_declarations(&mut tokens);
    let (_, marker_spans) = extract_terminal_markers(&mut tokens, true);
    let terminal_spans: Vec<Span> = marker_spans
        .into_iter()
        .map(|(kind, start, end)| Span { start, end, kind })
        .collect();

    let mut diagnostics = Vec::new();
    if let Some(diagnostic) = legacy_bullet_marker_diagnostic(&tokens) {
        diagnostics.push(diagnostic);
    }
    if let Some(diagnostic) =
        pomodoro_note_conflict_diagnostic(&tokens, leading)
    {
        diagnostics.push(diagnostic);
    }

    // The recognized `@...` token leaves the body exactly like execution
    // drops it before joining the remaining tokens with single spaces.
    let selected = select_marker_token(&tokens, leading);
    let has_destination_marker = selected.is_some();
    let marker_index = selected.as_ref().map(|(index, _)| *index);
    let marker_text =
        selected.as_ref().and_then(|(index, parse)| match parse {
            TokenParse::Marker(_) => Some(tokens[*index].text.to_string()),
            TokenParse::Invalid(_) => None,
        });
    let body = tokens
        .iter()
        .enumerate()
        .filter(|(index, _)| Some(*index) != marker_index)
        .map(|(_, token)| token.text)
        .collect::<Vec<_>>()
        .join(" ");

    let marker = match selected {
        Some((_, TokenParse::Marker(marker))) => Some(marker),
        Some((_, TokenParse::Invalid(diagnostic))) => {
            diagnostics.push(diagnostic);
            None
        }
        None => None,
    };

    LineEditorParse {
        body,
        marker,
        marker_text,
        declarations,
        terminal_spans,
        diagnostics,
        has_destination_marker,
    }
}

/// Track which item-wide marker slots earlier lines already resolved, so
/// a later line that resolves the same slot becomes a diagnostic instead of
/// silently overriding or being silently dropped.
#[derive(Default)]
struct SeenMarkers {
    schedule: bool,
    priority: bool,
    clip: bool,
    route: bool,
}

impl SeenMarkers {
    fn absorb_terminal_spans(
        &mut self,
        spans: &[Span],
        diagnostics: &mut Vec<Diagnostic>,
    ) {
        for span in spans {
            let seen = match span.kind {
                SpanKind::Schedule => &mut self.schedule,
                SpanKind::Priority => &mut self.priority,
                SpanKind::Clipboard => &mut self.clip,
                _ => continue,
            };
            if *seen {
                diagnostics.push(duplicate_capture_marker_diagnostic(
                    duplicate_marker_error(terminal_marker_label(span.kind)),
                    (span.start, span.end),
                ));
            }
            *seen = true;
        }
    }

    fn absorb_route(
        &mut self,
        range: Option<(usize, usize)>,
        diagnostics: &mut Vec<Diagnostic>,
    ) -> bool {
        let already_seen = self.route;
        if already_seen && let Some(range) = range {
            diagnostics.push(duplicate_capture_marker_diagnostic(
                duplicate_marker_error("route/mode marker (@route or #)"),
                range,
            ));
        }
        self.route = true;
        !already_seen
    }
}

fn terminal_marker_label(kind: SpanKind) -> &'static str {
    match kind {
        SpanKind::Schedule => "schedule marker (s:<N>)",
        SpanKind::Priority => "priority marker (p:<N>)",
        SpanKind::Clipboard => "clipboard marker (%)",
        _ => "capture marker",
    }
}

fn duplicate_capture_marker_diagnostic(
    message: String,
    range: (usize, usize),
) -> Diagnostic {
    Diagnostic {
        severity: Severity::Error,
        code: "duplicate_capture_marker",
        message,
        range: Some(range),
    }
}

/// Parse in-progress, possibly multi-line capture text for a live editor.
///
/// Unlike [`parse_capture_text_with_clip_control`] this never fails: an
/// incomplete interactive marker (`@`, `@#`, `@route#`, `@:`, `@route:`,
/// `@^`, `@route^`, `@+`, `@route+`, `@@`, `@@route+`, and their legacy `@!` aliases) is a valid editing state,
/// and an invalid marker component -- or line shape -- becomes a diagnostic
/// instead of an error. Tokenization, terminal marker extraction, and
/// marker classification all run through the same functions `bob capture`
/// executes with; `mode`/`route`/`section`/`block_id`/`needs` describe
/// whichever line resolved a marker first, exactly like `bob capture`
/// prefers the first line's leading form and later lines only compose
/// trailing markers, while `sub_bullets` reports every other authored
/// child's normalized body in source order. A `@@` declaration is metadata,
/// not body text; items inherit it unless they have a local destination
/// marker.
pub(crate) fn parse_for_editor(raw_text: &str) -> EditorParse {
    let draft = split_capture_draft(raw_text);
    let mut global_spans = Vec::new();
    let mut global_diagnostics = Vec::new();
    let item_outcomes = draft
        .items
        .iter()
        .map(parse_editor_item)
        .collect::<Vec<_>>();
    let mut declarations = draft.declarations;
    for outcome in &item_outcomes {
        declarations.extend(outcome.declarations.iter().copied());
    }

    let global_destination = parse_editor_global_declarations(
        &declarations,
        &mut global_spans,
        &mut global_diagnostics,
    );
    if !declarations.is_empty() && draft.items.is_empty() {
        global_diagnostics.push(Diagnostic {
            severity: Severity::Error,
            code: "missing_capture_item",
            message: missing_capture_item_error(),
            range: declarations.first().map(|declaration| {
                (declaration.token.start, declaration.token.end)
            }),
        });
    }

    let mut items = item_outcomes
        .into_iter()
        .map(|outcome| outcome.item)
        .collect::<Vec<_>>();
    if let Some(global) =
        global_destination.as_ref().filter(|global| global.inherit)
    {
        for item in &mut items {
            inherit_editor_global_destination(item, global);
        }
    }

    let Some(first) = items.first() else {
        let mut spans = global_spans;
        spans.sort_by_key(|span| (span.start, span.end));
        return EditorParse {
            body: String::new(),
            mode: global_destination
                .as_ref()
                .map(|global| global.mode)
                .unwrap_or(EditorMode::Task),
            route: global_destination
                .as_ref()
                .and_then(|global| global.route.clone()),
            section: None,
            block_id: global_destination
                .as_ref()
                .and_then(|global| global.block_id.clone()),
            needs: global_destination
                .as_ref()
                .map(|global| global.needs.clone())
                .unwrap_or_default(),
            spans,
            diagnostics: global_diagnostics,
            sub_bullets: Vec::new(),
            items,
            global_destination,
        };
    };

    let body = first.body.clone();
    let mode = first.mode;
    let route = first.route.clone();
    let section = first.section.clone();
    let block_id = first.block_id.clone();
    let needs = first.needs.clone();
    let sub_bullets = first.sub_bullets.clone();
    let mut spans = global_spans;
    spans.extend(items.iter().flat_map(|item| item.spans.iter().copied()));
    let mut diagnostics = global_diagnostics;
    diagnostics.extend(
        items
            .iter()
            .flat_map(|item| item.diagnostics.iter().cloned()),
    );
    spans.sort_by_key(|span| (span.start, span.end));

    EditorParse {
        body,
        mode,
        route,
        section,
        block_id,
        needs,
        spans,
        diagnostics,
        sub_bullets,
        items,
        global_destination,
    }
}

fn parse_editor_global_declarations(
    declarations: &[GlobalDeclarationToken<'_>],
    spans: &mut Vec<Span>,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<EditorGlobalDestination> {
    let mut effective = None;
    let first_line = declarations
        .first()
        .map(|declaration| declaration.line_number);
    for (index, declaration) in declarations.iter().enumerate() {
        let parsed = match classify_global_token(&declaration.token) {
            TokenParse::Marker(marker) => {
                spans.extend(marker.spans.clone());
                EditorGlobalDestination {
                    start: declaration.token.start,
                    end: declaration.token.end,
                    line: declaration.line_number,
                    mode: marker.mode,
                    route: marker.route,
                    block_id: marker.block_id,
                    needs: marker.needs,
                    inherit: true,
                }
            }
            TokenParse::Invalid(diagnostic) => {
                diagnostics.push(diagnostic);
                EditorGlobalDestination {
                    start: declaration.token.start,
                    end: declaration.token.end,
                    line: declaration.line_number,
                    mode: EditorMode::Task,
                    route: None,
                    block_id: None,
                    needs: Vec::new(),
                    inherit: false,
                }
            }
        };

        if index == 0 {
            effective = Some(parsed);
        } else {
            diagnostics.push(Diagnostic {
                severity: Severity::Error,
                code: "duplicate_global_destination",
                message: duplicate_global_destination_error(
                    first_line.expect("first declaration"),
                    declaration.line_number,
                ),
                range: Some((declaration.token.start, declaration.token.end)),
            });
        }
    }
    effective
}

fn inherit_editor_global_destination(
    item: &mut EditorItemParse,
    global: &EditorGlobalDestination,
) {
    if item.has_local_destination {
        return;
    }
    item.mode = global.mode;
    item.route = global.route.clone();
    item.section = None;
    item.block_id = global.block_id.clone();
    item.needs = global.needs.clone();
}

struct EditorItemOutcome<'a> {
    item: EditorItemParse,
    declarations: Vec<GlobalDeclarationToken<'a>>,
}

fn parse_editor_item<'a>(item: &CaptureItem<'a>) -> EditorItemOutcome<'a> {
    let mut spans: Vec<Span> = Vec::new();
    let mut diagnostics: Vec<Diagnostic> = Vec::new();
    let mut seen = SeenMarkers::default();
    let mut declarations = Vec::new();
    let mut local_destination_marker = None;
    let mut local_destination_markers = Vec::new();

    let parent_item_line = item.lines.first().expect("nonempty item");
    let parent_line = parent_item_line.raw;
    let parent_tokens = tokenize_line_with_spans(&parent_line);
    let parent_parse = parse_editor_line(parent_tokens, true);
    declarations.extend(global_declarations_from_tokens(
        parent_parse.declarations,
        parent_item_line.line_number,
    ));
    seen.absorb_terminal_spans(&parent_parse.terminal_spans, &mut diagnostics);
    spans.extend(parent_parse.terminal_spans);
    diagnostics.extend(parent_parse.diagnostics);

    let body = parent_parse.body;
    let mut has_local_destination = parent_parse.has_destination_marker;
    if local_destination_marker.is_none() {
        local_destination_marker = parent_parse.marker_text.clone();
    }
    if let Some(marker) = complete_local_destination_marker(
        parent_parse.marker_text.as_deref(),
        parent_parse.marker.as_ref(),
    ) {
        local_destination_markers.push(marker);
    }
    let (mut mode, mut route, mut section, mut block_id, mut needs) =
        match &parent_parse.marker {
            Some(marker) => {
                spans.extend(marker.spans.clone());
                seen.absorb_route(None, &mut diagnostics);
                (
                    marker.mode,
                    marker.route.clone(),
                    marker.section.clone(),
                    marker.block_id.clone(),
                    marker.needs.clone(),
                )
            }
            None => (EditorMode::Task, None, None, None, Vec::new()),
        };

    let mut sub_bullets = Vec::new();
    let mut has_first_level_owner = false;
    for line in item.lines.iter().skip(1) {
        let line_number = line.line_number;
        let raw = line.raw;
        let authored = match classify_authored_line(raw) {
            AuthoredLineClass::EmptyOrPlaceholder => continue,
            AuthoredLineClass::Invalid => {
                diagnostics.push(Diagnostic {
                    severity: Severity::Error,
                    code: "invalid_child_line",
                    message: invalid_child_line_error(line_number),
                    range: Some((raw.start, raw.end)),
                });
                continue;
            }
            AuthoredLineClass::Item(authored) => authored,
        };
        if authored.depth == AuthoredDepth::Nested && !has_first_level_owner {
            diagnostics.push(Diagnostic {
                severity: Severity::Error,
                code: "orphaned_nested_bullet",
                message: orphaned_nested_bullet_error(line_number),
                range: Some((raw.start, raw.end)),
            });
            continue;
        }

        let child_line = RawLine {
            text: authored.body,
            start: authored.body_start,
            end: raw.end,
        };
        let child_tokens = tokenize_line_with_spans(&child_line);
        let child_parse = parse_editor_line(child_tokens, false);
        declarations.extend(global_declarations_from_tokens(
            child_parse.declarations,
            line_number,
        ));

        seen.absorb_terminal_spans(
            &child_parse.terminal_spans,
            &mut diagnostics,
        );
        spans.extend(child_parse.terminal_spans);
        diagnostics.extend(child_parse.diagnostics);

        if child_parse.has_destination_marker {
            has_local_destination = true;
            if local_destination_marker.is_none() {
                local_destination_marker = child_parse.marker_text.clone();
            }
        }
        if let Some(marker) = complete_local_destination_marker(
            child_parse.marker_text.as_deref(),
            child_parse.marker.as_ref(),
        ) {
            local_destination_markers.push(marker);
        }
        if let Some(marker) = &child_parse.marker {
            spans.extend(marker.spans.clone());
            let range = marker.spans.first().map(|span| (span.start, span.end));
            if seen.absorb_route(range, &mut diagnostics) {
                mode = marker.mode;
                route = marker.route.clone();
                section = marker.section.clone();
                block_id = marker.block_id.clone();
                needs = marker.needs.clone();
            }
        }

        if child_parse.body.is_empty() {
            diagnostics.push(Diagnostic {
                severity: Severity::Error,
                code: "empty_child_after_markers",
                message: empty_child_after_markers_error(line_number),
                range: Some((raw.start, raw.end)),
            });
        } else {
            sub_bullets.push(AuthoredSubBullet {
                body: child_parse.body,
                depth: authored.depth,
            });
            if authored.depth == AuthoredDepth::First {
                has_first_level_owner = true;
            }
        }
    }

    if let Some(local_marker) = local_destination_marker.as_deref() {
        for declaration in &declarations {
            diagnostics.push(Diagnostic {
                severity: Severity::Warning,
                code: "global_destination_shadowed",
                message: global_destination_shadowed_warning(
                    local_marker,
                    declaration.token.text,
                ),
                range: Some((declaration.token.start, declaration.token.end)),
            });
        }
    }

    if mode == EditorMode::PomodoroNote {
        for span in &spans {
            let message = match span.kind {
                SpanKind::Schedule => pomodoro_note_schedule_conflict_error(),
                SpanKind::Priority => pomodoro_note_priority_conflict_error(),
                _ => continue,
            };
            diagnostics.push(Diagnostic {
                severity: Severity::Error,
                code: "pomodoro_note_conflict",
                message,
                range: Some((span.start, span.end)),
            });
        }
    }

    spans.sort_by_key(|span| (span.start, span.end));
    EditorItemOutcome {
        item: EditorItemParse {
            index: item.index,
            start: item.start,
            end: item.end,
            line_start: item.line_start,
            line_end: item.line_end,
            body,
            mode,
            route,
            section,
            block_id,
            needs,
            spans,
            diagnostics,
            sub_bullets,
            has_local_destination,
            local_destination_markers,
        },
        declarations,
    }
}

/// Build a [`LocalDestinationMarker`] from one line's resolved marker, when
/// that marker is a real, fully-typed destination. An incomplete marker
/// (still being typed, e.g. a bare `@`) is not one of the six local
/// destination marker forms `rewrite_draft` reasons about, so it is filtered
/// out here rather than at every call site.
fn complete_local_destination_marker(
    marker_text: Option<&str>,
    marker: Option<&MarkerParse>,
) -> Option<LocalDestinationMarker> {
    let marker = marker?;
    if marker.mode == EditorMode::Incomplete {
        return None;
    }
    let text = marker_text?;
    let first = marker.spans.first()?;
    let last = marker.spans.last()?;
    Some(LocalDestinationMarker {
        start: first.start,
        end: last.end,
        text: text.to_string(),
        mode: marker.mode,
        route: marker.route.clone(),
        block_id: marker.block_id.clone(),
        section: marker.section.clone(),
    })
}

pub(crate) fn editor_item_at(
    raw_text: &str,
    cursor: usize,
) -> Option<EditorItemParse> {
    let draft = split_capture_draft(raw_text);
    let index = draft
        .items
        .iter()
        .find(|item| {
            item.lines
                .iter()
                .any(|line| cursor >= line.raw.start && cursor <= line.raw.end)
        })?
        .index;
    parse_for_editor(raw_text)
        .items
        .into_iter()
        .find(|item| item.index == index)
}

/// Mirror `parse_capture_text_with_clip_control`'s precedence: the leading
/// token wins when `leading` is set (only ever true for the parent line),
/// and only a plain `@route` token needs body text on the other side before
/// it routes at all.
fn select_marker_token(
    tokens: &[Token<'_>],
    leading: bool,
) -> Option<(usize, TokenParse)> {
    if leading
        && let Some(first) = tokens.first()
        && let Some(parse) = classify_editor_token(first)
    {
        let requires_body = matches!(
            &parse,
            TokenParse::Marker(marker) if marker.requires_body
        );
        if !(requires_body && tokens.len() == 1) {
            return Some((0, parse));
        }
    }

    if tokens.len() >= 2 {
        let last = tokens.len() - 1;
        if is_pomodoro_note_marker(tokens[last].text) {
            return Some((
                last,
                TokenParse::Marker(pomodoro_note_marker_parse(&tokens[last])),
            ));
        }
        if let Some(parse) = classify_editor_token(&tokens[last]) {
            return Some((last, parse));
        }
    }

    None
}

/// The bare `#` token never has a partially-typed form, needs nothing else,
/// and -- unlike every `@...` marker -- is only ever recognized as the final
/// token of a line, so it is classified directly in
/// [`select_marker_token`]'s trailing slot rather than through
/// [`classify_editor_token`] (which a leading check also consults).
fn pomodoro_note_marker_parse(token: &Token<'_>) -> MarkerParse {
    MarkerParse {
        mode: EditorMode::PomodoroNote,
        route: None,
        section: None,
        block_id: None,
        needs: Vec::new(),
        spans: vec![Span {
            start: token.start,
            end: token.end,
            kind: SpanKind::PomodoroNote,
        }],
        requires_body: false,
    }
}

/// Classify one `@...` token, returning `None` when the token is not
/// route-shaped at all and therefore stays literal body text.
fn classify_global_token(token: &Token<'_>) -> TokenParse {
    let rest = match token.text.strip_prefix("@@") {
        Some(rest) => rest,
        None => {
            return TokenParse::Invalid(token_diagnostic(
                token,
                "invalid_global_destination",
                GLOBAL_DESTINATION_SHAPE_ERROR,
            ));
        }
    };
    if rest.contains('#') || rest.contains('^') || rest.contains(':') {
        return TokenParse::Invalid(token_diagnostic(
            token,
            "invalid_global_destination",
            &unsupported_global_destination_error(token.text),
        ));
    }
    if let Some((route_part, block_part)) = rest.split_once('+') {
        if !route_part.is_empty() && !is_route_token(route_part) {
            return TokenParse::Invalid(token_diagnostic(
                token,
                "invalid_global_destination",
                GLOBAL_DESTINATION_ROUTE_ERROR,
            ));
        }
        if !block_part.is_empty() && !is_block_id(block_part) {
            return TokenParse::Invalid(token_diagnostic(
                token,
                "invalid_global_destination",
                GLOBAL_DESTINATION_BLOCK_ID_ERROR,
            ));
        }
        return TokenParse::Marker(marker_parse(
            token,
            MarkerShape {
                sigil_len: 2,
                route_part,
                separator_len: 1,
                right_part: block_part,
                route_kind: SpanKind::GlobalSubBulletRoute,
                right_kind: SpanKind::GlobalSubBulletBlockId,
                complete_mode: EditorMode::SubBullet,
                right_need: Need::Task,
                third: None,
            },
        ));
    }
    if rest.is_empty() {
        return TokenParse::Marker(MarkerParse {
            mode: EditorMode::Incomplete,
            route: None,
            section: None,
            block_id: None,
            needs: vec![Need::Route],
            spans: vec![Span {
                start: token.start,
                end: token.end,
                kind: SpanKind::InteractivePlaceholder,
            }],
            requires_body: false,
        });
    }
    if !is_route_token(rest) {
        return TokenParse::Invalid(token_diagnostic(
            token,
            "invalid_global_destination",
            GLOBAL_DESTINATION_ROUTE_ERROR,
        ));
    }
    TokenParse::Marker(MarkerParse {
        mode: EditorMode::Task,
        route: Some(rest.to_ascii_lowercase()),
        section: None,
        block_id: None,
        needs: Vec::new(),
        spans: vec![Span {
            start: token.start,
            end: token.end,
            kind: SpanKind::GlobalRoute,
        }],
        requires_body: false,
    })
}

fn classify_editor_token(token: &Token<'_>) -> Option<TokenParse> {
    let text = token.text;
    if !text.starts_with('@') || text.starts_with("@@") {
        return None;
    }
    if is_sub_bullet_marker_candidate(text) {
        return Some(classify_sub_bullet_token(token));
    }
    if is_task_block_id_marker_candidate(text) {
        return Some(classify_task_block_id_token(token));
    }
    if is_retired_double_colon_marker_candidate(text) {
        return Some(classify_retired_double_colon_token(token));
    }
    if is_pomodoro_marker_candidate(text)
        || is_incomplete_pomodoro_marker_candidate(text)
    {
        return Some(classify_pomodoro_token(token));
    }
    classify_route_token(token).map(TokenParse::Marker)
}

/// `@:` and `@:<block-id>` never route in execution because the route is
/// still empty, so `is_pomodoro_marker_candidate` rejects them. They are
/// valid interactive states while the user is picking a target.
fn is_incomplete_pomodoro_marker_candidate(token: &str) -> bool {
    if token.starts_with("@!") {
        return false;
    }
    token
        .strip_prefix('@')
        .is_some_and(|marker| marker.starts_with(':'))
}

fn classify_sub_bullet_token(token: &Token<'_>) -> TokenParse {
    let marker = &token.text[1..];
    let (route_part, rest) =
        marker.split_once('+').expect("sub-bullet candidate");
    let (block_part, section_part) = match rest.split_once('#') {
        Some((block, section)) => (block, Some(section)),
        None => (rest, None),
    };

    if !route_part.is_empty() && !is_route_token(route_part) {
        return TokenParse::Invalid(token_diagnostic(
            token,
            "invalid_sub_bullet_route",
            SUB_BULLET_ROUTE_ERROR,
        ));
    }
    if !block_part.is_empty() && !is_block_id(block_part) {
        return TokenParse::Invalid(token_diagnostic(
            token,
            "invalid_sub_bullet_block_id",
            SUB_BULLET_BLOCK_ID_ERROR,
        ));
    }
    if section_part.is_some_and(|section| {
        !section.is_empty() && !is_selector_component(section)
    }) {
        return TokenParse::Invalid(token_diagnostic(
            token,
            "invalid_sub_bullet_section",
            SUB_BULLET_SECTION_ERROR,
        ));
    }

    TokenParse::Marker(marker_parse(
        token,
        MarkerShape {
            sigil_len: 1,
            route_part,
            separator_len: 1,
            right_part: block_part,
            route_kind: SpanKind::SubBulletRoute,
            right_kind: SpanKind::SubBulletBlockId,
            complete_mode: EditorMode::SubBullet,
            right_need: Need::Task,
            third: section_part.map(|part| MarkerThird {
                separator_len: 1,
                part,
                kind: SpanKind::SubBulletSection,
                need: Need::TaskSection,
            }),
        },
    ))
}

fn classify_task_block_id_token(token: &Token<'_>) -> TokenParse {
    let marker = &token.text[1..];
    let (route_part, block_part) =
        marker.split_once('^').expect("task block-ID candidate");

    if !route_part.is_empty() && !is_route_token(route_part) {
        return TokenParse::Invalid(token_diagnostic(
            token,
            "invalid_task_block_id_route",
            TASK_BLOCK_ID_ROUTE_ERROR,
        ));
    }
    if !block_part.is_empty() && !is_block_id(block_part) {
        return TokenParse::Invalid(token_diagnostic(
            token,
            "invalid_task_block_id",
            TASK_BLOCK_ID_ERROR,
        ));
    }

    TokenParse::Marker(marker_parse(
        token,
        MarkerShape {
            sigil_len: 1,
            route_part,
            separator_len: 1,
            right_part: block_part,
            route_kind: SpanKind::TaskBlockIdRoute,
            right_kind: SpanKind::TaskBlockId,
            complete_mode: EditorMode::Task,
            right_need: Need::BlockId,
            third: None,
        },
    ))
}

fn classify_retired_double_colon_token(token: &Token<'_>) -> TokenParse {
    TokenParse::Invalid(token_diagnostic(
        token,
        "retired_task_block_id_marker",
        RETIRED_DOUBLE_COLON_ERROR,
    ))
}

fn classify_pomodoro_token(token: &Token<'_>) -> TokenParse {
    let legacy = token.text.starts_with("@!");
    let sigil_len = if legacy { 2 } else { 1 };
    let marker = &token.text[sigil_len..];
    let (route_part, block_part, separator) = match marker.split_once(':') {
        Some((route, block_id)) => (route, block_id, true),
        None => (marker, "", false),
    };

    if legacy && separator && route_part.is_empty() {
        // `@!:id` has never been a legal shorthand: the Hammerspoon grammar
        // and `bob capture` both require a route before the colon.
        return TokenParse::Invalid(token_diagnostic(
            token,
            "invalid_pomodoro_route",
            POMODORO_ROUTE_ERROR,
        ));
    }
    if !route_part.is_empty() && !is_route_token(route_part) {
        return TokenParse::Invalid(token_diagnostic(
            token,
            "invalid_pomodoro_route",
            POMODORO_ROUTE_ERROR,
        ));
    }
    if !block_part.is_empty() && !is_block_id(block_part) {
        return TokenParse::Invalid(token_diagnostic(
            token,
            "invalid_pomodoro_block_id",
            POMODORO_BLOCK_ID_ERROR,
        ));
    }

    TokenParse::Marker(marker_parse(
        token,
        MarkerShape {
            sigil_len,
            route_part,
            separator_len: usize::from(separator),
            right_part: block_part,
            route_kind: SpanKind::PomodoroRoute,
            right_kind: SpanKind::PomodoroBlockId,
            complete_mode: EditorMode::PomodoroTask,
            right_need: Need::PomodoroId,
            third: None,
        },
    ))
}

/// Classify the remaining `@` forms: a bare `@`, the `@#`/`@#prefix` target
/// pickers, `@route#`/`@route#prefix` bullets, and a plain `@route` task.
fn classify_route_token(token: &Token<'_>) -> Option<MarkerParse> {
    let rest = token.text.strip_prefix('@')?;

    let Some((route_part, prefix)) = rest.split_once('#') else {
        if rest.is_empty() {
            return Some(MarkerParse {
                mode: EditorMode::Incomplete,
                route: None,
                section: None,
                block_id: None,
                needs: vec![Need::Route],
                spans: vec![Span {
                    start: token.start,
                    end: token.end,
                    kind: SpanKind::InteractivePlaceholder,
                }],
                requires_body: false,
            });
        }
        if !is_route_token(rest) {
            return None;
        }
        return Some(MarkerParse {
            mode: EditorMode::Task,
            route: Some(rest.to_ascii_lowercase()),
            section: None,
            block_id: None,
            needs: Vec::new(),
            spans: vec![Span {
                start: token.start,
                end: token.end,
                kind: SpanKind::Route,
            }],
            requires_body: true,
        });
    };

    if !route_part.is_empty() && !is_route_token(route_part) {
        // `@bad.route#x` never routed and is not an interactive state either,
        // so it stays literal exactly like `bob capture` leaves it.
        return None;
    }

    Some(marker_parse(
        token,
        MarkerShape {
            sigil_len: 1,
            route_part,
            separator_len: 1,
            right_part: prefix,
            route_kind: SpanKind::Route,
            right_kind: SpanKind::Section,
            complete_mode: EditorMode::Bullet,
            right_need: Need::Section,
            third: None,
        },
    ))
}

/// The shared shape of every `@<route><separator><right>` marker, with an
/// optional third component for `@route+block-id#section`.
struct MarkerShape<'a> {
    sigil_len: usize,
    route_part: &'a str,
    separator_len: usize,
    right_part: &'a str,
    route_kind: SpanKind,
    right_kind: SpanKind,
    complete_mode: EditorMode,
    right_need: Need,
    third: Option<MarkerThird<'a>>,
}

struct MarkerThird<'a> {
    separator_len: usize,
    part: &'a str,
    kind: SpanKind,
    need: Need,
}

/// Build the mode, needs, and spans for one marker from its component parts.
///
/// Spans never overlap and always sit on `char` boundaries. When a component
/// is still empty its sigil and separator become one
/// `interactive_placeholder` span so an editor can highlight the caret
/// position the user still has to fill in.
fn marker_parse(token: &Token<'_>, shape: MarkerShape<'_>) -> MarkerParse {
    let route_end = token.start + shape.sigil_len + shape.route_part.len();
    let has_route = !shape.route_part.is_empty();
    let has_right = !shape.right_part.is_empty();
    let has_third_sep = shape.third.is_some();
    let third_part = shape.third.as_ref().map(|third| third.part).unwrap_or("");
    let third_sep_len = shape
        .third
        .as_ref()
        .map(|third| third.separator_len)
        .unwrap_or(0);
    let has_third = has_third_sep && !third_part.is_empty();
    let right_start = route_end + shape.separator_len;
    let right_end = right_start + shape.right_part.len();
    let third_start = right_end + third_sep_len;

    let mut spans = Vec::new();
    if has_route {
        spans.push(Span {
            start: token.start,
            end: route_end,
            kind: shape.route_kind,
        });
        if !has_right && shape.separator_len > 0 {
            spans.push(Span {
                start: route_end,
                end: right_start,
                kind: SpanKind::InteractivePlaceholder,
            });
        }
    } else {
        let placeholder_end = route_end + shape.separator_len;
        spans.push(Span {
            start: token.start,
            end: placeholder_end,
            kind: SpanKind::InteractivePlaceholder,
        });
    }
    if has_right {
        spans.push(Span {
            start: right_start,
            end: right_end,
            kind: shape.right_kind,
        });
    }
    if let Some(third) = shape.third.as_ref() {
        if third.part.is_empty() {
            spans.push(Span {
                start: right_end,
                end: third_start,
                kind: SpanKind::InteractivePlaceholder,
            });
        } else {
            spans.push(Span {
                start: third_start,
                end: token.end,
                kind: third.kind,
            });
        }
    }

    // A `@route#` bullet is executable today (it means "any non-Tasks
    // section"), so it keeps its complete mode while still reporting the
    // section it could still resolve. A section can only be offered once the
    // route that owns its headings is known. A trailing `#` on a sub-bullet
    // marker is required once typed: `@route+id#` is incomplete.
    let section_is_optional =
        shape.right_need == Need::Section && !has_third_sep;
    let third_ok = !has_third_sep || has_third;

    let mut needs = Vec::new();
    if !has_route {
        needs.push(Need::Route);
    }
    if !has_right && (has_route || !section_is_optional) {
        needs.push(shape.right_need);
    }
    if let Some(third) = shape.third.as_ref()
        && third.part.is_empty()
    {
        needs.push(third.need);
    }

    let mode = if has_route && (has_right || section_is_optional) && third_ok {
        shape.complete_mode
    } else {
        EditorMode::Incomplete
    };

    let section = if has_third {
        Some(third_part.to_string())
    } else if shape.right_need == Need::Section && has_right {
        Some(shape.right_part.to_string())
    } else {
        None
    };

    MarkerParse {
        mode,
        route: has_route.then(|| shape.route_part.to_ascii_lowercase()),
        section,
        block_id: (shape.right_need != Need::Section && has_right)
            .then(|| shape.right_part.to_string()),
        needs,
        spans,
        requires_body: false,
    }
}

fn token_diagnostic(
    token: &Token<'_>,
    code: &'static str,
    message: &str,
) -> Diagnostic {
    Diagnostic {
        severity: Severity::Error,
        code,
        message: message.to_string(),
        range: Some((token.start, token.end)),
    }
}

/// Surface the retired standalone `#...` bullet marker as a diagnostic. The
/// rule itself is [`reject_legacy_bullet_markers`], so the editor and
/// `bob capture` can never disagree about which inputs are affected.
fn legacy_bullet_marker_diagnostic(tokens: &[Token<'_>]) -> Option<Diagnostic> {
    let texts: Vec<&str> = tokens.iter().map(|token| token.text).collect();
    let message = reject_legacy_bullet_markers(&texts, true).err()?;
    let last = tokens.last()?;
    let offender = if last.text.starts_with('#') {
        last
    } else {
        tokens.get(tokens.len().checked_sub(2)?)?
    };

    Some(Diagnostic {
        severity: Severity::Error,
        code: "legacy_bullet_marker",
        message,
        range: Some((offender.start, offender.end)),
    })
}

/// Diagnose a same-line combination of the bare `#` Pomodoro-note marker
/// with an `@route`-shaped marker, mirroring [`resolve_line`]'s route
/// conflict rejection so the editor and `bob capture` never disagree.
/// Schedule (`s:<N>`) and priority (`p:<N>`) conflicts are item-wide and
/// diagnosed separately in [`parse_editor_item`] once the whole item's mode
/// is known; a forced `--route` conflict never reaches the editor, which has
/// no forced-route flag.
fn pomodoro_note_conflict_diagnostic(
    tokens: &[Token<'_>],
    leading: bool,
) -> Option<Diagnostic> {
    let texts: Vec<&str> = tokens.iter().map(|token| token.text).collect();
    if !texts.contains(&"#") {
        return None;
    }
    let hash = tokens.iter().find(|token| token.text == "#")?;

    let conflict = match texts.last() {
        Some(&"#") => {
            let remaining = &texts[..texts.len() - 1];
            (leading
                && remaining
                    .first()
                    .is_some_and(|token| is_route_marker(token)))
                || remaining.last().is_some_and(|token| is_route_marker(token))
        }
        Some(&last) => {
            (leading
                && texts.first().is_some_and(|token| is_route_marker(token)))
                || is_route_marker(last)
        }
        None => false,
    };

    conflict.then(|| Diagnostic {
        severity: Severity::Error,
        code: "pomodoro_note_conflict",
        message: pomodoro_note_route_conflict_error(),
        range: Some((hash.start, hash.end)),
    })
}

// ---------------------------------------------------------------------------
// Cursor-aware completion
// ---------------------------------------------------------------------------

/// Which discovery source a completion request should query.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CompletionContext {
    Route,
    Section,
    PomodoroBlockId,
    Task,
    TaskSection,
    WikilinkNote,
    WikilinkHeading,
    WikilinkBlock,
}

/// The active marker component at one cursor position, ready for a
/// discovery scan and case-insensitive prefix/substring ranking.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CompletionField {
    pub(crate) context: CompletionContext,
    /// The already-resolved, lowercased route, when `context` needs one
    /// (`section`, `pomodoro_block_id`, `task`, and `task_section`).
    pub(crate) route: Option<String>,
    /// The already-typed block ID of a three-component
    /// `@route+id#section` marker. Set only when `context` is
    /// `task_section`; always `None` for existing contexts.
    pub(crate) block_id: Option<String>,
    /// The text already typed in this component, up to `cursor`.
    pub(crate) query: String,
    /// The half-open UTF-8 byte range of the whole component, which a
    /// completion replaces in full regardless of where the cursor sits
    /// inside it.
    pub(crate) replacement: (usize, usize),
}

struct CompletionThird<'a> {
    separator_len: usize,
    part: &'a str,
    context: CompletionContext,
}

struct CompletionParts<'a> {
    sigil_len: usize,
    route_part: &'a str,
    separator_len: usize,
    right_part: &'a str,
    right_context: Option<CompletionContext>,
    third: Option<CompletionThird<'a>>,
}

/// Identify the completable marker component at `cursor`, reusing the same
/// tokenizer, terminal-marker extraction, and `@token` candidate detection
/// as [`parse_for_editor`]. Returns `None` when the cursor is not inside an
/// eligible leading or trailing `@` marker: plain body text, a token in the
/// middle of the input, and an unrecognized or invalid marker never produce
/// a completion field.
///
/// Multi-line drafts complete the physical line the cursor is on: only the
/// first (parent) line offers a leading marker, and a later line's source
/// indentation plus bullet marker itself is never completable, matching the
/// authored-bullet grammar `bob capture` and `bob capture-parse` execute
/// with.
pub(crate) fn completion_field_at(
    raw_text: &str,
    cursor: usize,
) -> Option<CompletionField> {
    if let Some(line) = split_physical_lines(raw_text)
        .into_iter()
        .find(|line| cursor >= line.start && cursor <= line.end)
    {
        let tokens = tokenize_line_with_spans(&line);
        if let Some(token) = tokens.iter().find(|token| {
            token.text.starts_with("@@")
                && cursor >= token.start
                && cursor <= token.end
        }) {
            return global_completion_field_at(token, cursor);
        }
    }

    let draft = split_capture_draft(raw_text);
    let items = draft.items;
    let (item, line_index, line) = items.iter().find_map(|item| {
        item.lines
            .iter()
            .enumerate()
            .find(|(_, line)| {
                cursor >= line.raw.start && cursor <= line.raw.end
            })
            .map(|(line_index, line)| (item, line_index, line.raw))
    })?;
    let leading = line_index == 0;

    let scan_line = if leading {
        line
    } else {
        let AuthoredLineClass::Item(authored) = classify_authored_line(line)
        else {
            return None;
        };
        if authored.depth == AuthoredDepth::Nested
            && !has_previous_first_level_authored_item(&item.lines, line_index)
        {
            return None;
        }
        if cursor < authored.body_start {
            return None;
        }
        RawLine {
            text: authored.body,
            start: authored.body_start,
            end: line.end,
        }
    };

    let mut tokens = tokenize_line_with_spans(&scan_line);
    take_global_declarations(&mut tokens);
    extract_terminal_markers(&mut tokens, true);

    let index = completion_marker_index(&tokens, leading)?;
    let token = tokens[index];
    if cursor < token.start || cursor > token.end {
        return None;
    }

    marker_field_at_cursor(&token, cursor)
}

fn has_previous_first_level_authored_item(
    lines: &[ItemLine<'_>],
    current_line_index: usize,
) -> bool {
    lines[1..current_line_index].iter().any(|line| {
        let AuthoredLineClass::Item(authored) =
            classify_authored_line(line.raw)
        else {
            return false;
        };
        if authored.depth != AuthoredDepth::First {
            return false;
        }
        let normalized = normalize_task_text(authored.body);
        if normalized.is_empty() {
            return false;
        }
        let tokens = tokenize_with_spans(&normalized);
        !parse_editor_line(tokens, false).body.is_empty()
    })
}

/// Mirror [`select_marker_token`]'s leading-then-trailing precedence, but
/// without its requires-body/single-token exclusion: a lone leading
/// `@route` fragment with no body text yet is still the token a user is
/// actively completing, even though `bob capture` would leave it literal.
/// `leading` is only ever set for the parent (first) physical line.
fn completion_marker_index(
    tokens: &[Token<'_>],
    leading: bool,
) -> Option<usize> {
    if leading
        && let Some(first) = tokens.first()
        && classify_editor_token(first).is_some()
    {
        return Some(0);
    }

    if tokens.len() >= 2 {
        let last = tokens.len() - 1;
        if classify_editor_token(&tokens[last]).is_some() {
            return Some(last);
        }
    }

    None
}

/// Split one `@`-token the same way [`classify_route_token`],
/// [`classify_pomodoro_token`], and [`classify_sub_bullet_token`] do, then
/// resolve which component -- route or right-hand -- `cursor` sits in.
fn marker_field_at_cursor(
    token: &Token<'_>,
    cursor: usize,
) -> Option<CompletionField> {
    let text = token.text;

    if is_sub_bullet_marker_candidate(text) {
        let marker = &text[1..];
        let (route_part, rest) =
            marker.split_once('+').expect("sub-bullet candidate");
        let (block_part, third) = match rest.split_once('#') {
            Some((block, section)) => (
                block,
                Some(CompletionThird {
                    separator_len: 1,
                    part: section,
                    context: CompletionContext::TaskSection,
                }),
            ),
            None => (rest, None),
        };
        return completion_field_from_parts(
            token,
            CompletionParts {
                sigil_len: 1,
                route_part,
                separator_len: 1,
                right_part: block_part,
                right_context: Some(CompletionContext::Task),
                third,
            },
            cursor,
        );
    }

    if is_task_block_id_marker_candidate(text) {
        let marker = &text[1..];
        let (route_part, block_part) =
            marker.split_once('^').expect("task block-ID candidate");
        return completion_field_from_parts(
            token,
            CompletionParts {
                sigil_len: 1,
                route_part,
                separator_len: 1,
                right_part: block_part,
                right_context: None,
                third: None,
            },
            cursor,
        );
    }

    if is_retired_double_colon_marker_candidate(text) {
        return None;
    }

    if is_pomodoro_marker_candidate(text)
        || is_incomplete_pomodoro_marker_candidate(text)
    {
        let legacy = text.starts_with("@!");
        let sigil_len = if legacy { 2 } else { 1 };
        let marker = &text[sigil_len..];
        let (route_part, block_part, separator) = match marker.split_once(':') {
            Some((route, block_id)) => (route, block_id, true),
            None => (marker, "", false),
        };
        return completion_field_from_parts(
            token,
            CompletionParts {
                sigil_len,
                route_part,
                separator_len: usize::from(separator),
                right_part: block_part,
                right_context: Some(CompletionContext::PomodoroBlockId),
                third: None,
            },
            cursor,
        );
    }

    let rest = text.strip_prefix('@')?;
    if let Some((route_part, prefix)) = rest.split_once('#') {
        return completion_field_from_parts(
            token,
            CompletionParts {
                sigil_len: 1,
                route_part,
                separator_len: 1,
                right_part: prefix,
                right_context: Some(CompletionContext::Section),
                third: None,
            },
            cursor,
        );
    }

    // A bare `@` or a still-typing `@fragment` with no separator yet: the
    // whole remainder is the route component, and there is no right-hand
    // component to fall into.
    completion_field_from_parts(
        token,
        CompletionParts {
            sigil_len: 1,
            route_part: rest,
            separator_len: 0,
            right_part: "",
            right_context: Some(CompletionContext::Route),
            third: None,
        },
        cursor,
    )
}

fn global_completion_field_at(
    token: &Token<'_>,
    cursor: usize,
) -> Option<CompletionField> {
    if cursor < token.start || cursor > token.end {
        return None;
    }
    let rest = token.text.strip_prefix("@@")?;
    if let Some(cut) = rest.find(['#', '^', ':']) {
        if cursor > token.start + 2 + cut {
            return None;
        }
        return completion_field_from_parts(
            token,
            CompletionParts {
                sigil_len: 2,
                route_part: &rest[..cut],
                separator_len: 0,
                right_part: "",
                right_context: Some(CompletionContext::Route),
                third: None,
            },
            cursor,
        );
    }
    if let Some((route_part, block_part)) = rest.split_once('+') {
        return completion_field_from_parts(
            token,
            CompletionParts {
                sigil_len: 2,
                route_part,
                separator_len: 1,
                right_part: block_part,
                right_context: Some(CompletionContext::Task),
                third: None,
            },
            cursor,
        );
    }
    completion_field_from_parts(
        token,
        CompletionParts {
            sigil_len: 2,
            route_part: rest,
            separator_len: 0,
            right_part: "",
            right_context: Some(CompletionContext::Route),
            third: None,
        },
        cursor,
    )
}

/// Build the completion field for one decomposed `@<route><sep><right>`
/// marker, given which side of the (possible) separator `cursor` lands on.
/// The route component spans exactly the route text, excluding the leading
/// sigil; the right component spans exactly its text, excluding the
/// separator. An optional third component is the same: `#` is never part of
/// a replacement range, so a cursor after `#` on `@route+id#` is a
/// zero-length `task_section` replacement at the insertion point. Each
/// component stays well-defined -- and empty -- when its text has not been
/// typed yet.
fn completion_field_from_parts(
    token: &Token<'_>,
    parts: CompletionParts<'_>,
    cursor: usize,
) -> Option<CompletionField> {
    let route_start = token.start + parts.sigil_len;
    let route_end = route_start + parts.route_part.len();

    if parts.separator_len == 0 || cursor <= route_end {
        let split = cursor.clamp(route_start, route_end) - route_start;
        return Some(CompletionField {
            context: CompletionContext::Route,
            route: None,
            block_id: None,
            query: parts.route_part[..split].to_string(),
            replacement: (route_start, route_end),
        });
    }

    let right_start = route_end + parts.separator_len;
    if cursor < right_start {
        return None;
    }

    let right_end = right_start + parts.right_part.len();
    let in_right = parts.third.is_none() || cursor <= right_end;
    if in_right {
        // Past the first separator: complete the middle component when that
        // component is backed by a discovery source. Authored ID-only task
        // block IDs intentionally have no right-hand completion source.
        let right_context = parts.right_context?;
        // The right-hand component only makes sense once the route it
        // belongs to already resolves.
        if !is_route_token(parts.route_part) {
            return None;
        }
        let split = cursor.clamp(right_start, right_end) - right_start;
        return Some(CompletionField {
            context: right_context,
            route: Some(parts.route_part.to_ascii_lowercase()),
            block_id: None,
            query: parts.right_part[..split].to_string(),
            replacement: (right_start, right_end),
        });
    }

    let third = parts.third?;
    let third_start = right_end + third.separator_len;
    if cursor < third_start {
        return None;
    }
    if !is_route_token(parts.route_part) {
        return None;
    }
    let third_end = third_start + third.part.len();
    let split = cursor.clamp(third_start, third_end) - third_start;
    Some(CompletionField {
        context: third.context,
        route: Some(parts.route_part.to_ascii_lowercase()),
        block_id: (!parts.right_part.is_empty())
            .then(|| parts.right_part.to_string()),
        query: third.part[..split].to_string(),
        replacement: (third_start, third_end),
    })
}

// ---------------------------------------------------------------------------
// Rule A1-A6: `bob capture-rewrite`'s bare `@@` absorption
// ---------------------------------------------------------------------------

/// The result of applying the capture grammar's automatic draft rewrites to
/// `raw_text`. See [`rewrite_draft`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DraftRewrite {
    /// `None` when nothing changed.
    pub(crate) rule: Option<RewriteRule>,
    /// Sorted, non-overlapping edits into the original `raw_text`.
    pub(crate) edits: Vec<TextEdit>,
    /// `raw_text` with every edit applied; equals `raw_text` when `rule` is
    /// `None`.
    pub(crate) text: String,
    /// `Some` exactly when a cursor was supplied, mapped through the edits.
    pub(crate) cursor: Option<usize>,
    /// A short human sentence describing what changed; `None` when `rule` is
    /// `None`.
    pub(crate) summary: Option<String>,
    /// Rule A5 (and future) explanations for why no rewrite happened.
    pub(crate) notices: Vec<String>,
}

/// One `[start, end)` replacement into the original `raw_text`. Applying
/// every edit left-to-right yields [`DraftRewrite::text`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TextEdit {
    pub(crate) start: usize,
    pub(crate) end: usize,
    pub(crate) replacement: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RewriteRule {
    AbsorbLocalMarker,
    AbsorbDeclaration,
}

impl RewriteRule {
    pub(crate) fn code(self) -> &'static str {
        match self {
            Self::AbsorbLocalMarker => "absorb_local_marker",
            Self::AbsorbDeclaration => "absorb_declaration",
        }
    }
}

/// Apply Rule A1's absorption to the bare `@@` at (or before, when no cursor
/// is given, the last one in source order at) `cursor`: claim the item's own
/// absorbable local destination marker, or else the draft's one other
/// declaration token, rewriting the bare token to `@@<payload>` and deleting
/// the token(s) it absorbed. Never fails; an input with no eligible bare
/// `@@` -- or whose local marker cannot be expressed as a declaration
/// (Rule A5), or whose item already has more than one local marker
/// (Rule A6) -- returns `rule: None` with `text` unchanged.
pub(crate) fn rewrite_draft(
    raw_text: &str,
    cursor: Option<usize>,
) -> DraftRewrite {
    let draft = split_capture_draft(raw_text);
    let item_outcomes: Vec<EditorItemOutcome<'_>> =
        draft.items.iter().map(parse_editor_item).collect();

    let mut occurrences: Vec<DeclarationOccurrence<'_>> = draft
        .declarations
        .iter()
        .map(|declaration| DeclarationOccurrence {
            token: declaration.token,
            owner_item: None,
        })
        .collect();
    for (item_index, outcome) in item_outcomes.iter().enumerate() {
        for declaration in &outcome.declarations {
            occurrences.push(DeclarationOccurrence {
                token: declaration.token,
                owner_item: Some(item_index),
            });
        }
    }
    occurrences.sort_by_key(|occurrence| occurrence.token.start);

    let Some(selected_index) = select_bare_declaration(&occurrences, cursor)
    else {
        return unchanged_rewrite(raw_text, cursor, Vec::new());
    };

    if let Some(item_index) = occurrences[selected_index].owner_item {
        match item_outcomes[item_index]
            .item
            .local_destination_markers
            .as_slice()
        {
            [] => {}
            [marker] => {
                return match classify_local_marker(marker) {
                    LocalMarkerAbsorbability::Absorbable(payload) => {
                        finish_absorption(
                            raw_text,
                            cursor,
                            &draft,
                            &occurrences,
                            selected_index,
                            RewriteRule::AbsorbLocalMarker,
                            &payload,
                            Some((marker.start, marker.end)),
                            absorb_local_marker_summary(&marker.text, &payload),
                        )
                    }
                    LocalMarkerAbsorbability::NonAbsorbable => {
                        unchanged_rewrite(
                            raw_text,
                            cursor,
                            vec![non_absorbable_marker_notice(marker)],
                        )
                    }
                };
            }
            // Rule A6: two or more local markers already put the draft in a
            // duplicate-marker error state that `capture-parse` reports.
            _ => return unchanged_rewrite(raw_text, cursor, Vec::new()),
        }
    }

    // Source 2: the draft's one other declaration token, when it carries a
    // payload of its own.
    let others: Vec<usize> = (0..occurrences.len())
        .filter(|&index| index != selected_index)
        .collect();
    if let [other_index] = others[..] {
        let other_token = occurrences[other_index].token;
        if other_token.text != "@@" {
            let payload = other_token.text["@@".len()..].to_string();
            return finish_absorption(
                raw_text,
                cursor,
                &draft,
                &occurrences,
                selected_index,
                RewriteRule::AbsorbDeclaration,
                &payload,
                None,
                absorb_declaration_summary(&payload),
            );
        }
    }

    unchanged_rewrite(raw_text, cursor, Vec::new())
}

fn unchanged_rewrite(
    raw_text: &str,
    cursor: Option<usize>,
    notices: Vec<String>,
) -> DraftRewrite {
    DraftRewrite {
        rule: None,
        edits: Vec::new(),
        text: raw_text.to_string(),
        cursor,
        summary: None,
        notices,
    }
}

/// One `@@...` declaration token found anywhere in the draft, tagged with
/// the item it sits inside when it is not on a declaration-only line.
struct DeclarationOccurrence<'a> {
    token: Token<'a>,
    owner_item: Option<usize>,
}

/// Select the bare `@@` Rule A1 absorbs into: the one containing or ending
/// at `cursor` when a cursor is given, otherwise the last one in source
/// order. Returns an index into `occurrences`.
fn select_bare_declaration(
    occurrences: &[DeclarationOccurrence<'_>],
    cursor: Option<usize>,
) -> Option<usize> {
    let bare_indices = occurrences
        .iter()
        .enumerate()
        .filter(|(_, occurrence)| occurrence.token.text == "@@")
        .map(|(index, _)| index);

    match cursor {
        Some(position) => bare_indices.into_iter().find(|&index| {
            let token = occurrences[index].token;
            position >= token.start && position <= token.end
        }),
        None => bare_indices.into_iter().next_back(),
    }
}

enum LocalMarkerAbsorbability {
    Absorbable(String),
    NonAbsorbable,
}

/// Classify Rule A1's ordered payload source 1: `mode`/`block_id`/`section`
/// close over the six local destination marker forms the Vocabulary section
/// defines, so this match is exhaustive over real (non-incomplete) markers.
fn classify_local_marker(
    marker: &LocalDestinationMarker,
) -> LocalMarkerAbsorbability {
    match marker.mode {
        EditorMode::Task if marker.block_id.is_none() => {
            LocalMarkerAbsorbability::Absorbable(
                marker.route.clone().unwrap_or_default(),
            )
        }
        EditorMode::SubBullet if marker.section.is_none() => {
            let mut payload = marker.route.clone().unwrap_or_default();
            if let Some(block_id) = &marker.block_id {
                payload.push('+');
                payload.push_str(block_id);
            }
            LocalMarkerAbsorbability::Absorbable(payload)
        }
        EditorMode::Task
        | EditorMode::SubBullet
        | EditorMode::Bullet
        | EditorMode::PomodoroTask
        | EditorMode::PomodoroNote => LocalMarkerAbsorbability::NonAbsorbable,
        EditorMode::Incomplete => {
            unreachable!("complete_local_destination_marker filters these out")
        }
    }
}

/// Rule A5: explain why `@@` cannot take this item's single local marker.
fn non_absorbable_marker_notice(marker: &LocalDestinationMarker) -> String {
    let route = marker.route.as_deref().unwrap_or("route");
    match marker.mode {
        EditorMode::Bullet | EditorMode::SubBullet => format!(
            "@@ cannot take a section: leave {} on this item, or delete it and declare @@{route}",
            marker.text
        ),
        EditorMode::Task => format!(
            "@@ cannot take a block ID: leave {} on this item, or delete it and declare @@{route}",
            marker.text
        ),
        EditorMode::PomodoroTask => format!(
            "@@ cannot take a Pomodoro link: leave {} on this item, or delete it and declare @@{route}",
            marker.text
        ),
        EditorMode::PomodoroNote => format!(
            "@@ cannot take a Pomodoro note: leave {} on this item, or delete it",
            marker.text
        ),
        EditorMode::Incomplete => {
            unreachable!("complete_local_destination_marker filters these out")
        }
    }
}

fn absorb_local_marker_summary(marker_text: &str, payload: &str) -> String {
    format!("Moved {marker_text} into @@{payload}")
}

fn absorb_declaration_summary(payload: &str) -> String {
    format!("Moved the @@{payload} declaration here")
}

/// Build every edit Rule A1's absorption needs: the bare `@@` becomes
/// `@@<payload>`, every other declaration token in the draft is deleted, and
/// -- for `AbsorbLocalMarker` only -- `extra_deletion` (the local marker's
/// own span, which is not itself a declaration token) is deleted too.
#[allow(clippy::too_many_arguments)]
fn finish_absorption(
    raw_text: &str,
    cursor: Option<usize>,
    draft: &CaptureDraft<'_>,
    occurrences: &[DeclarationOccurrence<'_>],
    selected_index: usize,
    rule: RewriteRule,
    payload: &str,
    extra_deletion: Option<(usize, usize)>,
    summary: String,
) -> DraftRewrite {
    let selected_token = occurrences[selected_index].token;
    let replacement = format!("@@{payload}");

    let mut edits = vec![TextEdit {
        start: selected_token.start,
        end: selected_token.end,
        replacement: replacement.clone(),
    }];

    for (index, occurrence) in occurrences.iter().enumerate() {
        if index == selected_index {
            continue;
        }
        edits.extend(deletion_edits_for_token(
            raw_text,
            draft,
            (occurrence.token.start, occurrence.token.end),
        ));
    }
    if let Some(span) = extra_deletion {
        edits.extend(deletion_edits_for_token(raw_text, draft, span));
    }

    edits.sort_by_key(|edit| edit.start);
    debug_assert!(
        edits.windows(2).all(|pair| pair[0].end <= pair[1].start),
        "rewrite_draft edits must not overlap: {edits:?}"
    );

    let text = apply_text_edits(raw_text, &edits);
    let cursor = cursor.map(|_| {
        mapped_cursor_after(&edits, selected_token.start, replacement.len())
    });

    DraftRewrite {
        rule: Some(rule),
        edits,
        text,
        cursor,
        summary: Some(summary),
        notices: Vec::new(),
    }
}

fn apply_text_edits(raw_text: &str, edits: &[TextEdit]) -> String {
    let mut result = String::with_capacity(raw_text.len());
    let mut cursor = 0usize;
    for edit in edits {
        result.push_str(&raw_text[cursor..edit.start]);
        result.push_str(&edit.replacement);
        cursor = edit.end;
    }
    result.push_str(&raw_text[cursor..]);
    result
}

/// Map `replace_start` (the position of the replaced bare `@@` in the
/// original text) through every edit that lands before it, then add
/// `replacement_len` so the result sits just past the rewritten
/// `@@<payload>` token, per Rule A1's cursor contract.
fn mapped_cursor_after(
    edits: &[TextEdit],
    replace_start: usize,
    replacement_len: usize,
) -> usize {
    let mut delta: i64 = 0;
    for edit in edits {
        if edit.end <= replace_start {
            delta +=
                edit.replacement.len() as i64 - (edit.end - edit.start) as i64;
        }
    }
    (replace_start as i64 + delta) as usize + replacement_len
}

/// One physical line's byte bounds, plus the `[content_start, content_end)`
/// sub-range `deletion_edits_for_token` tokenizes: the whole line for a
/// parent or declaration-only line, or the authored body after its bullet
/// marker for a child line.
struct DeletionLineContext<'a> {
    physical: RawLine<'a>,
    content_start: usize,
    content_end: usize,
}

fn deletion_line_context<'a>(
    raw_text: &'a str,
    draft: &CaptureDraft<'a>,
    target: (usize, usize),
) -> DeletionLineContext<'a> {
    let physical = *split_physical_lines(raw_text)
        .iter()
        .find(|line| line.start <= target.0 && target.1 <= line.end)
        .expect("deleted token must sit on some physical line");

    let is_declaration_only_line =
        draft.declarations.iter().any(|declaration| {
            (declaration.token.start, declaration.token.end) == target
        });
    if is_declaration_only_line {
        return DeletionLineContext {
            physical,
            content_start: physical.start,
            content_end: physical.end,
        };
    }

    for item in &draft.items {
        let Some((position, item_line)) =
            item.lines.iter().enumerate().find(|(_, line)| {
                line.raw.start <= target.0 && target.1 <= line.raw.end
            })
        else {
            continue;
        };
        if position == 0 {
            return DeletionLineContext {
                physical,
                content_start: physical.start,
                content_end: physical.end,
            };
        }
        return match classify_authored_line(item_line.raw) {
            AuthoredLineClass::Item(authored) => DeletionLineContext {
                physical,
                content_start: authored.body_start,
                content_end: item_line.raw.end,
            },
            _ => DeletionLineContext {
                physical,
                content_start: physical.start,
                content_end: physical.end,
            },
        };
    }

    DeletionLineContext {
        physical,
        content_start: physical.start,
        content_end: physical.end,
    }
}

/// Delete one token per the whitespace rule: the whole physical line
/// (terminator included) when it is the only token in its content region,
/// otherwise the token plus whichever adjacent whitespace run keeps no
/// double space behind -- the preceding run when the token ends its content
/// region, otherwise the following run.
fn deletion_edits_for_token(
    raw_text: &str,
    draft: &CaptureDraft<'_>,
    target: (usize, usize),
) -> Vec<TextEdit> {
    let context = deletion_line_context(raw_text, draft, target);
    let region_text = &raw_text[context.content_start..context.content_end];
    let tokens: Vec<Token<'_>> = tokenize_with_spans(region_text)
        .into_iter()
        .map(|token| Token {
            text: token.text,
            start: token.start + context.content_start,
            end: token.end + context.content_start,
        })
        .collect();
    let index = tokens
        .iter()
        .position(|token| (token.start, token.end) == target)
        .expect("deleted token must appear in its own content region");

    if tokens.len() == 1 {
        let (start, end) = whole_line_deletion_span(raw_text, context.physical);
        return vec![TextEdit {
            start,
            end,
            replacement: String::new(),
        }];
    }

    let (start, end) = if index == tokens.len() - 1 {
        (tokens[index - 1].end, target.1)
    } else {
        (target.0, tokens[index + 1].start)
    };
    vec![TextEdit {
        start,
        end,
        replacement: String::new(),
    }]
}

/// The span of `line` plus its own trailing line terminator (zero-length
/// when `line` is the draft's last physical line). Always claiming the
/// *trailing* terminator, never the preceding one, keeps adjacent whole-line
/// deletions from fighting over the same terminator bytes.
fn whole_line_deletion_span(
    raw_text: &str,
    line: RawLine<'_>,
) -> (usize, usize) {
    let bytes = raw_text.as_bytes();
    let terminator_len = match bytes.get(line.end) {
        Some(b'\r') => {
            if bytes.get(line.end + 1) == Some(&b'\n') {
                2
            } else {
                1
            }
        }
        Some(b'\n') => 1,
        _ => 0,
    };
    (line.start, line.end + terminator_len)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn editor(raw: &str) -> EditorParse {
        parse_for_editor(raw)
    }

    fn span_kinds(parse: &EditorParse) -> Vec<SpanKind> {
        parse.spans.iter().map(|span| span.kind).collect()
    }

    fn ranges(parse: &EditorParse) -> Vec<(usize, usize, SpanKind)> {
        parse
            .spans
            .iter()
            .map(|span| (span.start, span.end, span.kind))
            .collect()
    }

    fn codes(parse: &EditorParse) -> Vec<&'static str> {
        parse
            .diagnostics
            .iter()
            .map(|diagnostic| diagnostic.code)
            .collect()
    }

    /// `(input, mode, route, section, block id, needs)`
    type MarkerCase = (
        &'static str,
        EditorMode,
        Option<&'static str>,
        Option<&'static str>,
        Option<&'static str>,
        &'static [Need],
    );

    /// `(input, mode, route, block id, body)`
    type CrossedMarkerCase = (
        &'static str,
        EditorMode,
        Option<&'static str>,
        Option<&'static str>,
        &'static str,
    );

    #[test]
    fn tokenizer_records_half_open_byte_spans() {
        let tokens = tokenize_with_spans("  buy   milk  ");
        assert_eq!(
            tokens
                .iter()
                .map(|token| (token.text, token.start, token.end))
                .collect::<Vec<_>>(),
            vec![("buy", 2, 5), ("milk", 8, 12)]
        );
    }

    #[test]
    fn tokenizer_keeps_multibyte_and_crlf_offsets_on_char_boundaries() {
        // "café" is 5 bytes, the emoji is 4, and the combining acute in
        // "e\u{301}" adds 2 more; every span must still slice cleanly.
        let raw = "caf\u{e9} \u{1f680}\r\ne\u{301}tude\ts:1";
        let tokens = tokenize_with_spans(raw);
        let observed: Vec<(&str, usize, usize)> = tokens
            .iter()
            .map(|token| (token.text, token.start, token.end))
            .collect();
        assert_eq!(
            observed,
            vec![
                ("caf\u{e9}", 0, 5),
                ("\u{1f680}", 6, 10),
                ("e\u{301}tude", 12, 19),
                ("s:1", 20, 23),
            ]
        );
        for token in &tokens {
            assert!(raw.is_char_boundary(token.start));
            assert!(raw.is_char_boundary(token.end));
            assert_eq!(&raw[token.start..token.end], token.text);
        }
        // Spans stay ordered and never overlap.
        for pair in tokens.windows(2) {
            assert!(pair[0].end <= pair[1].start);
        }
    }

    #[test]
    fn editor_spans_use_original_byte_offsets_after_multibyte_text() {
        let raw = "caf\u{e9} run \u{1f680} @Cash+goog-exit";
        let parse = editor(raw);
        assert_eq!(parse.body, "caf\u{e9} run \u{1f680}");
        assert_eq!(parse.mode, EditorMode::SubBullet);
        assert_eq!(
            ranges(&parse),
            vec![
                (15, 20, SpanKind::SubBulletRoute),
                (21, 30, SpanKind::SubBulletBlockId),
            ]
        );
        for span in &parse.spans {
            assert!(raw.is_char_boundary(span.start));
            assert!(raw.is_char_boundary(span.end));
        }
    }

    #[test]
    fn plan_worked_example_matches_documented_offsets() {
        let raw = "Call bank @Cash+";
        let parse = editor(raw);
        assert_eq!(parse.body, "Call bank");
        assert_eq!(parse.mode, EditorMode::Incomplete);
        assert_eq!(parse.route.as_deref(), Some("cash"));
        assert_eq!(parse.needs, vec![Need::Task]);
        assert_eq!(
            ranges(&parse),
            vec![
                (10, 15, SpanKind::SubBulletRoute),
                (15, 16, SpanKind::InteractivePlaceholder),
            ]
        );
        assert_eq!(&raw[10..15], "@Cash");
        assert_eq!(&raw[15..16], "+");
    }

    #[test]
    fn editor_reports_terminal_marker_spans() {
        let raw = "body p:2 s:1 % @groceries";
        let parse = editor(raw);
        assert_eq!(parse.body, "body");
        assert_eq!(parse.mode, EditorMode::Task);
        assert_eq!(parse.route.as_deref(), Some("groceries"));
        assert_eq!(
            ranges(&parse),
            vec![
                (5, 8, SpanKind::Priority),
                (9, 12, SpanKind::Schedule),
                (13, 14, SpanKind::Clipboard),
                (15, 25, SpanKind::Route),
            ]
        );
    }

    #[test]
    fn editor_modes_and_needs_cover_every_marker_shape() {
        // (input, mode, route, section, block_id, needs)
        let cases: &[MarkerCase] = &[
            (
                "Body @dev+focus-123",
                EditorMode::SubBullet,
                Some("dev"),
                None,
                Some("focus-123"),
                &[],
            ),
            (
                "Body @dev+",
                EditorMode::Incomplete,
                Some("dev"),
                None,
                None,
                &[Need::Task],
            ),
            (
                "Body @+focus-123",
                EditorMode::Incomplete,
                None,
                None,
                Some("focus-123"),
                &[Need::Route],
            ),
            (
                "Body @+",
                EditorMode::Incomplete,
                None,
                None,
                None,
                &[Need::Route, Need::Task],
            ),
            (
                "Body @dev+focus-123#req",
                EditorMode::SubBullet,
                Some("dev"),
                Some("req"),
                Some("focus-123"),
                &[],
            ),
            (
                "Body @dev+focus-123#",
                EditorMode::Incomplete,
                Some("dev"),
                None,
                Some("focus-123"),
                &[Need::TaskSection],
            ),
            (
                "Body @dev+#req",
                EditorMode::Incomplete,
                Some("dev"),
                Some("req"),
                None,
                &[Need::Task],
            ),
            (
                "Body @dev+#",
                EditorMode::Incomplete,
                Some("dev"),
                None,
                None,
                &[Need::Task, Need::TaskSection],
            ),
            (
                "Body @+focus-123#req",
                EditorMode::Incomplete,
                None,
                Some("req"),
                Some("focus-123"),
                &[Need::Route],
            ),
            (
                "Body @+#req",
                EditorMode::Incomplete,
                None,
                Some("req"),
                None,
                &[Need::Route, Need::Task],
            ),
            (
                "Body @+#",
                EditorMode::Incomplete,
                None,
                None,
                None,
                &[Need::Route, Need::Task, Need::TaskSection],
            ),
            (
                "Body @dev^focus-123",
                EditorMode::Task,
                Some("dev"),
                None,
                Some("focus-123"),
                &[],
            ),
            (
                "Body @dev^",
                EditorMode::Incomplete,
                Some("dev"),
                None,
                None,
                &[Need::BlockId],
            ),
            (
                "Body @^focus-123",
                EditorMode::Incomplete,
                None,
                None,
                Some("focus-123"),
                &[Need::Route],
            ),
            (
                "Body @^",
                EditorMode::Incomplete,
                None,
                None,
                None,
                &[Need::Route, Need::BlockId],
            ),
            (
                "Body @dev:focus-123",
                EditorMode::PomodoroTask,
                Some("dev"),
                None,
                Some("focus-123"),
                &[],
            ),
            (
                "Body @!dev:focus-123",
                EditorMode::PomodoroTask,
                Some("dev"),
                None,
                Some("focus-123"),
                &[],
            ),
            (
                "Body @dev:",
                EditorMode::Incomplete,
                Some("dev"),
                None,
                None,
                &[Need::PomodoroId],
            ),
            (
                "Body @!dev",
                EditorMode::Incomplete,
                Some("dev"),
                None,
                None,
                &[Need::PomodoroId],
            ),
            (
                "Body @:focus-123",
                EditorMode::Incomplete,
                None,
                None,
                Some("focus-123"),
                &[Need::Route],
            ),
            (
                "Body @:",
                EditorMode::Incomplete,
                None,
                None,
                None,
                &[Need::Route, Need::PomodoroId],
            ),
            (
                "Body @!",
                EditorMode::Incomplete,
                None,
                None,
                None,
                &[Need::Route, Need::PomodoroId],
            ),
            (
                "Body @",
                EditorMode::Incomplete,
                None,
                None,
                None,
                &[Need::Route],
            ),
            (
                "Body @#",
                EditorMode::Incomplete,
                None,
                None,
                None,
                &[Need::Route],
            ),
            (
                "Body @#Ideas",
                EditorMode::Incomplete,
                None,
                Some("Ideas"),
                None,
                &[Need::Route],
            ),
            (
                "Body @notes#",
                EditorMode::Bullet,
                Some("notes"),
                None,
                None,
                &[Need::Section],
            ),
            (
                "Body @notes#Ideas",
                EditorMode::Bullet,
                Some("notes"),
                Some("Ideas"),
                None,
                &[],
            ),
            (
                "Body @work",
                EditorMode::Task,
                Some("work"),
                None,
                None,
                &[],
            ),
            ("buy milk", EditorMode::Task, None, None, None, &[]),
            ("@route", EditorMode::Task, None, None, None, &[]),
        ];

        for (raw, mode, route, section, block_id, needs) in cases {
            let parse = editor(raw);
            assert_eq!(parse.mode, *mode, "{raw}");
            assert_eq!(parse.route.as_deref(), *route, "{raw}");
            assert_eq!(parse.section.as_deref(), *section, "{raw}");
            assert_eq!(parse.block_id.as_deref(), *block_id, "{raw}");
            assert_eq!(parse.needs, needs.to_vec(), "{raw}");
            assert!(parse.diagnostics.is_empty(), "{raw}");
        }
    }

    #[test]
    fn editor_spans_cover_every_marker_shape() {
        let cases: &[(&str, &[SpanKind])] = &[
            (
                "Body @dev+focus-123",
                &[SpanKind::SubBulletRoute, SpanKind::SubBulletBlockId],
            ),
            (
                "Body @dev+",
                &[SpanKind::SubBulletRoute, SpanKind::InteractivePlaceholder],
            ),
            (
                "Body @+focus-123",
                &[SpanKind::InteractivePlaceholder, SpanKind::SubBulletBlockId],
            ),
            ("Body @+", &[SpanKind::InteractivePlaceholder]),
            (
                "Body @dev+focus-123#req",
                &[
                    SpanKind::SubBulletRoute,
                    SpanKind::SubBulletBlockId,
                    SpanKind::SubBulletSection,
                ],
            ),
            (
                "Body @dev+focus-123#",
                &[
                    SpanKind::SubBulletRoute,
                    SpanKind::SubBulletBlockId,
                    SpanKind::InteractivePlaceholder,
                ],
            ),
            (
                "Body @dev+#req",
                &[
                    SpanKind::SubBulletRoute,
                    SpanKind::InteractivePlaceholder,
                    SpanKind::SubBulletSection,
                ],
            ),
            (
                "Body @dev+#",
                &[
                    SpanKind::SubBulletRoute,
                    SpanKind::InteractivePlaceholder,
                    SpanKind::InteractivePlaceholder,
                ],
            ),
            (
                "Body @+focus-123#req",
                &[
                    SpanKind::InteractivePlaceholder,
                    SpanKind::SubBulletBlockId,
                    SpanKind::SubBulletSection,
                ],
            ),
            (
                "Body @+#req",
                &[SpanKind::InteractivePlaceholder, SpanKind::SubBulletSection],
            ),
            (
                "Body @+#",
                &[
                    SpanKind::InteractivePlaceholder,
                    SpanKind::InteractivePlaceholder,
                ],
            ),
            (
                "Body @dev^focus-123",
                &[SpanKind::TaskBlockIdRoute, SpanKind::TaskBlockId],
            ),
            (
                "Body @dev^",
                &[SpanKind::TaskBlockIdRoute, SpanKind::InteractivePlaceholder],
            ),
            (
                "Body @^focus-123",
                &[SpanKind::InteractivePlaceholder, SpanKind::TaskBlockId],
            ),
            ("Body @^", &[SpanKind::InteractivePlaceholder]),
            (
                "Body @dev:focus-123",
                &[SpanKind::PomodoroRoute, SpanKind::PomodoroBlockId],
            ),
            (
                "Body @!dev:focus-123",
                &[SpanKind::PomodoroRoute, SpanKind::PomodoroBlockId],
            ),
            (
                "Body @dev:",
                &[SpanKind::PomodoroRoute, SpanKind::InteractivePlaceholder],
            ),
            ("Body @!dev", &[SpanKind::PomodoroRoute]),
            (
                "Body @:focus-123",
                &[SpanKind::InteractivePlaceholder, SpanKind::PomodoroBlockId],
            ),
            ("Body @:", &[SpanKind::InteractivePlaceholder]),
            ("Body @!", &[SpanKind::InteractivePlaceholder]),
            ("Body @", &[SpanKind::InteractivePlaceholder]),
            ("Body @#", &[SpanKind::InteractivePlaceholder]),
            (
                "Body @#Ideas",
                &[SpanKind::InteractivePlaceholder, SpanKind::Section],
            ),
            (
                "Body @notes#",
                &[SpanKind::Route, SpanKind::InteractivePlaceholder],
            ),
            ("Body @notes#Ideas", &[SpanKind::Route, SpanKind::Section]),
            ("Body @work", &[SpanKind::Route]),
            ("buy milk", &[]),
        ];

        for (raw, kinds) in cases {
            let parse = editor(raw);
            assert_eq!(span_kinds(&parse), kinds.to_vec(), "{raw}");
            for span in &parse.spans {
                assert!(raw.is_char_boundary(span.start), "{raw}");
                assert!(raw.is_char_boundary(span.end), "{raw}");
                assert!(span.start < span.end, "{raw}");
            }
            for pair in parse.spans.windows(2) {
                assert!(pair[0].end <= pair[1].start, "{raw}");
            }
        }
    }

    #[test]
    fn editor_reports_invalid_components_as_diagnostics() {
        let cases: &[(&str, &str, &str)] = &[
            (
                "Body @bad.route+id",
                "invalid_sub_bullet_route",
                SUB_BULLET_ROUTE_ERROR,
            ),
            (
                "Body @dev+bad.id",
                "invalid_sub_bullet_block_id",
                SUB_BULLET_BLOCK_ID_ERROR,
            ),
            (
                "Body @dev+id#bad_id",
                "invalid_sub_bullet_section",
                SUB_BULLET_SECTION_ERROR,
            ),
            (
                "Body @dev+id#req^x",
                "invalid_sub_bullet_section",
                SUB_BULLET_SECTION_ERROR,
            ),
            (
                "Body @dev+id#req:x",
                "invalid_sub_bullet_section",
                SUB_BULLET_SECTION_ERROR,
            ),
            (
                "Body @dev+id#req+x",
                "invalid_sub_bullet_section",
                SUB_BULLET_SECTION_ERROR,
            ),
            (
                "Body @dev+id#req#x",
                "invalid_sub_bullet_section",
                SUB_BULLET_SECTION_ERROR,
            ),
            (
                "Body @bad.route^id",
                "invalid_task_block_id_route",
                TASK_BLOCK_ID_ROUTE_ERROR,
            ),
            (
                "Body @dev^bad.id",
                "invalid_task_block_id",
                TASK_BLOCK_ID_ERROR,
            ),
            (
                "Body @bad.route:id",
                "invalid_pomodoro_route",
                POMODORO_ROUTE_ERROR,
            ),
            (
                "Body @dev:bad.id",
                "invalid_pomodoro_block_id",
                POMODORO_BLOCK_ID_ERROR,
            ),
        ];

        for (raw, code, message) in cases {
            let parse = editor(raw);
            assert_eq!(parse.mode, EditorMode::Task, "{raw}");
            assert_eq!(parse.body, "Body", "{raw}");
            assert!(parse.needs.is_empty(), "{raw}");
            assert_eq!(parse.diagnostics.len(), 1, "{raw}");
            let diagnostic = &parse.diagnostics[0];
            assert_eq!(diagnostic.severity, Severity::Error, "{raw}");
            assert_eq!(diagnostic.code, *code, "{raw}");
            assert_eq!(diagnostic.message, *message, "{raw}");
            assert_eq!(diagnostic.range, Some((5, raw.len())), "{raw}");
        }
    }

    #[test]
    fn editor_reports_retired_double_colon_as_migration_guidance() {
        for raw in [
            "Body @dev::focus-123",
            "Body @dev::",
            "Body @::focus-123",
            "Body @::",
            "Body @dev::bad.id",
        ] {
            let parse = editor(raw);
            assert_eq!(parse.mode, EditorMode::Task, "{raw}");
            assert_eq!(parse.body, "Body", "{raw}");
            assert_eq!(parse.route, None, "{raw}");
            assert!(parse.needs.is_empty(), "{raw}");
            assert_eq!(
                codes(&parse),
                vec!["retired_task_block_id_marker"],
                "{raw}"
            );
            assert_eq!(
                parse.diagnostics[0].message, RETIRED_DOUBLE_COLON_ERROR,
                "{raw}"
            );
            assert_eq!(
                parse.diagnostics[0].range,
                Some((5, raw.len())),
                "{raw}"
            );
        }
    }

    #[test]
    fn mixed_separators_keep_the_first_family_and_do_not_steal_section_suffixes(
    ) {
        let bullet_plus = editor("Jot @notes#time+box");
        assert_eq!(bullet_plus.mode, EditorMode::Bullet);
        assert_eq!(bullet_plus.section.as_deref(), Some("time+box"));
        assert!(bullet_plus.diagnostics.is_empty());

        let bullet_caret = editor("Jot @notes#time^box");
        assert_eq!(bullet_caret.mode, EditorMode::Bullet);
        assert_eq!(bullet_caret.section.as_deref(), Some("time^box"));
        assert!(bullet_caret.diagnostics.is_empty());

        let bullet_colons = editor("Jot @notes#time::box");
        assert_eq!(bullet_colons.mode, EditorMode::Bullet);
        assert_eq!(bullet_colons.section.as_deref(), Some("time::box"));
        assert!(bullet_colons.diagnostics.is_empty());

        let plus_then_hash = editor("Add context @route+bad#section");
        assert_eq!(plus_then_hash.mode, EditorMode::SubBullet);
        assert_eq!(plus_then_hash.block_id.as_deref(), Some("bad"));
        assert_eq!(plus_then_hash.section.as_deref(), Some("section"));
        assert!(plus_then_hash.diagnostics.is_empty());

        let plus_then_colon = editor("Add context @route+bad:id");
        assert_eq!(
            codes(&plus_then_colon),
            vec!["invalid_sub_bullet_block_id"]
        );

        let caret_then_colon = editor("Do work @route^bad:id");
        assert_eq!(codes(&caret_then_colon), vec!["invalid_task_block_id"]);

        let plus_then_caret = editor("Add context @route+id^x");
        assert_eq!(
            codes(&plus_then_caret),
            vec!["invalid_sub_bullet_block_id"]
        );

        let caret_then_plus = editor("Do work @route^id+x");
        assert_eq!(codes(&caret_then_plus), vec!["invalid_task_block_id"]);

        let colon_then_plus = editor("Do work @route:id+x");
        assert_eq!(codes(&colon_then_plus), vec!["invalid_pomodoro_block_id"]);

        let colon_then_caret = editor("Do work @route:id^x");
        assert_eq!(codes(&colon_then_caret), vec!["invalid_pomodoro_block_id"]);
    }

    #[test]
    fn editor_reports_legacy_bullet_markers_without_failing() {
        let parse = editor("Some note #bar");
        assert_eq!(codes(&parse), vec!["legacy_bullet_marker"]);
        assert_eq!(parse.diagnostics[0].range, Some((10, 14)));
        assert_eq!(parse.mode, EditorMode::Task);
        assert_eq!(parse.body, "Some note #bar");

        let parse = editor("Some note #bar @foo");
        assert_eq!(codes(&parse), vec!["legacy_bullet_marker"]);
        assert_eq!(parse.diagnostics[0].range, Some((10, 14)));
        // The trailing route still resolves so the editor can keep painting.
        assert_eq!(parse.route.as_deref(), Some("foo"));
    }

    #[test]
    fn editor_leading_marker_wins_over_trailing_marker() {
        let parse = editor("@work buy milk @home");
        assert_eq!(parse.route.as_deref(), Some("work"));
        assert_eq!(parse.body, "buy milk @home");
        assert_eq!(ranges(&parse), vec![(0, 5, SpanKind::Route)]);

        // An invalid prefix is not route-shaped, so the suffix still wins --
        // exactly like `parse_capture_text_with_clip_control`.
        let parse = editor("@bad! body @Good");
        assert_eq!(parse.route.as_deref(), Some("good"));
        assert_eq!(parse.body, "@bad! body");
    }

    #[test]
    fn editor_keeps_middle_and_time_tokens_literal() {
        for raw in [
            "Email @home soon",
            "call dentist @5:30pm",
            "standup @10:00",
            "Discuss @dev:id later",
            "Discuss @dev+id later",
            "Discuss @dev^id later",
            "Discuss @dev::id later",
        ] {
            let parse = editor(raw);
            assert_eq!(parse.mode, EditorMode::Task, "{raw}");
            assert_eq!(parse.body, raw, "{raw}");
            assert_eq!(parse.route, None, "{raw}");
            assert!(parse.spans.is_empty(), "{raw}");
            assert!(parse.diagnostics.is_empty(), "{raw}");
        }
    }

    #[test]
    fn editor_accepts_marker_only_input_with_an_empty_body() {
        for (raw, mode) in [
            ("@dev:id", EditorMode::PomodoroTask),
            ("@:", EditorMode::Incomplete),
            ("@dev+id", EditorMode::SubBullet),
            ("@dev+id#req", EditorMode::SubBullet),
            ("@+", EditorMode::Incomplete),
            ("@dev^id", EditorMode::Task),
            ("@^", EditorMode::Incomplete),
            ("@", EditorMode::Incomplete),
        ] {
            let parse = editor(raw);
            assert_eq!(parse.body, "", "{raw}");
            assert_eq!(parse.mode, mode, "{raw}");
        }
    }

    /// Every input `bob capture` resolves to a concrete capture must parse
    /// identically here. The interactive-only forms (`@`, `@#`, `@:`, `@^`,
    /// `@+`, and friends) are the documented exception: execution keeps them
    /// literal or rejects incomplete markers, and this module reports them
    /// as `incomplete` instead.
    #[test]
    fn editor_agrees_with_execution_for_resolved_captures() {
        let inputs = [
            "buy milk",
            "buy milk @groceries",
            "@Groceries Buy Milk",
            "a @b @C",
            "@Work buy milk @home",
            "Email @home soon",
            "@route",
            "@bad! body @Good",
            "Do thing @Dev^Foo-Bar",
            "@Dev^Foo-Bar Do thing s:2",
            "Do thing @Dev^Foo-Bar p:2 s:1",
            "Do thing @Dev:Foo-Bar",
            "@Dev:Foo-Bar Do thing s:2",
            "Do thing @!Dev:Foo-Bar s:2",
            "Called today @Cash+Goog-Exit",
            "Called today %log @Cash+Goog-Exit",
            "Called today @Cash+Goog-Exit s:1",
            "Postgres 17 minimum @foo+bar#requirements",
            "@foo+bar#requirements Postgres 17 minimum",
            "Postgres 17 minimum @foo+bar#requirements s:1",
            "Postgres 17 minimum %log @foo+bar#requirements",
            "Postgres 17 minimum @foo+bar#q-and-a",
            "Postgres 17 minimum @foo+bar#Q&A",
            "Some note @foo#bar",
            "@foo#bar Some note",
            "Some note @foo#",
            "@foo# Some note",
            "body p:2 s:1 % @groceries",
            "body @groceries %log p:3 s:4",
            "Jot @notes#time:box",
            "take s:1 pill",
            "save % now",
            "body %bad!",
            "remembered to bump the timeout #",
            "paste the failing output % #",
            "paste the failing output # %",
        ];

        for raw in inputs {
            let executed =
                parse_capture_text_with_clip_control(raw, None, None, true)
                    .unwrap_or_else(|error| panic!("{raw}: {error}"));
            let parse = editor(raw);
            assert_eq!(parse.body, executed.body, "{raw}");
            assert_eq!(parse.route, executed.route, "{raw}");
            let expected_mode = match &executed.kind {
                CaptureKind::Task | CaptureKind::TaskWithBlockId { .. } => {
                    EditorMode::Task
                }
                CaptureKind::Bullet { .. } => EditorMode::Bullet,
                CaptureKind::Pomodoro { .. } => EditorMode::PomodoroTask,
                CaptureKind::SubBullet { .. } => EditorMode::SubBullet,
                CaptureKind::PomodoroNote => EditorMode::PomodoroNote,
            };
            assert_eq!(parse.mode, expected_mode, "{raw}");
            if let CaptureKind::TaskWithBlockId { block_id } = &executed.kind {
                assert_eq!(parse.block_id.as_deref(), Some(block_id.as_str()));
            }
            if let CaptureKind::Pomodoro { block_id } = &executed.kind {
                assert_eq!(parse.block_id.as_deref(), Some(block_id.as_str()));
            }
            if let CaptureKind::SubBullet {
                target: SubBulletTarget::BlockId(block_id),
                section,
            } = &executed.kind
            {
                assert_eq!(parse.block_id.as_deref(), Some(block_id.as_str()));
                assert_eq!(
                    parse.section.as_deref(),
                    section.as_ref().map(|selector| selector.text.as_str()),
                    "{raw}"
                );
            }
            if let CaptureKind::Bullet { section_prefix, .. } = &executed.kind {
                assert_eq!(parse.section, *section_prefix, "{raw}");
            }
            assert!(parse.diagnostics.is_empty(), "{raw}");
        }
    }

    #[test]
    fn interactive_markers_are_the_only_divergence_from_execution() {
        // `bob capture` keeps these literal because no route resolves; the
        // interactive grammar reports what the picker still owes instead.
        for raw in [
            "Body @",
            "Body @#",
            "Body @#Ideas",
            "Body @:",
            "Body @:focus-123",
        ] {
            let executed =
                parse_capture_text_with_clip_control(raw, None, None, true)
                    .unwrap_or_else(|error| panic!("{raw}: {error}"));
            assert_eq!(executed.kind, CaptureKind::Task, "{raw}");
            assert_eq!(executed.route, None, "{raw}");
            assert_eq!(executed.body, raw, "{raw}");

            let parse = editor(raw);
            assert_eq!(parse.mode, EditorMode::Incomplete, "{raw}");
            assert_eq!(parse.body, "Body", "{raw}");
            assert_eq!(parse.needs.first(), Some(&Need::Route), "{raw}");
            assert!(parse.diagnostics.is_empty(), "{raw}");
        }

        // These reach `bob capture`'s strict marker validation and fail it,
        // yet they are ordinary mid-typing states for an editor.
        for raw in [
            "Body @^",
            "Body @dev^",
            "Body @^focus-123",
            "Body @+",
            "Body @dev+",
            "Body @+focus-123",
            "Body @dev+id#",
            "Body @dev+#req",
            "Body @+#req",
            "Body @+#",
            "Body @dev:",
            "Body @!",
            "Body @!dev",
        ] {
            parse_capture_text_with_clip_control(raw, None, None, true)
                .expect_err(raw);

            let parse = editor(raw);
            assert_eq!(parse.mode, EditorMode::Incomplete, "{raw}");
            assert_eq!(parse.body, "Body", "{raw}");
            assert!(!parse.needs.is_empty(), "{raw}");
            assert!(parse.diagnostics.is_empty(), "{raw}");
        }
    }

    // -----------------------------------------------------------------
    // Ported from chezmoi's `tests/hammerspoon/task_capture_spec.lua`.
    //
    // The picker state-machine cases (`capture.new_state`, `stage`,
    // `reset`, `set_block_id`, `finalize`, `finalize_sub_bullet`) are not
    // ported: they model Hammerspoon UI state, not the capture grammar,
    // and this module exposes the same information as `needs`.
    // -----------------------------------------------------------------

    #[test]
    fn lua_parses_all_four_canonical_pomodoro_forms() {
        let complete = editor("Do work @Dev:focus-123");
        assert_eq!(complete.mode, EditorMode::PomodoroTask);
        assert_eq!(complete.body, "Do work");
        assert_eq!(complete.route.as_deref(), Some("dev"));
        assert_eq!(complete.block_id.as_deref(), Some("focus-123"));
        assert!(complete.needs.is_empty());

        let needs_id = editor("Do work @Dev:");
        assert_eq!(needs_id.route.as_deref(), Some("dev"));
        assert_eq!(needs_id.needs, vec![Need::PomodoroId]);

        let needs_target = editor("Do work @:focus-123");
        assert_eq!(needs_target.block_id.as_deref(), Some("focus-123"));
        assert_eq!(needs_target.needs, vec![Need::Route]);

        let needs_both = editor("Do work @:");
        assert_eq!(needs_both.needs, vec![Need::Route, Need::PomodoroId]);
    }

    #[test]
    fn lua_accepts_legacy_boundary_aliases() {
        // Delta from Lua: the Lua fixture used `@!Dev:old_id`, but Bob's
        // block IDs have never accepted `_`, so that exact input is a
        // diagnostic here and the hyphenated form is the passing case.
        let complete = editor("Do work @!Dev:old-id");
        assert_eq!(complete.mode, EditorMode::PomodoroTask);
        assert_eq!(complete.route.as_deref(), Some("dev"));
        assert_eq!(complete.block_id.as_deref(), Some("old-id"));

        let underscored = editor("Do work @!Dev:old_id");
        assert_eq!(codes(&underscored), vec!["invalid_pomodoro_block_id"]);

        let route_only = editor("Do work @!Dev");
        assert_eq!(route_only.route.as_deref(), Some("dev"));
        assert_eq!(route_only.needs, vec![Need::PomodoroId]);

        let neither = editor("Do work @!");
        assert_eq!(neither.needs, vec![Need::Route, Need::PomodoroId]);
    }

    #[test]
    fn lua_parses_all_four_canonical_sub_bullet_forms() {
        let complete = editor("Add context @Dev+focus-123");
        assert_eq!(complete.mode, EditorMode::SubBullet);
        assert_eq!(complete.body, "Add context");
        assert_eq!(complete.route.as_deref(), Some("dev"));
        assert_eq!(complete.block_id.as_deref(), Some("focus-123"));
        assert!(complete.needs.is_empty());

        let needs_task = editor("Add context @Dev+");
        assert_eq!(needs_task.route.as_deref(), Some("dev"));
        assert_eq!(needs_task.needs, vec![Need::Task]);

        let needs_target = editor("Add context @+focus-123");
        assert_eq!(needs_target.block_id.as_deref(), Some("focus-123"));
        assert_eq!(needs_target.needs, vec![Need::Route]);

        let needs_both = editor("Add context @+");
        assert_eq!(needs_both.needs, vec![Need::Route, Need::Task]);

        let with_section = editor("Add context @Dev+focus-123#req");
        assert_eq!(with_section.mode, EditorMode::SubBullet);
        assert_eq!(with_section.block_id.as_deref(), Some("focus-123"));
        assert_eq!(with_section.section.as_deref(), Some("req"));
        assert!(with_section.needs.is_empty());

        let needs_section = editor("Add context @Dev+focus-123#");
        assert_eq!(needs_section.needs, vec![Need::TaskSection]);
        assert_eq!(needs_section.block_id.as_deref(), Some("focus-123"));
    }

    #[test]
    fn lua_parses_all_four_canonical_task_block_id_forms() {
        let complete = editor("Do work @Dev^focus-123");
        assert_eq!(complete.mode, EditorMode::Task);
        assert_eq!(complete.body, "Do work");
        assert_eq!(complete.route.as_deref(), Some("dev"));
        assert_eq!(complete.block_id.as_deref(), Some("focus-123"));
        assert!(complete.needs.is_empty());

        let needs_id = editor("Do work @Dev^");
        assert_eq!(needs_id.route.as_deref(), Some("dev"));
        assert_eq!(needs_id.needs, vec![Need::BlockId]);

        let needs_target = editor("Do work @^focus-123");
        assert_eq!(needs_target.block_id.as_deref(), Some("focus-123"));
        assert_eq!(needs_target.needs, vec![Need::Route]);

        let needs_both = editor("Do work @^");
        assert_eq!(needs_both.needs, vec![Need::Route, Need::BlockId]);
    }

    #[test]
    fn lua_gives_sub_bullet_markers_precedence_over_pomodoro_markers() {
        let malformed = editor("Add context @route+bad:id");
        assert_eq!(codes(&malformed), vec!["invalid_sub_bullet_block_id"]);
        assert!(
            malformed.diagnostics[0].message.contains("sub-bullet"),
            "{:?}",
            malformed.diagnostics
        );

        let pomodoro = editor("Do work @route:id");
        assert_eq!(pomodoro.mode, EditorMode::PomodoroTask);
        assert_eq!(pomodoro.block_id.as_deref(), Some("id"));
    }

    #[test]
    fn lua_rejects_invalid_sub_bullet_and_pomodoro_components() {
        let cases = [
            ("Add context @bad.route+id", "invalid_sub_bullet_route"),
            ("Add context @route+bad.id", "invalid_sub_bullet_block_id"),
            ("Add context @route+bad_id", "invalid_sub_bullet_block_id"),
            ("Add context @route+id#bad_id", "invalid_sub_bullet_section"),
            ("Do work @bad.route:id", "invalid_pomodoro_route"),
            ("Do work @route:bad.id", "invalid_pomodoro_block_id"),
            ("Do work @route:id:extra", "invalid_pomodoro_block_id"),
            ("Do work @!:id", "invalid_pomodoro_route"),
        ];

        for (raw, code) in cases {
            let parse = editor(raw);
            assert_eq!(codes(&parse), vec![code], "{raw}");
            assert_eq!(parse.diagnostics[0].severity, Severity::Error, "{raw}");
        }
    }

    #[test]
    fn lua_keeps_middle_markers_literal_and_marker_only_bodies_empty() {
        let parse = editor("Discuss @dev:id later");
        assert_eq!(parse.mode, EditorMode::Task);
        assert_eq!(parse.body, "Discuss @dev:id later");

        let parse = editor("Discuss @dev+id later");
        assert_eq!(parse.mode, EditorMode::Task);
        assert_eq!(parse.body, "Discuss @dev+id later");

        let parse = editor("Discuss @dev^id later");
        assert_eq!(parse.mode, EditorMode::Task);
        assert_eq!(parse.body, "Discuss @dev^id later");

        for raw in ["@dev:id", "@:", "@dev+id", "@+", "@dev^id", "@^"] {
            assert_eq!(editor(raw).body, "", "{raw}");
        }
    }

    #[test]
    fn lua_composes_clipboard_terminal_markers_around_every_picker_token() {
        // (token, mode, route, section, block_id, needs)
        let cases: &[MarkerCase] = &[
            (
                "@",
                EditorMode::Incomplete,
                None,
                None,
                None,
                &[Need::Route],
            ),
            (
                "@#",
                EditorMode::Incomplete,
                None,
                None,
                None,
                &[Need::Route],
            ),
            (
                "@#Ideas",
                EditorMode::Incomplete,
                None,
                Some("Ideas"),
                None,
                &[Need::Route],
            ),
            (
                "@Notes#",
                EditorMode::Bullet,
                Some("notes"),
                None,
                None,
                &[Need::Section],
            ),
            (
                "@Dev:focus-123",
                EditorMode::PomodoroTask,
                Some("dev"),
                None,
                Some("focus-123"),
                &[],
            ),
            (
                "@Dev:",
                EditorMode::Incomplete,
                Some("dev"),
                None,
                None,
                &[Need::PomodoroId],
            ),
            (
                "@:focus-123",
                EditorMode::Incomplete,
                None,
                None,
                Some("focus-123"),
                &[Need::Route],
            ),
            (
                "@:",
                EditorMode::Incomplete,
                None,
                None,
                None,
                &[Need::Route, Need::PomodoroId],
            ),
            (
                "@!Dev:focus-123",
                EditorMode::PomodoroTask,
                Some("dev"),
                None,
                Some("focus-123"),
                &[],
            ),
            (
                "@!Dev",
                EditorMode::Incomplete,
                Some("dev"),
                None,
                None,
                &[Need::PomodoroId],
            ),
            (
                "@!",
                EditorMode::Incomplete,
                None,
                None,
                None,
                &[Need::Route, Need::PomodoroId],
            ),
            (
                "@Dev+focus-123",
                EditorMode::SubBullet,
                Some("dev"),
                None,
                Some("focus-123"),
                &[],
            ),
            (
                "@Dev+",
                EditorMode::Incomplete,
                Some("dev"),
                None,
                None,
                &[Need::Task],
            ),
            (
                "@+focus-123",
                EditorMode::Incomplete,
                None,
                None,
                Some("focus-123"),
                &[Need::Route],
            ),
            (
                "@+",
                EditorMode::Incomplete,
                None,
                None,
                None,
                &[Need::Route, Need::Task],
            ),
            (
                "@Dev+focus-123#req",
                EditorMode::SubBullet,
                Some("dev"),
                Some("req"),
                Some("focus-123"),
                &[],
            ),
            (
                "@Dev^focus-123",
                EditorMode::Task,
                Some("dev"),
                None,
                Some("focus-123"),
                &[],
            ),
            (
                "@Dev^",
                EditorMode::Incomplete,
                Some("dev"),
                None,
                None,
                &[Need::BlockId],
            ),
            (
                "@^focus-123",
                EditorMode::Incomplete,
                None,
                None,
                Some("focus-123"),
                &[Need::Route],
            ),
            (
                "@^",
                EditorMode::Incomplete,
                None,
                None,
                None,
                &[Need::Route, Need::BlockId],
            ),
        ];

        for (token, mode, route, section, block_id, needs) in cases {
            for marker in ["%", "%03", "%build_log"] {
                for raw in [
                    format!("Body {marker} {token}"),
                    format!("Body {token} {marker}"),
                ] {
                    let parse = editor(&raw);
                    assert_eq!(parse.mode, *mode, "{raw}");
                    assert_eq!(parse.route.as_deref(), *route, "{raw}");
                    assert_eq!(parse.section.as_deref(), *section, "{raw}");
                    assert_eq!(parse.block_id.as_deref(), *block_id, "{raw}");
                    assert_eq!(parse.needs, needs.to_vec(), "{raw}");
                    assert!(parse.diagnostics.is_empty(), "{raw}");
                }
            }
        }
    }

    #[test]
    fn lua_clipboard_composition_body_follows_bob_terminal_extraction() {
        // Delta from Lua: Hammerspoon left `%`/`s:`/`p:` in the body for
        // `bob capture` to interpret, while this module extracts them into
        // structured spans. A clipboard marker that precedes an incomplete
        // marker still stays in the body, because `is_route_marker` (shared
        // with execution) only recognizes complete route tokens.
        assert_eq!(editor("Body % @Dev:focus-123").body, "Body");
        assert_eq!(editor("Body @Dev:focus-123 %").body, "Body");
        assert_eq!(editor("Body @Dev+ %build_log").body, "Body");
        assert_eq!(editor("Body % @Dev+").body, "Body %");
        assert_eq!(editor("Body @Dev^ %build_log").body, "Body");
        assert_eq!(editor("Body % @Dev^").body, "Body %");
    }

    #[test]
    fn lua_preserves_crossed_clipboard_and_schedule_markers() {
        let cases: &[CrossedMarkerCase] = &[
            (
                "Body @Notes# % s:2",
                EditorMode::Bullet,
                Some("notes"),
                None,
                "Body",
            ),
            (
                "Body @Notes# s:2 %03",
                EditorMode::Bullet,
                Some("notes"),
                None,
                "Body",
            ),
            (
                "Body % @Notes# s:2",
                EditorMode::Bullet,
                Some("notes"),
                None,
                "Body",
            ),
            (
                "Body s:2 @Notes# %build_log",
                EditorMode::Bullet,
                Some("notes"),
                None,
                "Body",
            ),
            (
                "Body @:focus-123 % s:0",
                EditorMode::Incomplete,
                None,
                Some("focus-123"),
                "Body",
            ),
            (
                "Body @Dev+ s:10 %build_log",
                EditorMode::Incomplete,
                Some("dev"),
                None,
                "Body",
            ),
            (
                "Body @Dev^ s:10 %build_log",
                EditorMode::Incomplete,
                Some("dev"),
                None,
                "Body",
            ),
            (
                "Body @Notes# p:2 s:1",
                EditorMode::Bullet,
                Some("notes"),
                None,
                "Body",
            ),
            (
                "Body p:2 @Notes# s:1",
                EditorMode::Bullet,
                Some("notes"),
                None,
                "Body",
            ),
            (
                "Body @:focus-123 p:3 s:0",
                EditorMode::Incomplete,
                None,
                Some("focus-123"),
                "Body",
            ),
            (
                "Body @Dev+ p:4 s:10",
                EditorMode::Incomplete,
                Some("dev"),
                None,
                "Body",
            ),
            (
                "Body @Dev^ p:4 s:10",
                EditorMode::Incomplete,
                Some("dev"),
                None,
                "Body",
            ),
        ];

        for (raw, mode, route, block_id, body) in cases {
            let parse = editor(raw);
            assert_eq!(parse.mode, *mode, "{raw}");
            assert_eq!(parse.route.as_deref(), *route, "{raw}");
            assert_eq!(parse.block_id.as_deref(), *block_id, "{raw}");
            assert_eq!(parse.body, *body, "{raw}");
        }

        // Delta from Lua: a leading `p:N` that sits before an incomplete
        // marker is not part of the terminal region for either parser, so it
        // stays in the body here just as `bob capture` would keep it.
        let parse = editor("Body p:3 @:focus-123 s:0");
        assert_eq!(parse.body, "Body p:3");
        assert_eq!(parse.block_id.as_deref(), Some("focus-123"));
    }

    #[test]
    fn lua_preserves_existing_note_and_section_descriptors() {
        let parse = editor("Task @");
        assert_eq!(parse.mode, EditorMode::Incomplete);
        assert_eq!(parse.body, "Task");
        assert_eq!(parse.needs, vec![Need::Route]);

        let parse = editor("Idea @#");
        assert_eq!(parse.mode, EditorMode::Incomplete);
        assert_eq!(parse.body, "Idea");
        assert_eq!(parse.section, None);

        let parse = editor("Idea @#Ideas");
        assert_eq!(parse.mode, EditorMode::Incomplete);
        assert_eq!(parse.section.as_deref(), Some("Ideas"));

        let parse = editor("Idea @Notes#");
        assert_eq!(parse.mode, EditorMode::Bullet);
        assert_eq!(parse.route.as_deref(), Some("notes"));
        assert_eq!(parse.needs, vec![Need::Section]);

        // Delta from Lua: Hammerspoon left `@route#prefix` and plain
        // `@route` tokens to `bob capture`, so it reported mode "none".
        // Bob's own grammar resolves both, and this endpoint is
        // authoritative, so it reports the resolved capture instead.
        let parse = editor("Idea @notes#time:box");
        assert_eq!(parse.mode, EditorMode::Bullet);
        assert_eq!(parse.section.as_deref(), Some("time:box"));
        assert_eq!(parse.body, "Idea");

        let parse = editor("Idea @notes#Ideas");
        assert_eq!(parse.mode, EditorMode::Bullet);
        assert_eq!(parse.section.as_deref(), Some("Ideas"));

        let parse = editor("Task @dev");
        assert_eq!(parse.mode, EditorMode::Task);
        assert_eq!(parse.route.as_deref(), Some("dev"));
        assert_eq!(parse.body, "Task");
    }

    #[test]
    fn lua_leaves_invalid_or_unsupported_terminal_regions_to_bob_capture() {
        // These stay literal for both parsers: the terminal token is not a
        // marker Bob accepts, so nothing is extracted and nothing routes.
        for raw in [
            "Idea @Notes# %0",
            "Idea @Notes# %bad.header",
            "Idea @Notes# %18446744073709551616",
            "Idea @Notes# s:18446744073709551616",
            "Idea @Notes# p:18446744073709551616",
        ] {
            let parse = editor(raw);
            assert_eq!(parse.mode, EditorMode::Task, "{raw}");
            assert_eq!(parse.body, raw, "{raw}");
            assert_eq!(parse.route, None, "{raw}");
        }

        // Delta from Lua: Bob still consumes the first valid marker of each
        // kind before it stops at the duplicate or non-marker, so the body
        // loses that token while the route stays unresolved.
        for (raw, body) in [
            ("Idea @Notes# % %3", "Idea @Notes# %"),
            ("Idea @Notes# s:1 s:2", "Idea @Notes# s:1"),
            ("Idea @Notes# % s:1 %build_log", "Idea @Notes# %"),
            ("Idea @Notes# middle %", "Idea @Notes# middle"),
            ("Idea @Notes# p:1 p:2", "Idea @Notes# p:1"),
        ] {
            let parse = editor(raw);
            assert_eq!(parse.mode, EditorMode::Task, "{raw}");
            assert_eq!(parse.body, body, "{raw}");
            assert_eq!(parse.route, None, "{raw}");
        }

        // Delta from Lua: Bob resolves plain `@route` and `@route#prefix`
        // tokens itself, so these are complete captures rather than "none".
        let parse = editor("Task @dev %");
        assert_eq!(parse.mode, EditorMode::Task);
        assert_eq!(parse.route.as_deref(), Some("dev"));
        assert_eq!(parse.body, "Task");

        let parse = editor("Idea @notes#Ideas %");
        assert_eq!(parse.mode, EditorMode::Bullet);
        assert_eq!(parse.section.as_deref(), Some("Ideas"));
        assert_eq!(parse.body, "Idea");
    }

    #[test]
    fn editor_normalizes_intra_line_whitespace_like_execution() {
        // Only horizontal whitespace collapses within a physical line now;
        // `\n`/`\r` are line terminators, not normalized-away whitespace.
        let parse = editor(" \t buy\t  milk \t @groceries  ");
        assert_eq!(parse.body, "buy milk");
        assert_eq!(parse.route.as_deref(), Some("groceries"));
    }

    #[test]
    fn normalize_task_text_still_collapses_newlines_as_whitespace() {
        // `normalize_task_text` is a general-purpose whitespace collapser
        // reused for each physical line's own text; called directly on a
        // string that still has embedded newlines, it keeps collapsing them
        // exactly like `split_whitespace` always has.
        assert_eq!(
            normalize_task_text(" \n buy\t  milk \r\n @groceries  "),
            "buy milk @groceries"
        );
    }

    #[test]
    fn editor_serializes_snake_case_vocabulary() {
        let parse = editor("Call bank @Cash+");
        let value = serde_json::json!({
            "mode": parse.mode,
            "needs": parse.needs,
            "spans": parse.spans,
        });
        assert_eq!(value["mode"], "incomplete");
        assert_eq!(value["needs"][0], "task");
        assert_eq!(value["spans"][0]["kind"], "sub_bullet_route");
        assert_eq!(value["spans"][0]["start"], 10);
        assert_eq!(value["spans"][1]["kind"], "interactive_placeholder");
    }

    #[test]
    fn diagnostics_serialize_with_a_nullable_range_pair() {
        let parse = editor("Body @dev+bad.id");
        let value = serde_json::to_value(&parse.diagnostics).expect("json");
        assert_eq!(value[0]["severity"], "error");
        assert_eq!(value[0]["code"], "invalid_sub_bullet_block_id");
        assert_eq!(value[0]["range"][0], 5);
        assert_eq!(value[0]["range"][1], 16);
    }

    // -----------------------------------------------------------------
    // Cursor-aware completion
    // -----------------------------------------------------------------

    fn field(raw: &str, cursor: usize) -> Option<CompletionField> {
        completion_field_at(raw, cursor)
    }

    #[test]
    fn bare_at_completes_an_empty_route() {
        let completion = field("@", 1).expect("route field");
        assert_eq!(completion.context, CompletionContext::Route);
        assert_eq!(completion.route, None);
        assert_eq!(completion.query, "");
        assert_eq!(completion.replacement, (1, 1));
    }

    #[test]
    fn leading_route_fragment_completes_with_no_body_yet() {
        // The lone `@ca` token never routes for `bob capture` (no body text
        // exists), but it is still the fragment a live editor completes.
        let completion = field("@ca", 3).expect("route field");
        assert_eq!(completion.context, CompletionContext::Route);
        assert_eq!(completion.query, "ca");
        assert_eq!(completion.replacement, (1, 3));
    }

    #[test]
    fn cursor_mid_route_fragment_uses_the_prefix_before_the_cursor() {
        let completion = field("@cash buy milk", 2).expect("route field");
        assert_eq!(completion.context, CompletionContext::Route);
        assert_eq!(completion.query, "c");
        // The whole fragment is replaced regardless of where the cursor sits.
        assert_eq!(completion.replacement, (1, 5));
    }

    #[test]
    fn missing_route_portion_of_bullet_marker_completes_a_route() {
        let completion = field("Idea @#Ideas", 6).expect("route field");
        assert_eq!(completion.context, CompletionContext::Route);
        assert_eq!(completion.query, "");
        assert_eq!(completion.replacement, (6, 6));
    }

    #[test]
    fn missing_route_portion_of_pomodoro_marker_completes_a_route() {
        let completion = field("@:focus-123", 1).expect("route field");
        assert_eq!(completion.context, CompletionContext::Route);
        assert_eq!(completion.replacement, (1, 1));
    }

    #[test]
    fn missing_route_portion_of_task_block_id_marker_completes_a_route() {
        let completion = field("@^focus-123", 1).expect("route field");
        assert_eq!(completion.context, CompletionContext::Route);
        assert_eq!(completion.replacement, (1, 1));
    }

    #[test]
    fn missing_route_portion_of_sub_bullet_marker_completes_a_route() {
        let completion = field("@+focus-123", 1).expect("route field");
        assert_eq!(completion.context, CompletionContext::Route);
        assert_eq!(completion.replacement, (1, 1));
    }

    #[test]
    fn section_completes_after_a_resolved_route() {
        let completion = field("Idea @notes#Id", 14).expect("section field");
        assert_eq!(completion.context, CompletionContext::Section);
        assert_eq!(completion.route.as_deref(), Some("notes"));
        assert_eq!(completion.query, "Id");
        assert_eq!(completion.replacement, (12, 14));
    }

    #[test]
    fn pomodoro_block_id_completes_after_a_resolved_route() {
        let completion = field("Do work @Dev:foc", 16).expect("pomodoro id");
        assert_eq!(completion.context, CompletionContext::PomodoroBlockId);
        assert_eq!(completion.route.as_deref(), Some("dev"));
        assert_eq!(completion.query, "foc");
        assert_eq!(completion.replacement, (13, 16));
    }

    #[test]
    fn task_block_id_route_completes_but_authored_id_does_not() {
        let route = field("Do work @Dev^new-id", 12).expect("route field");
        assert_eq!(route.context, CompletionContext::Route);
        assert_eq!(route.query, "Dev");
        assert_eq!(route.replacement, (9, 12));

        assert_eq!(field("Do work @Dev^new-id", 13), None);
        assert_eq!(field("Do work @Dev^new-id", 19), None);
    }

    #[test]
    fn legacy_pomodoro_alias_completes_the_same_as_the_canonical_form() {
        let completion = field("Do work @!Dev:foc", 17).expect("pomodoro id");
        assert_eq!(completion.context, CompletionContext::PomodoroBlockId);
        assert_eq!(completion.route.as_deref(), Some("dev"));
        assert_eq!(completion.query, "foc");
        assert_eq!(completion.replacement, (14, 17));
    }

    #[test]
    fn task_completes_after_a_resolved_sub_bullet_route() {
        let completion = field("note @Cash+goog", 15).expect("task field");
        assert_eq!(completion.context, CompletionContext::Task);
        assert_eq!(completion.route.as_deref(), Some("cash"));
        assert_eq!(completion.query, "goog");
        assert_eq!(completion.replacement, (11, 15));
    }

    #[test]
    fn right_component_without_a_resolved_route_has_no_completion() {
        assert_eq!(field("@:foc", 5), None);
        assert_eq!(field("@^foc", 5), None);
        assert_eq!(field("@+foc", 5), None);
    }

    #[test]
    fn cursor_in_body_text_has_no_completion() {
        assert_eq!(field("buy milk @groceries", 4), None);
    }

    #[test]
    fn cursor_on_a_middle_token_has_no_completion() {
        // `@home` here stays literal body text because the leading `@work`
        // marker wins, exactly like `parse_for_editor` reports it.
        assert_eq!(field("@work buy milk @home", 17), None);
        assert!(field("@work buy milk @home", 0).is_some());
    }

    #[test]
    fn cursor_past_a_trailing_space_has_no_completion() {
        assert_eq!(field("Body @dev ", 10), None);
    }

    #[test]
    fn retired_double_colon_marker_has_no_completion_field() {
        assert_eq!(field("Do work @Dev::new-id", 12), None);
        assert_eq!(field("Do work @Dev::new-id", 14), None);
        assert_eq!(field("Do work @Dev::new-id", 20), None);
        assert_eq!(field("@::focus-123", 1), None);
    }

    #[test]
    fn invalid_block_id_characters_still_produce_a_field() {
        // The field extractor never validates block-ID syntax; a discovery
        // scan naturally returns no candidates for a query no real block ID
        // could match, without a separate invalid/error path here.
        let completion = field("note @dev+bad.id", 16).expect("task field");
        assert_eq!(completion.context, CompletionContext::Task);
        assert_eq!(completion.route.as_deref(), Some("dev"));
        assert_eq!(completion.query, "bad.id");
    }

    #[test]
    fn terminal_markers_do_not_interfere_with_route_completion() {
        let completion = field("body p:2 s:1 % @ca", 18).expect("route field");
        assert_eq!(completion.context, CompletionContext::Route);
        assert_eq!(completion.query, "ca");
        assert_eq!(completion.replacement, (16, 18));
    }

    #[test]
    fn completion_field_stays_on_unicode_scalar_boundaries() {
        let raw = "caf\u{e9} \u{1f680} @Cash+goog-exit";
        for cursor in
            (0..=raw.len()).filter(|&index| raw.is_char_boundary(index))
        {
            let _ = field(raw, cursor);
        }
    }

    #[test]
    fn completion_field_uses_byte_offsets_after_multibyte_prefix_text() {
        let raw = "caf\u{e9} \u{1f680} @Cash+goog-exit";
        // "@Cash+goog-exit" starts at byte 11, right after the multibyte
        // café and rocket-emoji body text.
        assert_eq!(&raw[11..16], "@Cash");
        assert_eq!(&raw[17..26], "goog-exit");

        let route = field(raw, 15).expect("route field");
        assert_eq!(route.context, CompletionContext::Route);
        assert_eq!(route.query, "Cas");
        assert_eq!(route.replacement, (12, 16));

        let task = field(raw, 21).expect("task field");
        assert_eq!(task.context, CompletionContext::Task);
        assert_eq!(task.route.as_deref(), Some("cash"));
        assert_eq!(task.query, "goog");
        assert_eq!(task.replacement, (17, 26));
        assert_eq!(task.block_id, None);
    }

    #[test]
    fn task_section_completes_after_hash_on_a_sub_bullet_marker() {
        let raw = "note @Cash+goog#req";
        let hash = raw.find('#').expect("hash");
        let completion = field(raw, raw.len()).expect("task section");
        assert_eq!(completion.context, CompletionContext::TaskSection);
        assert_eq!(completion.route.as_deref(), Some("cash"));
        assert_eq!(completion.block_id.as_deref(), Some("goog"));
        assert_eq!(completion.query, "req");
        assert_eq!(completion.replacement, (hash + 1, raw.len()));
    }

    #[test]
    fn empty_selector_after_hash_is_a_zero_length_task_section_field() {
        let raw = "note @Cash+goog#";
        let completion = field(raw, raw.len()).expect("task section");
        assert_eq!(completion.context, CompletionContext::TaskSection);
        assert_eq!(completion.route.as_deref(), Some("cash"));
        assert_eq!(completion.block_id.as_deref(), Some("goog"));
        assert_eq!(completion.query, "");
        assert_eq!(completion.replacement, (raw.len(), raw.len()));
    }

    #[test]
    fn cursor_in_route_or_block_id_of_three_component_marker_keeps_existing_contexts(
    ) {
        let raw = "note @Cash+goog#req";
        let at = raw.find('@').expect("at");
        let plus = raw.find('+').expect("plus");
        let hash = raw.find('#').expect("hash");

        let route = field(raw, at + 3).expect("route");
        assert_eq!(route.context, CompletionContext::Route);
        assert_eq!(route.block_id, None);
        assert_eq!(route.replacement, (at + 1, plus));

        let task = field(raw, plus + 3).expect("task");
        assert_eq!(task.context, CompletionContext::Task);
        assert_eq!(task.block_id, None);
        assert_eq!(task.query, "go");
        assert_eq!(task.replacement, (plus + 1, hash));
        assert_eq!(&raw[plus + 1..hash], "goog");
    }

    #[test]
    fn hash_separator_is_not_part_of_block_id_or_section_replacement() {
        let raw = "note @Cash+goog#";
        let plus = raw.find('+').expect("plus");
        let hash = raw.find('#').expect("hash");
        let task = field(raw, hash).expect("cursor on hash stays task");
        assert_eq!(task.context, CompletionContext::Task);
        assert_eq!(task.replacement, (plus + 1, hash));

        let section = field(raw, hash + 1).expect("cursor after hash");
        assert_eq!(section.context, CompletionContext::TaskSection);
        assert_eq!(section.replacement, (hash + 1, hash + 1));
    }

    #[test]
    fn empty_block_id_with_section_still_yields_a_task_section_field() {
        let raw = "note @Cash+#req";
        let completion = field(raw, raw.len()).expect("task section");
        assert_eq!(completion.context, CompletionContext::TaskSection);
        assert_eq!(completion.route.as_deref(), Some("cash"));
        assert_eq!(completion.block_id, None);
        assert_eq!(completion.query, "req");
    }

    #[test]
    fn three_component_right_side_without_a_resolved_route_has_no_completion() {
        assert_eq!(field("@+#req", 6), None);
        assert_eq!(field("@+id#req", 8), None);
    }

    #[test]
    fn completion_field_stays_on_boundaries_of_a_three_component_marker() {
        let raw = "caf\u{e9} \u{1f680} @Cash+goog-exit#req";
        for cursor in
            (0..=raw.len()).filter(|&index| raw.is_char_boundary(index))
        {
            let _ = field(raw, cursor);
        }
        let hash = raw.find('#').expect("hash");
        let section = field(raw, raw.len()).expect("task section");
        assert_eq!(section.context, CompletionContext::TaskSection);
        assert_eq!(section.block_id.as_deref(), Some("goog-exit"));
        assert_eq!(section.replacement, (hash + 1, raw.len()));
    }

    #[test]
    fn leading_three_component_marker_completes_each_component() {
        let raw = "@Cash+goog#req body";
        let plus = raw.find('+').expect("plus");
        let hash = raw.find('#').expect("hash");
        let space = raw.find(' ').expect("space");

        let route = field(raw, 3).expect("route");
        assert_eq!(route.context, CompletionContext::Route);
        assert_eq!(route.replacement, (1, plus));

        let task = field(raw, plus + 2).expect("task");
        assert_eq!(task.context, CompletionContext::Task);
        assert_eq!(task.replacement, (plus + 1, hash));

        let section = field(raw, hash + 2).expect("section");
        assert_eq!(section.context, CompletionContext::TaskSection);
        assert_eq!(section.replacement, (hash + 1, space));
        assert_eq!(section.query, "r");
    }

    // -----------------------------------------------------------------
    // Line-aware capture: physical line splitting and bullet stripping.
    // -----------------------------------------------------------------

    fn line_texts(raw: &str) -> Vec<&str> {
        split_physical_lines(raw)
            .iter()
            .map(|line| line.text)
            .collect()
    }

    #[test]
    fn split_physical_lines_treats_lf_crlf_and_bare_cr_as_terminators() {
        assert_eq!(line_texts("a\nb"), vec!["a", "b"]);
        assert_eq!(line_texts("a\r\nb"), vec!["a", "b"]);
        assert_eq!(line_texts("a\rb"), vec!["a", "b"]);
        assert_eq!(line_texts("a\r\n\rb\n\r\nc"), vec!["a", "", "b", "", "c"]);
    }

    #[test]
    fn split_physical_lines_drops_only_one_trailing_terminator() {
        assert_eq!(line_texts("a\n"), vec!["a"]);
        assert_eq!(line_texts("a\r\n"), vec!["a"]);
        assert_eq!(line_texts("a\n\n"), vec!["a", ""]);
        assert_eq!(line_texts(""), Vec::<&str>::new());
    }

    #[test]
    fn split_physical_lines_reports_byte_offsets_excluding_terminators() {
        let raw = "ab\r\ncd\nef";
        let lines = split_physical_lines(raw);
        assert_eq!(
            lines
                .iter()
                .map(|line| (line.text, line.start, line.end))
                .collect::<Vec<_>>(),
            vec![("ab", 0, 2), ("cd", 4, 6), ("ef", 7, 9)]
        );
        for line in &lines {
            assert_eq!(&raw[line.start..line.end], line.text);
        }
    }

    #[test]
    fn split_capture_draft_reports_ranges_and_ignores_separator_runs() {
        let raw = " \nfirst\n- child\n\n\nsecond @work\r\n\r\nthird";
        let items = split_capture_draft(raw).items;
        let summaries = items
            .iter()
            .map(|item| {
                (
                    item.index,
                    item.line_start,
                    item.line_end,
                    &raw[item.start..item.end],
                )
            })
            .collect::<Vec<_>>();

        assert_eq!(
            summaries,
            vec![
                (0, 2, 3, "first\n- child"),
                (1, 6, 6, "second @work"),
                (2, 8, 8, "third"),
            ]
        );
    }

    #[test]
    fn authored_line_classifier_accepts_first_level_and_nested_items() {
        for (raw, body, depth, body_start) in [
            ("- body", "body", AuthoredDepth::First, 2),
            ("* body", "body", AuthoredDepth::First, 2),
            ("+ body", "body", AuthoredDepth::First, 2),
            ("-\tbody", "body", AuthoredDepth::First, 2),
            ("-   body", "body", AuthoredDepth::First, 4),
            ("  - nested", "nested", AuthoredDepth::Nested, 4),
            ("  * nested", "nested", AuthoredDepth::Nested, 4),
            ("  +\tnested", "nested", AuthoredDepth::Nested, 4),
        ] {
            let line = RawLine {
                text: raw,
                start: 10,
                end: 10 + raw.len(),
            };
            let AuthoredLineClass::Item(item) = classify_authored_line(line)
            else {
                panic!("expected item for {raw:?}");
            };
            assert_eq!(item.body, body, "{raw}");
            assert_eq!(item.depth, depth, "{raw}");
            assert_eq!(item.body_start, 10 + body_start, "{raw}");
        }
    }

    #[test]
    fn authored_line_classifier_accepts_placeholders_without_items() {
        for raw in ["", "   ", "- ", "-\t", "-", " -", "  -", "  - "] {
            let line = RawLine {
                text: raw,
                start: 0,
                end: raw.len(),
            };
            assert_eq!(
                classify_authored_line(line),
                AuthoredLineClass::EmptyOrPlaceholder,
                "{raw:?}"
            );
        }
    }

    #[test]
    fn authored_line_classifier_rejects_every_other_shape() {
        for raw in [
            "body",
            " - indented",
            "   - too deep",
            "\t- tabbed",
            "-body",
            "#body",
        ] {
            let line = RawLine {
                text: raw,
                start: 0,
                end: raw.len(),
            };
            assert_eq!(
                classify_authored_line(line),
                AuthoredLineClass::Invalid,
                "{raw:?}"
            );
        }
    }

    // -----------------------------------------------------------------
    // Line-aware capture: execution grammar.
    // -----------------------------------------------------------------

    fn execute(raw: &str) -> Result<ParsedCaptureText, String> {
        parse_capture_text_with_clip_control(raw, None, None, true)
    }

    fn sub_bullet_bodies(sub_bullets: &[AuthoredSubBullet]) -> Vec<&str> {
        sub_bullets.iter().map(|item| item.body.as_str()).collect()
    }

    fn sub_bullet_depths(sub_bullets: &[AuthoredSubBullet]) -> Vec<u8> {
        sub_bullets.iter().map(|item| item.depth.level()).collect()
    }

    #[test]
    fn execution_renders_authored_children_in_source_order() {
        let parsed =
            execute("@work parent line\n- first child\n- second child")
                .expect("parse");
        assert_eq!(parsed.body, "parent line");
        assert_eq!(parsed.route.as_deref(), Some("work"));
        assert_eq!(
            sub_bullet_bodies(&parsed.sub_bullets),
            vec!["first child", "second child"]
        );
        assert_eq!(sub_bullet_depths(&parsed.sub_bullets), vec![1, 1]);
    }

    #[test]
    fn execution_tracks_nested_children_under_the_nearest_first_level_owner() {
        let parsed = execute(
            "@work parent line\n- first child\n  - first detail\n- second child\n  - second detail",
        )
        .expect("parse");
        assert_eq!(
            sub_bullet_bodies(&parsed.sub_bullets),
            vec![
                "first child",
                "first detail",
                "second child",
                "second detail"
            ]
        );
        assert_eq!(sub_bullet_depths(&parsed.sub_bullets), vec![1, 2, 1, 2]);
    }

    #[test]
    fn execution_nested_placeholders_do_not_require_or_clear_an_owner() {
        let parsed = execute("parent\n  - \n- first child\n  -\n  - detail")
            .expect("parse");
        assert_eq!(
            sub_bullet_bodies(&parsed.sub_bullets),
            vec!["first child", "detail"]
        );
        assert_eq!(sub_bullet_depths(&parsed.sub_bullets), vec![1, 2]);
    }

    #[test]
    fn execution_treats_crlf_and_bare_cr_children_like_lf() {
        for raw in [
            "@work parent\n- child one\n- child two",
            "@work parent\r\n- child one\r\n- child two",
            "@work parent\r- child one\r- child two",
        ] {
            let parsed = execute(raw).unwrap_or_else(|error| panic!("{error}"));
            assert_eq!(parsed.body, "parent");
            assert_eq!(
                sub_bullet_bodies(&parsed.sub_bullets),
                vec!["child one", "child two"],
                "{raw}"
            );
            assert_eq!(sub_bullet_depths(&parsed.sub_bullets), vec![1, 1]);
        }
    }

    #[test]
    fn execution_skips_placeholder_child_lines() {
        let parsed = execute("parent\n- real child\n- \n-\t\n").expect("parse");
        assert_eq!(sub_bullet_bodies(&parsed.sub_bullets), vec!["real child"]);
    }

    #[test]
    fn execution_single_item_parser_rejects_blank_line_batches() {
        let error = execute("parent\n\nsecond item").unwrap_err();
        assert_eq!(
            error,
            "capture text contains multiple blank-line-separated items"
        );
    }

    #[test]
    fn execution_batch_parser_prefixes_item_and_line_context() {
        let error = parse_capture_draft_with_clip_control(
            "parent\n\nsecond\n  - orphan",
            None,
            None,
            true,
        )
        .unwrap_err();
        assert_eq!(
            error,
            "capture item 2 starting on line 3: capture line 4 is a nested bullet but has no preceding first-level authored bullet to attach to"
        );
    }

    #[test]
    fn execution_rejects_indented_or_deeper_child_lines() {
        let error = execute("parent\n   - too deep").unwrap_err();
        assert_eq!(
            error,
            "capture line 2 must be a column-zero bullet or a two-space nested \
bullet using \"-\", \"*\", or \"+\" followed by a space or tab, or be left \
blank"
        );
    }

    #[test]
    fn execution_rejects_orphaned_nested_child_lines() {
        let error = execute("parent\n  - orphan").unwrap_err();
        assert_eq!(
            error,
            "capture line 2 is a nested bullet but has no preceding first-level \
authored bullet to attach to"
        );
    }

    #[test]
    fn execution_rejects_nonbullet_continuation_prose() {
        let error =
            execute("parent\n- real child\ncontinuation prose").unwrap_err();
        assert!(error.contains("capture line 3"), "{error}");
    }

    #[test]
    fn execution_rejects_a_child_emptied_by_marker_removal() {
        let error = execute("parent\n- s:1").unwrap_err();
        assert_eq!(
            error,
            "capture line 2 has no text left after its capture markers \
were removed"
        );

        let error = execute("parent\n- p:2\n- @work").unwrap_err();
        assert_eq!(
            error,
            "capture line 2 has no text left after its capture markers \
were removed"
        );
    }

    #[test]
    fn execution_composes_a_trailing_marker_from_any_child_line() {
        let parsed = execute(
            "Prepare the launch review\n- Confirm the owner\n- Attach the checklist @work p:1 s:2",
        )
        .expect("parse");
        assert_eq!(parsed.body, "Prepare the launch review");
        assert_eq!(parsed.route.as_deref(), Some("work"));
        assert_eq!(parsed.priority_level, Some(1));
        assert_eq!(parsed.scheduled_offset, Some(2));
        assert_eq!(
            sub_bullet_bodies(&parsed.sub_bullets),
            vec!["Confirm the owner", "Attach the checklist"]
        );
        assert_eq!(sub_bullet_depths(&parsed.sub_bullets), vec![1, 1]);
    }

    #[test]
    fn execution_rejects_duplicate_route_markers_across_lines() {
        let error = execute("@work parent\n- child @home").unwrap_err();
        assert!(error.contains("route/mode marker"), "{error}");
        assert!(error.contains("only one line"), "{error}");
    }

    #[test]
    fn execution_rejects_duplicate_schedule_priority_and_clip_markers_across_lines(
    ) {
        assert!(execute("@work parent s:1\n- child s:2")
            .unwrap_err()
            .contains("schedule marker"));
        assert!(execute("@work parent p:1\n- child p:2")
            .unwrap_err()
            .contains("priority marker"));
        assert!(execute("@work parent %\n- child %")
            .unwrap_err()
            .contains("clipboard marker"));
    }

    #[test]
    fn execution_allows_the_same_marker_kind_once_across_the_whole_draft() {
        let parsed =
            execute("parent s:1\n- child one\n- child two p:3").expect("parse");
        assert_eq!(parsed.scheduled_offset, Some(1));
        assert_eq!(parsed.priority_level, Some(3));
    }

    #[test]
    fn execution_preserves_unicode_child_bodies() {
        let parsed = execute("café parent\n- \u{1f680} launch\n- \u{e9}tude")
            .expect("parse");
        assert_eq!(parsed.body, "café parent");
        assert_eq!(
            sub_bullet_bodies(&parsed.sub_bullets),
            vec!["\u{1f680} launch", "\u{e9}tude"]
        );
    }

    #[test]
    fn execution_forced_route_keeps_child_markers_literal() {
        let parsed = parse_capture_text_with_clip_control(
            "parent\n- child @home",
            Some("work"),
            None,
            true,
        )
        .expect("parse");
        assert_eq!(parsed.route.as_deref(), Some("work"));
        assert_eq!(sub_bullet_bodies(&parsed.sub_bullets), vec!["child @home"]);
    }

    #[test]
    fn execution_ordinary_single_line_capture_has_no_sub_bullets() {
        let parsed = execute("buy milk @groceries").expect("parse");
        assert!(parsed.sub_bullets.is_empty());
    }

    #[test]
    fn execution_retired_double_colon_is_a_usage_error() {
        for raw in [
            "Do thing @Dev::Foo-Bar",
            "@Dev::Foo-Bar Do thing",
            "body @cash::",
            "body @::id",
        ] {
            let error = execute(raw).expect_err(raw);
            assert_eq!(error, RETIRED_DOUBLE_COLON_ERROR, "{raw}");
        }
    }

    #[test]
    fn execution_plus_sub_bullet_does_not_conflict_with_authored_plus_child() {
        let parsed = execute(
            "parent line\n+ authored child @dev+focus-123\n+ second child",
        )
        .expect("parse");
        assert_eq!(parsed.body, "parent line");
        assert_eq!(parsed.route.as_deref(), Some("dev"));
        assert_eq!(
            parsed.kind,
            CaptureKind::SubBullet {
                target: SubBulletTarget::BlockId("focus-123".to_string()),
                section: None,
            }
        );
        assert_eq!(
            sub_bullet_bodies(&parsed.sub_bullets),
            vec!["authored child", "second child"]
        );
    }

    #[test]
    fn execution_parses_three_component_sub_bullet_markers() {
        let cases = [
            (
                "Postgres 17 minimum @foo+bar#requirements",
                "Postgres 17 minimum",
                "requirements",
                None,
                None,
            ),
            (
                "@foo+bar#requirements Postgres 17 minimum",
                "Postgres 17 minimum",
                "requirements",
                None,
                None,
            ),
            (
                "note @foo+bar#future-work s:1",
                "note",
                "future-work",
                Some(1),
                None,
            ),
            (
                "note s:1 @foo+bar#future-work",
                "note",
                "future-work",
                Some(1),
                None,
            ),
            ("note p:2 @foo+bar#Q&A", "note", "Q&A", None, Some(2)),
            ("note @foo+bar#Q&A p:2", "note", "Q&A", None, Some(2)),
            (
                "note %log @foo+bar#non-goals",
                "note",
                "non-goals",
                None,
                None,
            ),
            (
                "note @foo+bar#non-goals %log",
                "note",
                "non-goals",
                None,
                None,
            ),
        ];
        for (raw, body, section, scheduled, priority) in cases {
            let parsed =
                execute(raw).unwrap_or_else(|error| panic!("{raw}: {error}"));
            assert_eq!(parsed.body, body, "{raw}");
            assert_eq!(parsed.route.as_deref(), Some("foo"), "{raw}");
            assert_eq!(parsed.scheduled_offset, scheduled, "{raw}");
            assert_eq!(parsed.priority_level, priority, "{raw}");
            assert_eq!(
                parsed.kind,
                CaptureKind::SubBullet {
                    target: SubBulletTarget::BlockId("bar".to_string()),
                    section: Some(TaskSectionSelector {
                        text: section.to_string(),
                        exact: false,
                    }),
                },
                "{raw}"
            );
        }
    }

    #[test]
    fn execution_three_component_marker_composes_on_multiline_first_line_only()
    {
        let parsed = execute(
            "@foo+bar#requirements parent line\n- first child\n- second child",
        )
        .expect("parse");
        assert_eq!(parsed.body, "parent line");
        assert_eq!(
            parsed.kind,
            CaptureKind::SubBullet {
                target: SubBulletTarget::BlockId("bar".to_string()),
                section: Some(TaskSectionSelector {
                    text: "requirements".to_string(),
                    exact: false,
                }),
            }
        );
        assert_eq!(
            sub_bullet_bodies(&parsed.sub_bullets),
            vec!["first child", "second child"]
        );

        let parsed = execute(
            "parent line\n- first child @foo+bar#requirements\n- second child",
        )
        .expect("trailing child marker");
        assert_eq!(parsed.body, "parent line");
        assert_eq!(parsed.route.as_deref(), Some("foo"));
        assert_eq!(
            sub_bullet_bodies(&parsed.sub_bullets),
            vec!["first child", "second child"]
        );

        let parsed = execute("parent @foo+bar#req later\n- child")
            .expect("mid-text stays literal");
        assert_eq!(parsed.body, "parent @foo+bar#req later");
        assert_eq!(parsed.kind, CaptureKind::Task);
        assert_eq!(parsed.route, None);
    }

    #[test]
    fn execution_keeps_pomodoro_note_and_other_families_unchanged() {
        let parsed =
            execute("remembered to bump the timeout #").expect("bare hash");
        assert_eq!(parsed.kind, CaptureKind::PomodoroNote);
        assert_eq!(parsed.body, "remembered to bump the timeout");

        let parsed = execute("Some note @foo#Ideas").expect("note bullet");
        assert!(matches!(
            parsed.kind,
            CaptureKind::Bullet {
                section_prefix: Some(ref prefix),
                exact: false,
            } if prefix == "Ideas"
        ));

        let parsed = execute("Do thing @foo^id").expect("caret");
        assert!(matches!(parsed.kind, CaptureKind::TaskWithBlockId { .. }));

        let parsed = execute("Do thing @foo:id").expect("colon");
        assert!(matches!(parsed.kind, CaptureKind::Pomodoro { .. }));

        let error = execute("Do thing @foo::id").expect_err("retired");
        assert_eq!(error, RETIRED_DOUBLE_COLON_ERROR);
    }

    #[test]
    fn execution_forced_route_keeps_retired_and_special_markers_literal() {
        let parsed = parse_capture_text_with_clip_control(
            "Do thing @dev::id @dev+parent @dev^new-id",
            Some("work"),
            None,
            true,
        )
        .expect("parse");
        assert_eq!(parsed.route.as_deref(), Some("work"));
        assert_eq!(parsed.kind, CaptureKind::Task);
        assert_eq!(parsed.body, "Do thing @dev::id @dev+parent @dev^new-id");
    }

    // -----------------------------------------------------------------
    // Line-aware capture: editor grammar.
    // -----------------------------------------------------------------

    #[test]
    fn editor_reports_sub_bullets_for_a_multiline_draft() {
        let parse = editor("@work parent\n- first child\n- second child");
        assert_eq!(parse.body, "parent");
        assert_eq!(parse.route.as_deref(), Some("work"));
        assert_eq!(
            sub_bullet_bodies(&parse.sub_bullets),
            vec!["first child", "second child"]
        );
        assert_eq!(sub_bullet_depths(&parse.sub_bullets), vec![1, 1]);
        assert!(parse.diagnostics.is_empty());
    }

    #[test]
    fn editor_reports_nested_sub_bullets_and_depths() {
        let parse = editor(
            "@work parent\n- first child\n  - first detail\n- second child\n  - second detail",
        );
        assert_eq!(
            sub_bullet_bodies(&parse.sub_bullets),
            vec![
                "first child",
                "first detail",
                "second child",
                "second detail"
            ]
        );
        assert_eq!(sub_bullet_depths(&parse.sub_bullets), vec![1, 2, 1, 2]);
        assert!(parse.diagnostics.is_empty());
    }

    #[test]
    fn editor_diagnoses_an_invalid_child_line_without_failing() {
        let parse = editor("parent\n   - nested");
        assert_eq!(parse.body, "parent");
        assert!(parse.sub_bullets.is_empty());
        assert_eq!(codes(&parse), vec!["invalid_child_line"]);
        let raw = "parent\n   - nested";
        let expected_start = raw.find("   - nested").expect("line 2 offset");
        assert_eq!(
            parse.diagnostics[0].range,
            Some((expected_start, raw.len()))
        );
    }

    #[test]
    fn editor_diagnoses_an_orphaned_nested_child_without_failing() {
        let raw = "parent\n  - orphan";
        let parse = editor(raw);
        assert_eq!(parse.body, "parent");
        assert!(parse.sub_bullets.is_empty());
        assert_eq!(codes(&parse), vec!["orphaned_nested_bullet"]);
        let expected_start = raw.find("  - orphan").expect("line 2 offset");
        assert_eq!(
            parse.diagnostics[0].range,
            Some((expected_start, raw.len()))
        );
    }

    #[test]
    fn editor_diagnoses_a_child_emptied_by_marker_removal() {
        let raw = "parent\n- s:1";
        let parse = editor(raw);
        assert!(parse.sub_bullets.is_empty());
        assert_eq!(codes(&parse), vec!["empty_child_after_markers"]);
        let line2_start = raw.find("- s:1").expect("line 2 offset");
        assert_eq!(parse.diagnostics[0].range, Some((line2_start, raw.len())));
    }

    #[test]
    fn editor_diagnoses_duplicate_markers_across_lines_but_keeps_the_first() {
        let parse = editor("@work parent\n- child @home");
        assert_eq!(parse.route.as_deref(), Some("work"));
        assert_eq!(codes(&parse), vec!["duplicate_capture_marker"]);
        assert!(
            parse.diagnostics[0].message.contains("route/mode marker"),
            "{:?}",
            parse.diagnostics
        );

        let parse = editor("parent s:1\n- child s:2");
        assert_eq!(codes(&parse), vec!["duplicate_capture_marker"]);
        assert!(parse.diagnostics[0].message.contains("schedule marker"));
    }

    #[test]
    fn editor_child_line_markers_extend_spans_with_absolute_offsets() {
        let raw = "parent\n- child @work";
        let parse = editor(raw);
        assert_eq!(parse.route.as_deref(), Some("work"));
        let route_span = parse
            .spans
            .iter()
            .find(|span| span.kind == SpanKind::Route)
            .expect("route span");
        assert_eq!(&raw[route_span.start..route_span.end], "@work");
    }

    #[test]
    fn editor_placeholder_child_lines_produce_no_sub_bullet_or_diagnostic() {
        let parse = editor("parent\n- real child\n- \n");
        assert_eq!(sub_bullet_bodies(&parse.sub_bullets), vec!["real child"]);
        assert!(parse.diagnostics.is_empty());
    }

    #[test]
    fn editor_child_line_alone_can_resolve_the_capture_mode() {
        // The parent has no marker of its own; the child's trailing marker
        // becomes the whole capture's mode, exactly like execution.
        let parse = editor("plain parent\n- do it @dev:focus-1");
        assert_eq!(parse.mode, EditorMode::PomodoroTask);
        assert_eq!(parse.route.as_deref(), Some("dev"));
        assert_eq!(parse.block_id.as_deref(), Some("focus-1"));
        assert_eq!(sub_bullet_bodies(&parse.sub_bullets), vec!["do it"]);
    }

    // -----------------------------------------------------------------
    // Line-aware capture: cursor-aware completion.
    // -----------------------------------------------------------------

    #[test]
    fn completion_on_a_child_line_completes_a_trailing_route() {
        let raw = "parent line\n- context @ca";
        let completion = field(raw, raw.len()).expect("route field");
        assert_eq!(completion.context, CompletionContext::Route);
        assert_eq!(completion.query, "ca");
        let at_index = raw.rfind('@').expect("at sign");
        assert_eq!(completion.replacement, (at_index + 1, raw.len()));
    }

    #[test]
    fn completion_on_a_nested_child_line_completes_a_trailing_route() {
        let raw = "parent line\n- first child\n  - context @ca";
        let completion = field(raw, raw.len()).expect("route field");
        assert_eq!(completion.context, CompletionContext::Route);
        assert_eq!(completion.query, "ca");
        let at_index = raw.rfind('@').expect("at sign");
        assert_eq!(completion.replacement, (at_index + 1, raw.len()));
    }

    #[test]
    fn completion_on_nested_prefix_or_orphaned_nested_line_is_empty() {
        let raw = "parent line\n- first child\n  - context @ca";
        let nested_line_start = raw.rfind("  -").expect("nested line");
        assert_eq!(field(raw, nested_line_start), None);
        assert_eq!(field(raw, nested_line_start + 1), None);
        assert_eq!(field(raw, nested_line_start + 3), None);

        let orphan = "parent line\n  - context @ca";
        assert_eq!(field(orphan, orphan.len()), None);
    }

    #[test]
    fn completion_works_on_an_earlier_child_line_not_only_the_last() {
        // A marker on the *first* child line, with more lines after it,
        // still completes -- completion is scoped per physical line, not
        // just to the leading/trailing ends of the whole draft.
        let raw = "parent line\n- first @ca\n- second child\n- third child";
        let at_index = raw.find('@').expect("at sign");
        let cursor = at_index + 3;
        let completion = field(raw, cursor).expect("route field");
        assert_eq!(completion.context, CompletionContext::Route);
        assert_eq!(completion.query, "ca");
        let line_end = raw.find("\n- second").expect("line end");
        assert_eq!(completion.replacement, (at_index + 1, line_end));
    }

    #[test]
    fn completion_inside_a_child_bullet_marker_has_no_completion() {
        let raw = "parent\n- @work";
        // Cursor sitting inside the "- " marker itself, before the body.
        let dash_index = raw.rfind("- ").expect("marker");
        assert_eq!(field(raw, dash_index), None);
        assert_eq!(field(raw, dash_index + 1), None);
    }

    #[test]
    fn completion_on_the_parent_line_still_supports_leading_markers() {
        let raw = "@ca parent\n- child";
        let completion = field(raw, 3).expect("route field");
        assert_eq!(completion.context, CompletionContext::Route);
        assert_eq!(completion.query, "ca");
        assert_eq!(completion.replacement, (1, 3));
    }

    #[test]
    fn completion_on_a_child_line_never_offers_a_leading_route() {
        // On the parent line a lone `@ca` fragment still completes (see
        // `leading_route_fragment_completes_with_no_body_yet`), because the
        // first line keeps the established leading-route form. A child line
        // never gets that treatment, so the identical lone fragment here is
        // not completable at all.
        let raw = "parent\n- @ca";
        assert_eq!(field(raw, raw.len()), None);
    }

    // -----------------------------------------------------------------
    // Global @@ destination declaration.
    // -----------------------------------------------------------------

    fn draft_items(raw: &str) -> Vec<(usize, usize, &str)> {
        split_capture_draft(raw)
            .items
            .iter()
            .map(|item| {
                (item.index, item.line_start, &raw[item.start..item.end])
            })
            .collect()
    }

    #[test]
    fn split_capture_draft_strips_declaration_only_lines() {
        assert_eq!(
            draft_items("@@foo\nFirst task\n\nSecond task"),
            vec![(0, 2, "First task"), (1, 4, "Second task")]
        );
        assert_eq!(
            draft_items("@@foo\n\nFirst task\n\nSecond task"),
            vec![(0, 3, "First task"), (1, 5, "Second task")]
        );
    }

    #[test]
    fn split_capture_draft_ignores_leading_blanks_and_crlf() {
        let raw = "\r\n\n@@foo\r\nFirst";
        let draft = split_capture_draft(raw);
        let declaration = draft.declarations[0];
        assert_eq!(declaration.token.text, "@@foo");
        assert_eq!(
            &raw[declaration.token.start..declaration.token.end],
            "@@foo"
        );
        assert_eq!(declaration.line_number, 3);
        assert_eq!(draft_items(raw), vec![(0, 4, "First")]);
    }

    #[test]
    fn execution_inherits_a_global_task_route_unless_an_item_overrides() {
        let draft = parse_capture_draft_with_clip_control(
            "@@Foo\nFirst task\n\nSecond task @bar\n\nThird task",
            None,
            None,
            true,
        )
        .expect("parse");
        let global = draft.global.expect("global");
        assert_eq!(global.route, "foo");
        assert_eq!(global.block_id, None);
        assert_eq!(draft.items[0].parsed.body, "First task");
        assert_eq!(draft.items[0].parsed.route.as_deref(), Some("foo"));
        assert_eq!(draft.items[0].parsed.kind, CaptureKind::Task);
        assert_eq!(draft.items[1].parsed.body, "Second task");
        assert_eq!(draft.items[1].parsed.route.as_deref(), Some("bar"));
        assert_eq!(draft.items[2].parsed.body, "Third task");
        assert_eq!(draft.items[2].parsed.route.as_deref(), Some("foo"));
    }

    #[test]
    fn execution_inherits_a_global_sub_bullet_and_keeps_authored_children() {
        let draft = parse_capture_draft_with_clip_control(
            "@@foo+a-id\nFirst note\n- authored detail\n\nSecond note",
            None,
            None,
            true,
        )
        .expect("parse");
        assert_eq!(
            draft.global.as_ref().unwrap().block_id.as_deref(),
            Some("a-id")
        );
        assert_eq!(
            draft.items[0].parsed.kind,
            CaptureKind::SubBullet {
                target: SubBulletTarget::BlockId("a-id".to_string()),
                section: None,
            }
        );
        assert_eq!(
            sub_bullet_bodies(&draft.items[0].parsed.sub_bullets),
            vec!["authored detail"]
        );
        assert_eq!(
            draft.items[1].parsed.kind,
            CaptureKind::SubBullet {
                target: SubBulletTarget::BlockId("a-id".to_string()),
                section: None,
            }
        );
    }

    #[test]
    fn execution_local_markers_override_a_global_declaration() {
        let draft = parse_capture_draft_with_clip_control(
            "@@foo+a-id\nKeep\n\nBullet @bar#Ideas\n\nId @bar^b-id\n\nPomo @bar:p-id\n\nChild @bar+b-id\n\nNote #",
            None,
            None,
            true,
        )
        .expect("parse");
        assert!(matches!(
            draft.items[0].parsed.kind,
            CaptureKind::SubBullet { .. }
        ));
        assert!(matches!(
            draft.items[1].parsed.kind,
            CaptureKind::Bullet { .. }
        ));
        assert_eq!(draft.items[1].parsed.route.as_deref(), Some("bar"));
        assert!(matches!(
            draft.items[2].parsed.kind,
            CaptureKind::TaskWithBlockId { .. }
        ));
        assert!(matches!(
            draft.items[3].parsed.kind,
            CaptureKind::Pomodoro { .. }
        ));
        assert!(matches!(
            draft.items[4].parsed.kind,
            CaptureKind::SubBullet { .. }
        ));
        assert_eq!(draft.items[4].parsed.route.as_deref(), Some("bar"));
        assert_eq!(draft.items[5].parsed.kind, CaptureKind::PomodoroNote);
        assert_eq!(draft.items[5].parsed.route, None);
    }

    #[test]
    fn execution_rejects_a_declaration_only_draft() {
        let error =
            parse_capture_draft_with_clip_control("@@foo", None, None, true)
                .unwrap_err();
        assert_eq!(error, MISSING_CAPTURE_ITEM_ERROR);
    }

    #[test]
    fn execution_rejects_unsupported_global_forms() {
        for (raw, needle) in [
            ("@@foo#Ideas\nTask", "not supported"),
            ("@@foo^id\nTask", "not supported"),
            ("@@foo:id\nTask", "not supported"),
            ("@@foo+id#sec\nTask", "not supported"),
        ] {
            let error =
                parse_capture_draft_with_clip_control(raw, None, None, true)
                    .unwrap_err();
            assert!(error.contains(needle), "{raw}: {error}");
        }
    }

    #[test]
    fn execution_accepts_a_later_declaration_only_line() {
        let draft = parse_capture_draft_with_clip_control(
            "First task\n\n@@foo\nSecond",
            None,
            None,
            true,
        )
        .expect("parse");
        assert_eq!(draft.global.as_ref().unwrap().line, 3);
        assert_eq!(draft.items[0].parsed.route.as_deref(), Some("foo"));
        assert_eq!(draft.items[1].parsed.route.as_deref(), Some("foo"));
    }

    #[test]
    fn execution_strips_inline_declarations_before_terminal_markers() {
        let draft = parse_capture_draft_with_clip_control(
            "Buy milk s:2 @@Groceries",
            None,
            None,
            true,
        )
        .expect("parse");
        let global = draft.global.expect("global");
        assert_eq!(global.route, "groceries");
        assert_eq!(global.line, 1);
        assert_eq!(draft.items[0].parsed.body, "Buy milk");
        assert_eq!(draft.items[0].parsed.route.as_deref(), Some("groceries"));
        assert_eq!(draft.items[0].parsed.scheduled_offset, Some(2));
    }

    #[test]
    fn execution_rejects_duplicate_global_declarations_by_line() {
        let error = parse_capture_draft_with_clip_control(
            "@@foo\nBuy milk @@bar",
            None,
            None,
            true,
        )
        .unwrap_err();
        assert!(error.contains("duplicate global destination"), "{error}");
        assert!(error.contains("line 1"), "{error}");
        assert!(error.contains("line 2"), "{error}");
    }

    #[test]
    fn execution_warns_when_a_local_marker_shadows_its_declaration() {
        let draft = parse_capture_draft_with_clip_control(
            "Buy milk @dev @@groceries\n\nOther",
            None,
            None,
            true,
        )
        .expect("parse");
        assert_eq!(draft.items[0].parsed.route.as_deref(), Some("dev"));
        assert_eq!(draft.items[1].parsed.route.as_deref(), Some("groceries"));
        assert_eq!(
            draft.warnings,
            vec![
                "this item's @dev marker overrides the @@groceries destination it declares; move @@groceries to an item without a local marker, or delete @dev"
                    .to_string()
            ]
        );
    }

    #[test]
    fn editor_inherits_global_destination_and_keeps_local_overrides() {
        let parse = editor("@@foo\nFirst\n\nSecond @bar");
        let global = parse.global_destination.as_ref().expect("global");
        assert_eq!(global.route.as_deref(), Some("foo"));
        assert_eq!(parse.route.as_deref(), Some("foo"));
        assert_eq!(parse.items[0].route.as_deref(), Some("foo"));
        assert_eq!(parse.items[1].route.as_deref(), Some("bar"));
        assert!(parse.items[1].has_local_destination);
        assert_eq!(ranges(&parse)[0], (0, 5, SpanKind::GlobalRoute));
    }

    #[test]
    fn editor_reports_incomplete_and_declaration_only_globals() {
        let incomplete = editor("@@");
        assert_eq!(incomplete.mode, EditorMode::Incomplete);
        assert_eq!(incomplete.needs, vec![Need::Route]);
        assert_eq!(codes(&incomplete), vec!["missing_capture_item"]);

        let declaration_only = editor("@@foo");
        assert_eq!(
            declaration_only
                .global_destination
                .as_ref()
                .unwrap()
                .route
                .as_deref(),
            Some("foo")
        );
        assert_eq!(codes(&declaration_only), vec!["missing_capture_item"]);
    }

    #[test]
    fn completion_on_a_global_declaration_excludes_both_sigils_and_plus() {
        let route = field("@@fo", 4).expect("route");
        assert_eq!(route.context, CompletionContext::Route);
        assert_eq!(route.query, "fo");
        assert_eq!(route.replacement, (2, 4));

        let raw = "@@Cash+goog\nnote";
        let plus = raw.find('+').expect("plus");
        let task = field(raw, plus + 2).expect("task");
        assert_eq!(task.context, CompletionContext::Task);
        assert_eq!(task.route.as_deref(), Some("cash"));
        assert_eq!(task.query, "g");
        assert_eq!(task.replacement, (plus + 1, plus + 5));
    }

    #[test]
    fn completion_inside_an_item_stays_item_local_with_a_global_declaration() {
        let raw = "@@foo\nFirst @ba";
        let at = raw.rfind('@').expect("local at");
        let completion = field(raw, raw.len()).expect("local route");
        assert_eq!(completion.context, CompletionContext::Route);
        assert_eq!(completion.replacement, (at + 1, raw.len()));
        assert_eq!(field(raw, 3).unwrap().context, CompletionContext::Route);
        assert_eq!(field(raw, 3).unwrap().replacement, (2, 5));
    }

    #[test]
    fn editor_item_at_uses_the_inherited_global_route() {
        let raw = "@@sase\nSee [[#De";
        let item = editor_item_at(raw, raw.len()).expect("item");
        assert_eq!(item.route.as_deref(), Some("sase"));
        assert!(!item.has_local_destination);
    }

    // -----------------------------------------------------------------------
    // `rewrite_draft` (Rules A1-A6)
    // -----------------------------------------------------------------------

    #[test]
    fn rewrite_draft_absorbs_a_trailing_local_marker() {
        let raw = "Buy milk @dev @@";
        let rewrite = rewrite_draft(raw, Some(raw.len()));
        assert_eq!(rewrite.rule, Some(RewriteRule::AbsorbLocalMarker));
        assert_eq!(rewrite.text, "Buy milk @@dev");
        assert_eq!(rewrite.cursor, Some(14));
        assert_eq!(rewrite.summary.as_deref(), Some("Moved @dev into @@dev"));
        assert_eq!(
            rewrite.edits,
            vec![
                TextEdit {
                    start: 9,
                    end: 14,
                    replacement: String::new(),
                },
                TextEdit {
                    start: 14,
                    end: 16,
                    replacement: "@@dev".to_string(),
                },
            ]
        );
    }

    #[test]
    fn rewrite_draft_absorbs_a_leading_local_marker() {
        let raw = "@dev Buy milk @@";
        let rewrite = rewrite_draft(raw, None);
        assert_eq!(rewrite.rule, Some(RewriteRule::AbsorbLocalMarker));
        assert_eq!(rewrite.text, "Buy milk @@dev");
    }

    #[test]
    fn rewrite_draft_absorbs_a_parent_lines_marker_from_a_child_lines_bare_at_at(
    ) {
        let raw = "Buy milk @dev\n- more detail @@";
        let rewrite = rewrite_draft(raw, None);
        assert_eq!(rewrite.rule, Some(RewriteRule::AbsorbLocalMarker));
        assert_eq!(rewrite.text, "Buy milk\n- more detail @@dev");
    }

    #[test]
    fn rewrite_draft_absorbs_a_sub_bullet_local_marker() {
        let raw = "Buy stock @cash+goog-exit @@";
        let rewrite = rewrite_draft(raw, None);
        assert_eq!(rewrite.rule, Some(RewriteRule::AbsorbLocalMarker));
        assert_eq!(rewrite.text, "Buy stock @@cash+goog-exit");
    }

    #[test]
    fn rewrite_draft_absorbs_a_declaration_only_line_into_a_later_items_bare_at_at(
    ) {
        let raw = "@@foo\nBuy milk @@";
        let rewrite = rewrite_draft(raw, None);
        assert_eq!(rewrite.rule, Some(RewriteRule::AbsorbDeclaration));
        assert_eq!(rewrite.text, "Buy milk @@foo");
        assert_eq!(
            rewrite.summary.as_deref(),
            Some("Moved the @@foo declaration here")
        );
    }

    #[test]
    fn rewrite_draft_reports_rule_a5_notices_for_non_absorbable_markers() {
        for (raw, needle) in [
            ("note @notes#Ideas @@", "cannot take a section"),
            ("note @dev^id @@", "cannot take a block ID"),
            ("note @dev:id @@", "cannot take a Pomodoro link"),
            ("note this # @@", "cannot take a Pomodoro note"),
        ] {
            let rewrite = rewrite_draft(raw, None);
            assert_eq!(rewrite.rule, None, "{raw}");
            assert_eq!(rewrite.text, raw, "{raw}");
            assert_eq!(rewrite.notices.len(), 1, "{raw}");
            assert!(
                rewrite.notices[0].contains(needle),
                "{raw}: {}",
                rewrite.notices[0]
            );
        }
    }

    #[test]
    fn rewrite_draft_declines_when_the_item_has_two_local_markers() {
        let raw = "Buy milk @dev @@\n- child @notes#Ideas";
        let rewrite = rewrite_draft(raw, None);
        assert_eq!(rewrite.rule, None);
        assert_eq!(rewrite.text, raw);
        assert!(rewrite.notices.is_empty());
    }

    #[test]
    fn rewrite_draft_is_a_no_op_without_a_bare_at_at() {
        let raw = "Buy milk @dev";
        let rewrite = rewrite_draft(raw, None);
        assert_eq!(rewrite.rule, None);
        assert_eq!(rewrite.text, raw);
        assert_eq!(rewrite.cursor, None);
        assert!(rewrite.edits.is_empty());
    }

    #[test]
    fn rewrite_draft_selects_the_bare_at_at_under_the_cursor_else_the_last() {
        let raw = "Buy milk @dev @@ @@";
        let claimed_start = |rewrite: &DraftRewrite| {
            rewrite
                .edits
                .iter()
                .find(|edit| edit.replacement == "@@dev")
                .map(|edit| edit.start)
                .expect("replace edit")
        };

        assert_eq!(claimed_start(&rewrite_draft(raw, Some(15))), 14);
        assert_eq!(claimed_start(&rewrite_draft(raw, Some(18))), 17);
        assert_eq!(claimed_start(&rewrite_draft(raw, None)), 17);
    }

    #[test]
    fn rewrite_draft_is_idempotent() {
        let raw = "Buy milk @dev @@";
        let first = rewrite_draft(raw, Some(raw.len()));
        assert_eq!(first.rule, Some(RewriteRule::AbsorbLocalMarker));

        let second = rewrite_draft(&first.text, first.cursor);
        assert_eq!(second.rule, None);
        assert_eq!(second.text, first.text);
    }

    #[test]
    fn rewrite_draft_avoids_double_spaces_and_the_result_parses_cleanly() {
        let raw = "@dev note @@ more text";
        let rewrite = rewrite_draft(raw, None);
        assert_eq!(rewrite.rule, Some(RewriteRule::AbsorbLocalMarker));
        assert_eq!(rewrite.text, "note @@dev more text");
        assert!(!rewrite.text.contains("  "), "{}", rewrite.text);

        let parsed = parse_for_editor(&rewrite.text);
        assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    }

    #[test]
    fn rewrite_draft_keeps_offsets_on_char_boundaries_with_multibyte_input() {
        let raw = "caf\u{e9} \u{1f680} @dev @@";
        let rewrite = rewrite_draft(raw, Some(raw.len()));
        assert_eq!(rewrite.rule, Some(RewriteRule::AbsorbLocalMarker));
        for edit in &rewrite.edits {
            assert!(raw.is_char_boundary(edit.start));
            assert!(raw.is_char_boundary(edit.end));
        }
        assert_eq!(rewrite.text, "caf\u{e9} \u{1f680} @@dev");
    }
}
