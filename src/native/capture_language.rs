//! The capture grammar shared by `bob capture` and `bob capture-parse`.
//!
//! This module owns every position-agnostic classification rule for capture
//! text: whitespace normalization, terminal marker extraction, and `@token`
//! routing. `capture.rs` layers execution (files, clipboard, note mutation)
//! on top of it, and `capture_parse.rs` layers a span-aware, read-only
//! editor view on the same functions. There is exactly one grammar here; the
//! editor path never re-implements token classification.
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
    Bullet {
        section_prefix: Option<String>,
        exact: bool,
    },
    Pomodoro {
        block_id: String,
    },
    SubBullet {
        target: SubBulletTarget,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SubBulletTarget {
    BlockId(String),
    Ref { line: usize, digest: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ParsedCaptureText {
    pub(crate) body: String,
    pub(crate) clip: Option<ClipRequest>,
    pub(crate) route: Option<String>,
    pub(crate) kind: CaptureKind,
    pub(crate) scheduled_offset: Option<u64>,
    pub(crate) priority_level: Option<u64>,
    /// Normalized authored-child bodies, in source order, with their `-`,
    /// `*`, or `+` source marker and capture-wide markers already removed.
    /// Empty when the draft was an ordinary single-line capture.
    pub(crate) sub_bullets: Vec<String>,
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
    route: String,
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

/// Recognize one physical continuation line as a flat authored bullet: it
/// must start at byte 0 with `-`, `*`, or `+`, immediately followed by at
/// least one space or tab. Returns the text after the marker and its
/// contiguous run of separating whitespace, which is the item's raw body
/// before whitespace normalization. Returns `None` for every other shape --
/// indentation, nesting, a different leading character, or ordinary
/// continuation prose -- so the caller can report one consistent diagnostic
/// for all of them.
pub(crate) fn strip_bullet_marker(line_text: &str) -> Option<&str> {
    let mut chars = line_text.char_indices();
    let (_, first) = chars.next()?;
    if !matches!(first, '-' | '*' | '+') {
        return None;
    }
    let (next_index, next_char) = chars.next()?;
    if next_char != ' ' && next_char != '\t' {
        return None;
    }

    let bytes = line_text.as_bytes();
    let mut end = next_index + next_char.len_utf8();
    while end < bytes.len() && matches!(bytes[end], b' ' | b'\t') {
        end += 1;
    }
    Some(&line_text[end..])
}

pub(crate) fn parse_capture_text_with_clip_control(
    raw_text: &str,
    forced_route: Option<&str>,
    forced_section: Option<&str>,
    parse_clip_markers: bool,
) -> Result<ParsedCaptureText, String> {
    let lines = split_physical_lines(raw_text);
    let Some((parent_line, child_lines)) = lines.split_first() else {
        return Err(missing_text_error());
    };
    let detect_route = forced_route.is_none();

    let parent_normalized = normalize_task_text(parent_line.text);
    if parent_normalized.is_empty() {
        return Err(missing_text_error());
    }
    let parent_tokens: Vec<&str> = parent_normalized.split(' ').collect();
    let parent_outcome =
        resolve_line(parent_tokens, true, detect_route, parse_clip_markers)?;
    if parent_outcome.body.is_empty() {
        return Err(missing_text_error());
    }

    let mut aggregate = AggregateMarkers::default();
    aggregate.absorb(parent_outcome.markers, parent_outcome.route)?;

    let mut sub_bullets = Vec::new();
    for (index, line) in child_lines.iter().enumerate() {
        let line_number = index + 2;
        if line.text.trim().is_empty() {
            continue;
        }
        let Some(remainder) = strip_bullet_marker(line.text) else {
            return Err(invalid_child_line_error(line_number));
        };
        if remainder.trim().is_empty() {
            continue;
        }
        let normalized = normalize_task_text(remainder);
        let tokens: Vec<&str> = normalized.split(' ').collect();
        let outcome =
            resolve_line(tokens, false, detect_route, parse_clip_markers)?;
        if outcome.body.is_empty() {
            return Err(empty_child_after_markers_error(line_number));
        }
        aggregate.absorb(outcome.markers, outcome.route)?;
        sub_bullets.push(outcome.body);
    }

    if let Some(section) = forced_section {
        let Some(route) = forced_route else {
            return Err("--section requires --route".to_string());
        };
        if section.trim().is_empty() {
            return Err("--section must not be empty".to_string());
        }
        let route = normalize_forced_route(route)?;
        return Ok(ParsedCaptureText {
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
        });
    }

    if let Some(route) = forced_route {
        let route = normalize_forced_route(route)?;
        return Ok(ParsedCaptureText {
            body: parent_outcome.body,
            clip: aggregate.clip,
            route: Some(route),
            kind: CaptureKind::Task,
            scheduled_offset: aggregate.scheduled_offset,
            priority_level: aggregate.priority_level,
            sub_bullets,
        });
    }

    let (route, kind) = match aggregate.route {
        Some(token) => (Some(token.route), token.kind),
        None => (None, CaptureKind::Task),
    };
    Ok(ParsedCaptureText {
        body: parent_outcome.body,
        clip: aggregate.clip,
        route,
        kind,
        scheduled_offset: aggregate.scheduled_offset,
        priority_level: aggregate.priority_level,
        sub_bullets,
    })
}

/// One physical line's resolved capture-wide markers and (when a route was
/// recognized on this line) its route/mode token. `body` is the line's
/// remaining text after every recognized marker is removed; it is empty
/// exactly when the line held no non-marker tokens.
struct LineOutcome {
    body: String,
    markers: TerminalMarkers,
    route: Option<RouteToken>,
}

/// Resolve one physical line's whitespace tokens exactly like the original
/// single-line grammar resolved the whole draft. `leading` allows a
/// first-token route to win and is only ever set for the parent line, which
/// is the only line that preserves the established leading-route form.
/// `detect_route` is false whenever `--route`/`--section` already fixed the
/// route, in which case every `@...`-shaped token stays literal on every
/// line, exactly like the single-line forced-route path did.
fn resolve_line(
    mut tokens: Vec<&str>,
    leading: bool,
    detect_route: bool,
    parse_clip_markers: bool,
) -> Result<LineOutcome, String> {
    let (markers, _) =
        extract_terminal_markers(&mut tokens, parse_clip_markers);
    if tokens.is_empty() {
        return Ok(LineOutcome {
            body: String::new(),
            markers,
            route: None,
        });
    }

    reject_legacy_bullet_markers(&tokens, detect_route)?;

    if !detect_route {
        return Ok(LineOutcome {
            body: tokens.join(" "),
            markers,
            route: None,
        });
    }

    // Leading route wins: when the first token is a route token followed by
    // body text, route by it and do not inspect later route-looking tokens.
    if leading && let Some(token) = parse_terminal_route_token(tokens[0])? {
        let rest = &tokens[1..];
        if rest.is_empty() {
            if !matches!(token.kind, CaptureKind::Task) {
                return Err(missing_text_error());
            }
            // A bare `@foo` with no body stays literal task text.
        } else {
            return Ok(LineOutcome {
                body: rest.join(" "),
                markers,
                route: Some(token),
            });
        }
    }

    validate_special_terminal_markers_line(&tokens, leading)?;

    // Otherwise a trailing route token routes the body that precedes it.
    if let Some((&last, rest)) = tokens.split_last()
        && !rest.is_empty()
        && let Some(token) = parse_terminal_route_token(last)?
    {
        return Ok(LineOutcome {
            body: rest.join(" "),
            markers,
            route: Some(token),
        });
    }

    Ok(LineOutcome {
        body: tokens.join(" "),
        markers,
        route: None,
    })
}

/// Accumulate the four capture-wide marker slots (route/mode, schedule,
/// priority, clipboard) across every physical line. Each slot may be set by
/// at most one line; a second line that resolves the same slot is ambiguous.
#[derive(Default)]
struct AggregateMarkers {
    clip: Option<ClipRequest>,
    scheduled_offset: Option<u64>,
    priority_level: Option<u64>,
    route: Option<RouteToken>,
}

impl AggregateMarkers {
    fn absorb(
        &mut self,
        markers: TerminalMarkers,
        route: Option<RouteToken>,
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
                    "route/mode marker (@route)",
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
        "capture line {line_number} must be a flat bullet starting with \
\"- \", \"* \", or \"+ \" at the start of the line, or left blank"
    )
}

fn empty_child_after_markers_error(line_number: usize) -> String {
    format!(
        "capture line {line_number} has no text left after its capture \
markers were removed"
    )
}

pub(crate) fn missing_text_error() -> String {
    "task text is required; pass TEXT or pipe it on stdin".to_string()
}

fn legacy_marker_error() -> String {
    "bullet section markers must be appended to an @route token; use \
@foo#bar instead of #bar @foo"
        .to_string()
}

pub(crate) fn normalize_task_text(raw_text: &str) -> String {
    raw_text.split_whitespace().collect::<Vec<_>>().join(" ")
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
        route: route_part.to_ascii_lowercase(),
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
    if is_pomodoro_marker_candidate(token) {
        return parse_pomodoro_route_token(token).map(Some);
    }
    Ok(parse_route_token(token))
}

fn parse_sub_bullet_route_token(token: &str) -> Result<RouteToken, String> {
    let marker = token.strip_prefix('@').ok_or_else(|| {
        "sub-bullet capture markers must use @<route>^<block-id>".to_string()
    })?;
    let Some((route, block_id)) = marker.split_once('^') else {
        return Err("sub-bullet capture markers must use @<route>^<block-id>"
            .to_string());
    };
    if route.is_empty() {
        return Err("sub-bullet capture markers must use @<route>^<block-id>"
            .to_string());
    }
    if block_id.is_empty() {
        return Err(format!(
            "sub-bullet capture requires a block ID: @<route>^<block-id> (run 'bob capture-tasks -r {}' to list task block IDs)",
            route.to_ascii_lowercase()
        ));
    }
    if !is_route_token(route) {
        return Err(SUB_BULLET_ROUTE_ERROR.to_string());
    }
    if !is_block_id(block_id) {
        return Err(SUB_BULLET_BLOCK_ID_ERROR.to_string());
    }

    Ok(RouteToken {
        route: route.to_ascii_lowercase(),
        kind: CaptureKind::SubBullet {
            target: SubBulletTarget::BlockId(block_id.to_string()),
        },
    })
}

fn is_sub_bullet_marker_candidate(token: &str) -> bool {
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
        .find([':', '#'])
        .is_none_or(|separator| caret < separator)
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
        route: route.to_ascii_lowercase(),
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
    colon.is_some_and(|colon| {
        hash.is_none_or(|hash| colon < hash)
            && caret.is_none_or(|caret| colon < caret)
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
    parse_route_token(token).is_some()
        || (is_sub_bullet_marker_candidate(token)
            && parse_sub_bullet_route_token(token).is_ok())
        || (is_pomodoro_marker_candidate(token)
            && parse_pomodoro_route_token(token).is_ok())
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

    if last.starts_with('#') {
        return Err(legacy_marker_error());
    }

    if allow_route
        && tokens.len() >= 2
        && tokens[tokens.len() - 2].starts_with('#')
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

const SUB_BULLET_ROUTE_ERROR: &str =
    "sub-bullet capture route must contain only A-Z, a-z, 0-9, '_' or '-'";
const SUB_BULLET_BLOCK_ID_ERROR: &str =
    "sub-bullet capture block ID must be non-empty and contain only A-Z, a-z, 0-9 or '-'";
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
    PomodoroRoute,
    PomodoroBlockId,
    SubBulletRoute,
    SubBulletBlockId,
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
            Self::PomodoroRoute => "pomodoro_route",
            Self::PomodoroBlockId => "pomodoro_block_id",
            Self::SubBulletRoute => "sub_bullet_route",
            Self::SubBulletBlockId => "sub_bullet_block_id",
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
    SubBullet,
    Incomplete,
}

impl EditorMode {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Task => "task",
            Self::Bullet => "bullet",
            Self::PomodoroTask => "pomodoro_task",
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
    PomodoroId,
    Task,
}

impl Need {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Route => "route",
            Self::Section => "section",
            Self::PomodoroId => "pomodoro_id",
            Self::Task => "task",
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
    /// Normalized authored-child bodies of every other valid, nonempty
    /// physical line, in source order. A malformed or empty-after-markers
    /// child line is reported as a diagnostic instead and excluded here.
    pub(crate) sub_bullets: Vec<String>,
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
struct LineEditorParse {
    body: String,
    marker: Option<MarkerParse>,
    terminal_spans: Vec<Span>,
    diagnostics: Vec<Diagnostic>,
}

/// Resolve one line's already offset-tagged tokens exactly like
/// `parse_for_editor` resolved its single line before this module became
/// line-aware. `leading` allows a first-token route to win and must only be
/// set for the parent line.
fn parse_editor_line(
    mut tokens: Vec<Token<'_>>,
    leading: bool,
) -> LineEditorParse {
    let (_, marker_spans) = extract_terminal_markers(&mut tokens, true);
    let terminal_spans: Vec<Span> = marker_spans
        .into_iter()
        .map(|(kind, start, end)| Span { start, end, kind })
        .collect();

    let mut diagnostics = Vec::new();
    if let Some(diagnostic) = legacy_bullet_marker_diagnostic(&tokens) {
        diagnostics.push(diagnostic);
    }

    // The recognized `@...` token leaves the body exactly like execution
    // drops it before joining the remaining tokens with single spaces.
    let selected = select_marker_token(&tokens, leading);
    let marker_index = selected.as_ref().map(|(index, _)| *index);
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
        terminal_spans,
        diagnostics,
    }
}

/// Track which capture-wide marker slots earlier lines already resolved, so
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
                duplicate_marker_error("route/mode marker (@route)"),
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
/// `@^`, `@route^`, and their legacy `@!` aliases) is a valid editing state,
/// and an invalid marker component -- or line shape -- becomes a diagnostic
/// instead of an error. Tokenization, terminal marker extraction, and
/// marker classification all run through the same functions `bob capture`
/// executes with; `mode`/`route`/`section`/`block_id`/`needs` describe
/// whichever line resolved a marker first, exactly like `bob capture`
/// prefers the first line's leading form and later lines only compose
/// trailing markers, while `sub_bullets` reports every other authored
/// child's normalized body in source order.
pub(crate) fn parse_for_editor(raw_text: &str) -> EditorParse {
    let lines = split_physical_lines(raw_text);
    let synthetic_empty = RawLine {
        text: "",
        start: 0,
        end: 0,
    };
    let parent_line = lines.first().copied().unwrap_or(synthetic_empty);
    let child_lines: &[RawLine] =
        if lines.len() > 1 { &lines[1..] } else { &[] };

    let mut spans: Vec<Span> = Vec::new();
    let mut diagnostics: Vec<Diagnostic> = Vec::new();
    let mut seen = SeenMarkers::default();

    let parent_tokens = tokenize_line_with_spans(&parent_line);
    let parent_parse = parse_editor_line(parent_tokens, true);
    seen.absorb_terminal_spans(&parent_parse.terminal_spans, &mut diagnostics);
    spans.extend(parent_parse.terminal_spans);
    diagnostics.extend(parent_parse.diagnostics);

    let body = parent_parse.body;
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
    for (index, line) in child_lines.iter().enumerate() {
        let line_number = index + 2;
        if line.text.trim().is_empty() {
            continue;
        }
        let Some(remainder) = strip_bullet_marker(line.text) else {
            diagnostics.push(Diagnostic {
                severity: Severity::Error,
                code: "invalid_child_line",
                message: invalid_child_line_error(line_number),
                range: Some((line.start, line.end)),
            });
            continue;
        };
        if remainder.trim().is_empty() {
            continue;
        }

        let remainder_start = line.start + (line.text.len() - remainder.len());
        let child_line = RawLine {
            text: remainder,
            start: remainder_start,
            end: line.end,
        };
        let child_tokens = tokenize_line_with_spans(&child_line);
        let child_parse = parse_editor_line(child_tokens, false);

        seen.absorb_terminal_spans(
            &child_parse.terminal_spans,
            &mut diagnostics,
        );
        spans.extend(child_parse.terminal_spans);
        diagnostics.extend(child_parse.diagnostics);

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
                range: Some((line.start, line.end)),
            });
        } else {
            sub_bullets.push(child_parse.body);
        }
    }

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
    }
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
        if let Some(parse) = classify_editor_token(&tokens[last]) {
            return Some((last, parse));
        }
    }

    None
}

/// Classify one `@...` token, returning `None` when the token is not
/// route-shaped at all and therefore stays literal body text.
fn classify_editor_token(token: &Token<'_>) -> Option<TokenParse> {
    let text = token.text;
    if !text.starts_with('@') {
        return None;
    }
    if is_sub_bullet_marker_candidate(text) {
        return Some(classify_sub_bullet_token(token));
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
    let (route_part, block_part) =
        marker.split_once('^').expect("sub-bullet candidate");

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

    TokenParse::Marker(marker_parse(
        token,
        MarkerShape {
            sigil_len: 1,
            route_part,
            separator: true,
            right_part: block_part,
            route_kind: SpanKind::SubBulletRoute,
            right_kind: SpanKind::SubBulletBlockId,
            complete_mode: EditorMode::SubBullet,
            right_need: Need::Task,
        },
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
            separator,
            right_part: block_part,
            route_kind: SpanKind::PomodoroRoute,
            right_kind: SpanKind::PomodoroBlockId,
            complete_mode: EditorMode::PomodoroTask,
            right_need: Need::PomodoroId,
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
            separator: true,
            right_part: prefix,
            route_kind: SpanKind::Route,
            right_kind: SpanKind::Section,
            complete_mode: EditorMode::Bullet,
            right_need: Need::Section,
        },
    ))
}

/// The shared shape of every `@<route><separator><right>` marker.
struct MarkerShape<'a> {
    sigil_len: usize,
    route_part: &'a str,
    separator: bool,
    right_part: &'a str,
    route_kind: SpanKind,
    right_kind: SpanKind,
    complete_mode: EditorMode,
    right_need: Need,
}

/// Build the mode, needs, and spans for one marker from its component parts.
///
/// Spans never overlap and always sit on `char` boundaries. When a component
/// is still empty its sigil and separator become one
/// `interactive_placeholder` span so an editor can highlight the caret
/// position the user still has to fill in.
fn marker_parse(token: &Token<'_>, shape: MarkerShape<'_>) -> MarkerParse {
    let route_end = token.start + shape.sigil_len + shape.route_part.len();
    let right_start = token.end - shape.right_part.len();
    let has_route = !shape.route_part.is_empty();
    let has_right = !shape.right_part.is_empty();

    let mut spans = Vec::new();
    if has_route {
        spans.push(Span {
            start: token.start,
            end: route_end,
            kind: shape.route_kind,
        });
        if !has_right && shape.separator {
            spans.push(Span {
                start: route_end,
                end: route_end + 1,
                kind: SpanKind::InteractivePlaceholder,
            });
        }
    } else {
        let placeholder_end = if shape.separator {
            route_end + 1
        } else {
            route_end
        };
        spans.push(Span {
            start: token.start,
            end: placeholder_end,
            kind: SpanKind::InteractivePlaceholder,
        });
    }
    if has_right {
        spans.push(Span {
            start: right_start,
            end: token.end,
            kind: shape.right_kind,
        });
    }

    // A `@route#` bullet is executable today (it means "any non-Tasks
    // section"), so it keeps its complete mode while still reporting the
    // section it could still resolve. A section can only be offered once the
    // route that owns its headings is known.
    let section_is_optional = shape.right_need == Need::Section;

    let mut needs = Vec::new();
    if !has_route {
        needs.push(Need::Route);
    }
    if !has_right && (has_route || !section_is_optional) {
        needs.push(shape.right_need);
    }

    let mode = if has_route && (has_right || section_is_optional) {
        shape.complete_mode
    } else {
        EditorMode::Incomplete
    };

    MarkerParse {
        mode,
        route: has_route.then(|| shape.route_part.to_ascii_lowercase()),
        section: (shape.right_need == Need::Section && has_right)
            .then(|| shape.right_part.to_string()),
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
    /// (`section`, `pomodoro_block_id`, and `task`).
    pub(crate) route: Option<String>,
    /// The text already typed in this component, up to `cursor`.
    pub(crate) query: String,
    /// The half-open UTF-8 byte range of the whole component, which a
    /// completion replaces in full regardless of where the cursor sits
    /// inside it.
    pub(crate) replacement: (usize, usize),
}

/// Identify the completable marker component at `cursor`, reusing the same
/// tokenizer, terminal-marker extraction, and `@token` candidate detection
/// as [`parse_for_editor`]. Returns `None` when the cursor is not inside an
/// eligible leading or trailing `@` marker: plain body text, a token in the
/// middle of the input, and an unrecognized or invalid marker never produce
/// a completion field.
///
/// Multi-line drafts complete the physical line the cursor is on: only the
/// first (parent) line offers a leading marker, and a child line's bullet
/// marker (`- `, `* `, or `+ `) itself is never completable, matching the
/// authored-bullet grammar `bob capture` and `bob capture-parse` execute
/// with.
pub(crate) fn completion_field_at(
    raw_text: &str,
    cursor: usize,
) -> Option<CompletionField> {
    let lines = split_physical_lines(raw_text);
    let (line_index, line) = lines
        .iter()
        .enumerate()
        .find(|(_, line)| cursor >= line.start && cursor <= line.end)?;
    let leading = line_index == 0;

    let scan_line = if leading {
        *line
    } else {
        let remainder = strip_bullet_marker(line.text)?;
        let remainder_start = line.start + (line.text.len() - remainder.len());
        if cursor < remainder_start {
            return None;
        }
        RawLine {
            text: remainder,
            start: remainder_start,
            end: line.end,
        }
    };

    let mut tokens = tokenize_line_with_spans(&scan_line);
    extract_terminal_markers(&mut tokens, true);

    let index = completion_marker_index(&tokens, leading)?;
    let token = tokens[index];
    if cursor < token.start || cursor > token.end {
        return None;
    }

    marker_field_at_cursor(&token, cursor)
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
        let (route_part, block_part) =
            marker.split_once('^').expect("sub-bullet candidate");
        return completion_field_from_parts(
            token,
            1,
            route_part,
            true,
            block_part,
            CompletionContext::Task,
            cursor,
        );
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
            sigil_len,
            route_part,
            separator,
            block_part,
            CompletionContext::PomodoroBlockId,
            cursor,
        );
    }

    let rest = text.strip_prefix('@')?;
    if let Some((route_part, prefix)) = rest.split_once('#') {
        return completion_field_from_parts(
            token,
            1,
            route_part,
            true,
            prefix,
            CompletionContext::Section,
            cursor,
        );
    }

    // A bare `@` or a still-typing `@fragment` with no separator yet: the
    // whole remainder is the route component, and there is no right-hand
    // component to fall into.
    completion_field_from_parts(
        token,
        1,
        rest,
        false,
        "",
        CompletionContext::Route,
        cursor,
    )
}

/// Build the completion field for one decomposed `@<route><sep><right>`
/// marker, given which side of the (possible) separator `cursor` lands on.
/// The route component spans exactly the route text, excluding the leading
/// sigil; the right component spans exactly its text, excluding the
/// one-byte separator. Both stay well-defined -- and empty -- when their
/// text has not been typed yet, so a bare `@` or a fresh `@route:` still
/// reports a real, zero-length replacement range at the insertion point.
fn completion_field_from_parts(
    token: &Token<'_>,
    sigil_len: usize,
    route_part: &str,
    separator: bool,
    right_part: &str,
    right_context: CompletionContext,
    cursor: usize,
) -> Option<CompletionField> {
    let route_start = token.start + sigil_len;
    let route_end = route_start + route_part.len();

    if !separator || cursor <= route_end {
        let split = cursor.clamp(route_start, route_end) - route_start;
        return Some(CompletionField {
            context: CompletionContext::Route,
            route: None,
            query: route_part[..split].to_string(),
            replacement: (route_start, route_end),
        });
    }

    // Past the one-byte separator: complete the right-hand component. It
    // only makes sense once the route it belongs to already resolves.
    if !is_route_token(route_part) {
        return None;
    }
    let right_start = route_end + 1;
    let right_end = right_start + right_part.len();
    let split = cursor.clamp(right_start, right_end) - right_start;
    Some(CompletionField {
        context: right_context,
        route: Some(route_part.to_ascii_lowercase()),
        query: right_part[..split].to_string(),
        replacement: (right_start, right_end),
    })
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
        let raw = "caf\u{e9} run \u{1f680} @Cash^goog-exit";
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
        let raw = "Call bank @Cash^";
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
        assert_eq!(&raw[15..16], "^");
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
                "Body @dev^focus-123",
                EditorMode::SubBullet,
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
                &[Need::Task],
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
                &[Need::Route, Need::Task],
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
                "Body @dev^focus-123",
                &[SpanKind::SubBulletRoute, SpanKind::SubBulletBlockId],
            ),
            (
                "Body @dev^",
                &[SpanKind::SubBulletRoute, SpanKind::InteractivePlaceholder],
            ),
            (
                "Body @^focus-123",
                &[SpanKind::InteractivePlaceholder, SpanKind::SubBulletBlockId],
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
                "Body @bad.route^id",
                "invalid_sub_bullet_route",
                SUB_BULLET_ROUTE_ERROR,
            ),
            (
                "Body @dev^bad.id",
                "invalid_sub_bullet_block_id",
                SUB_BULLET_BLOCK_ID_ERROR,
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
            "Discuss @dev^id later",
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
            ("@dev^id", EditorMode::SubBullet),
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
    /// and friends) are the documented exception: execution keeps them
    /// literal, and this module reports them as `incomplete` instead.
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
            "Do thing @Dev:Foo-Bar",
            "@Dev:Foo-Bar Do thing s:2",
            "Do thing @!Dev:Foo-Bar s:2",
            "Called today @Cash^Goog-Exit",
            "Called today %log @Cash^Goog-Exit",
            "Called today @Cash^Goog-Exit s:1",
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
        ];

        for raw in inputs {
            let executed =
                parse_capture_text_with_clip_control(raw, None, None, true)
                    .unwrap_or_else(|error| panic!("{raw}: {error}"));
            let parse = editor(raw);
            assert_eq!(parse.body, executed.body, "{raw}");
            assert_eq!(parse.route, executed.route, "{raw}");
            let expected_mode = match &executed.kind {
                CaptureKind::Task => EditorMode::Task,
                CaptureKind::Bullet { .. } => EditorMode::Bullet,
                CaptureKind::Pomodoro { .. } => EditorMode::PomodoroTask,
                CaptureKind::SubBullet { .. } => EditorMode::SubBullet,
            };
            assert_eq!(parse.mode, expected_mode, "{raw}");
            if let CaptureKind::Pomodoro { block_id } = &executed.kind {
                assert_eq!(parse.block_id.as_deref(), Some(block_id.as_str()));
            }
            if let CaptureKind::SubBullet {
                target: SubBulletTarget::BlockId(block_id),
            } = &executed.kind
            {
                assert_eq!(parse.block_id.as_deref(), Some(block_id.as_str()));
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
        let complete = editor("Add context @Dev^focus-123");
        assert_eq!(complete.mode, EditorMode::SubBullet);
        assert_eq!(complete.body, "Add context");
        assert_eq!(complete.route.as_deref(), Some("dev"));
        assert_eq!(complete.block_id.as_deref(), Some("focus-123"));
        assert!(complete.needs.is_empty());

        let needs_task = editor("Add context @Dev^");
        assert_eq!(needs_task.route.as_deref(), Some("dev"));
        assert_eq!(needs_task.needs, vec![Need::Task]);

        let needs_target = editor("Add context @^focus-123");
        assert_eq!(needs_target.block_id.as_deref(), Some("focus-123"));
        assert_eq!(needs_target.needs, vec![Need::Route]);

        let needs_both = editor("Add context @^");
        assert_eq!(needs_both.needs, vec![Need::Route, Need::Task]);
    }

    #[test]
    fn lua_gives_sub_bullet_markers_precedence_over_pomodoro_markers() {
        let malformed = editor("Add context @route^bad:id");
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
            ("Add context @bad.route^id", "invalid_sub_bullet_route"),
            ("Add context @route^bad.id", "invalid_sub_bullet_block_id"),
            ("Add context @route^bad_id", "invalid_sub_bullet_block_id"),
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

        let parse = editor("Discuss @dev^id later");
        assert_eq!(parse.mode, EditorMode::Task);
        assert_eq!(parse.body, "Discuss @dev^id later");

        for raw in ["@dev:id", "@:", "@dev^id", "@^"] {
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
                "@Dev^focus-123",
                EditorMode::SubBullet,
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
                &[Need::Task],
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
                &[Need::Route, Need::Task],
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
        let parse = editor("Call bank @Cash^");
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
        let parse = editor("Body @dev^bad.id");
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
    fn missing_route_portion_of_sub_bullet_marker_completes_a_route() {
        let completion = field("@^focus-123", 1).expect("route field");
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
    fn legacy_pomodoro_alias_completes_the_same_as_the_canonical_form() {
        let completion = field("Do work @!Dev:foc", 17).expect("pomodoro id");
        assert_eq!(completion.context, CompletionContext::PomodoroBlockId);
        assert_eq!(completion.route.as_deref(), Some("dev"));
        assert_eq!(completion.query, "foc");
        assert_eq!(completion.replacement, (14, 17));
    }

    #[test]
    fn task_completes_after_a_resolved_sub_bullet_route() {
        let completion = field("note @Cash^goog", 15).expect("task field");
        assert_eq!(completion.context, CompletionContext::Task);
        assert_eq!(completion.route.as_deref(), Some("cash"));
        assert_eq!(completion.query, "goog");
        assert_eq!(completion.replacement, (11, 15));
    }

    #[test]
    fn right_component_without_a_resolved_route_has_no_completion() {
        assert_eq!(field("@:foc", 5), None);
        assert_eq!(field("@^foc", 5), None);
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
    fn invalid_block_id_characters_still_produce_a_field() {
        // The field extractor never validates block-ID syntax; a discovery
        // scan naturally returns no candidates for a query no real block ID
        // could match, without a separate invalid/error path here.
        let completion = field("note @dev^bad.id", 16).expect("task field");
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
        let raw = "caf\u{e9} \u{1f680} @Cash^goog-exit";
        for cursor in
            (0..=raw.len()).filter(|&index| raw.is_char_boundary(index))
        {
            let _ = field(raw, cursor);
        }
    }

    #[test]
    fn completion_field_uses_byte_offsets_after_multibyte_prefix_text() {
        let raw = "caf\u{e9} \u{1f680} @Cash^goog-exit";
        // "@Cash^goog-exit" starts at byte 11, right after the multibyte
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
    fn strip_bullet_marker_accepts_dash_star_plus_with_space_or_tab() {
        assert_eq!(strip_bullet_marker("- body"), Some("body"));
        assert_eq!(strip_bullet_marker("* body"), Some("body"));
        assert_eq!(strip_bullet_marker("+ body"), Some("body"));
        assert_eq!(strip_bullet_marker("-\tbody"), Some("body"));
        assert_eq!(strip_bullet_marker("-   body"), Some("body"));
        assert_eq!(strip_bullet_marker("- "), Some(""));
        assert_eq!(strip_bullet_marker("-\t"), Some(""));
    }

    #[test]
    fn strip_bullet_marker_rejects_every_other_shape() {
        assert_eq!(strip_bullet_marker("body"), None);
        assert_eq!(strip_bullet_marker("  - indented"), None);
        assert_eq!(strip_bullet_marker("-body"), None);
        assert_eq!(strip_bullet_marker("-"), None);
        assert_eq!(strip_bullet_marker("#body"), None);
        assert_eq!(strip_bullet_marker(""), None);
    }

    // -----------------------------------------------------------------
    // Line-aware capture: execution grammar.
    // -----------------------------------------------------------------

    fn execute(raw: &str) -> Result<ParsedCaptureText, String> {
        parse_capture_text_with_clip_control(raw, None, None, true)
    }

    #[test]
    fn execution_renders_authored_children_in_source_order() {
        let parsed =
            execute("@work parent line\n- first child\n- second child")
                .expect("parse");
        assert_eq!(parsed.body, "parent line");
        assert_eq!(parsed.route.as_deref(), Some("work"));
        assert_eq!(
            parsed.sub_bullets,
            vec!["first child".to_string(), "second child".to_string()]
        );
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
                parsed.sub_bullets,
                vec!["child one".to_string(), "child two".to_string()],
                "{raw}"
            );
        }
    }

    #[test]
    fn execution_skips_blank_and_placeholder_child_lines() {
        let parsed =
            execute("parent\n\n- real child\n- \n-\t\n   \n").expect("parse");
        assert_eq!(parsed.sub_bullets, vec!["real child".to_string()]);
    }

    #[test]
    fn execution_rejects_indented_or_nested_child_lines() {
        let error = execute("parent\n  - nested").unwrap_err();
        assert_eq!(
            error,
            "capture line 2 must be a flat bullet starting with \"- \", \
\"* \", or \"+ \" at the start of the line, or left blank"
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
            parsed.sub_bullets,
            vec![
                "Confirm the owner".to_string(),
                "Attach the checklist".to_string()
            ]
        );
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
            parsed.sub_bullets,
            vec!["\u{1f680} launch".to_string(), "\u{e9}tude".to_string()]
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
        assert_eq!(parsed.sub_bullets, vec!["child @home".to_string()]);
    }

    #[test]
    fn execution_ordinary_single_line_capture_has_no_sub_bullets() {
        let parsed = execute("buy milk @groceries").expect("parse");
        assert!(parsed.sub_bullets.is_empty());
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
            parse.sub_bullets,
            vec!["first child".to_string(), "second child".to_string()]
        );
        assert!(parse.diagnostics.is_empty());
    }

    #[test]
    fn editor_diagnoses_an_invalid_child_line_without_failing() {
        let parse = editor("parent\n  - nested");
        assert_eq!(parse.body, "parent");
        assert!(parse.sub_bullets.is_empty());
        assert_eq!(codes(&parse), vec!["invalid_child_line"]);
        let raw = "parent\n  - nested";
        let expected_start = raw.find("  - nested").expect("line 2 offset");
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
    fn editor_placeholder_and_blank_child_lines_produce_no_sub_bullet_or_diagnostic(
    ) {
        let parse = editor("parent\n\n- real child\n- \n");
        assert_eq!(parse.sub_bullets, vec!["real child".to_string()]);
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
        assert_eq!(parse.sub_bullets, vec!["do it".to_string()]);
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
}
