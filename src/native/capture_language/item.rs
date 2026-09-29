//! Capture-item and session-operator parsing.

use super::close_selection::*;
use super::draft::*;
use super::editor_parse::*;
use super::line::*;
use super::markers::*;
use super::model::*;
use super::project_tasks::*;
use super::tokens::*;

pub(super) struct ParsedCaptureItemOutcome<'a> {
    pub(super) index: usize,
    pub(super) start: usize,
    pub(super) end: usize,
    pub(super) line_start: usize,
    pub(super) line_end: usize,
    pub(super) parsed: ParsedCaptureText,
    pub(super) declarations: Vec<GlobalDeclarationToken<'a>>,
    pub(super) local_destination_marker: Option<String>,
}

pub(super) fn parse_capture_item<'a>(
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
    if let Some(outcome) = parse_pomodoro_equals_item(
        item,
        parent_line,
        forced_route,
        forced_section,
    )? {
        return Ok(outcome);
    }
    if let Some(outcome) = parse_pomodoro_adjust_item(
        item,
        parent_line,
        forced_route,
        forced_section,
    )? {
        return Ok(outcome);
    }
    if let Some(outcome) = parse_pomodoro_link_item(
        item,
        parent_line,
        forced_route,
        forced_section,
    )? {
        return Ok(outcome);
    }
    let parent_tokens = tokenize_line_with_spans(&parent_line.raw);
    let parent_outcome =
        resolve_line(parent_tokens, true, detect_route, parse_clip_markers)?;
    declarations.extend(global_declarations_from_tokens(
        parent_outcome.declarations,
        parent_line.line_number,
    ));
    let parent_body_is_empty = parent_outcome.body.is_empty();
    let parent_is_toggle_candidate = matches!(
        parent_outcome.route.as_ref().map(|route| &route.token.kind),
        Some(
            CaptureKind::SubBullet {
                target: SubBulletTarget::BlockId(_),
                ..
            } | CaptureKind::TaskToggle { .. }
        )
    );
    let parent_is_pomodoro_candidate = matches!(
        parent_outcome.route.as_ref().map(|route| &route.token.kind),
        Some(CaptureKind::Pomodoro { .. })
    );
    if parent_body_is_empty
        && !parent_is_toggle_candidate
        && !parent_is_pomodoro_candidate
    {
        return Err(missing_text_error());
    }

    let mut aggregate = AggregateMarkers::default();
    aggregate.absorb(parent_outcome.markers, parent_outcome.route)?;

    let mut sub_bullets = Vec::new();
    let mut sub_bullet_lines = Vec::new();
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
            task_id: None,
        });
        sub_bullet_lines.push(line_number);
        if authored.depth == AuthoredDepth::First {
            has_first_level_owner = true;
        }
    }

    if (forced_route.is_some() || forced_section.is_some())
        && !parent_is_pomodoro_candidate
    {
        let trimmed = parent_line.raw.text.trim();
        if trimmed.split_whitespace().count() == 1
            && is_pomodoro_marker_candidate(trimmed)
            && parse_pomodoro_route_token(trimmed).is_ok()
            && item.lines.len() == 1
        {
            return Err(
                "Pomodoro link capture cannot be combined with --route, --section, --task, --task-ref, --task-section, or --clip; capture the link alone".to_string(),
            );
        }
    }

    if parent_is_pomodoro_candidate && parent_body_is_empty {
        if forced_route.is_some() || forced_section.is_some() {
            return Err(
                "Pomodoro link capture cannot be combined with --route, --section, --task, --task-ref, --task-section, or --clip; capture the link alone".to_string(),
            );
        }
        if aggregate.clip.is_some() {
            return Err(
                "Pomodoro link capture cannot be combined with % clipboard markers".to_string(),
            );
        }
        if aggregate.scheduled_offset.is_some() {
            return Err("Pomodoro link capture cannot be combined with s:<N>"
                .to_string());
        }
        if aggregate.priority_level.is_some() {
            return Err("Pomodoro link capture cannot be combined with p:<N>"
                .to_string());
        }
        if !sub_bullets.is_empty() {
            return Err(
                "Pomodoro link capture cannot be combined with authored child bullets".to_string(),
            );
        }
        let marker_text = aggregate
            .route
            .as_ref()
            .map(|route| route.marker_text.clone());
        let (route, kind) = match aggregate.route {
            Some(line_route) => (line_route.token.route, line_route.token.kind),
            None => (None, CaptureKind::Task),
        };
        let CaptureKind::Pomodoro {
            block_id,
            pomodoro_name,
            start,
            close,
        } = kind
        else {
            return Err(missing_text_error());
        };
        return Ok(parsed_capture_item_outcome(
            item,
            ParsedCaptureText {
                body: String::new(),
                clip: None,
                route,
                kind: CaptureKind::PomodoroLink {
                    block_id,
                    pomodoro_name,
                    start,
                    close,
                    spelling: PomodoroLinkSpelling::At,
                },
                scheduled_offset: None,
                priority_level: None,
                sub_bullets: Vec::new(),
            },
            declarations,
            marker_text,
        ));
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
    let kind = resolve_sub_bullet_kind(
        kind,
        parent_body_is_empty,
        sub_bullets.is_empty(),
        aggregate.clip.is_none()
            && aggregate.scheduled_offset.is_none()
            && aggregate.priority_level.is_none(),
    )?;
    if matches!(kind, CaptureKind::PomodoroNote) {
        if aggregate.scheduled_offset.is_some() {
            return Err(pomodoro_note_schedule_conflict_error());
        }
        if aggregate.priority_level.is_some() {
            return Err(pomodoro_note_priority_conflict_error());
        }
    }
    if let CaptureKind::Pomodoro { start: Some(_), .. } = &kind {
        if aggregate.scheduled_offset.is_some() {
            return Err(POMODORO_START_SCHEDULE_CONFLICT_ERROR.to_string());
        }
        if aggregate.priority_level.is_some() {
            return Err(POMODORO_START_PRIORITY_CONFLICT_ERROR.to_string());
        }
    }
    if let CaptureKind::Pomodoro { close: Some(_), .. } = &kind {
        if aggregate.scheduled_offset.is_some() {
            return Err(POMODORO_CLOSE_SCHEDULE_CONFLICT_ERROR.to_string());
        }
        if aggregate.priority_level.is_some() {
            return Err(POMODORO_CLOSE_PRIORITY_CONFLICT_ERROR.to_string());
        }
    }
    // A project-note `#pomodoro` name picks the Pomodoro that ` :<id>`
    // Task Links go under. The trailing-ID post-pass below strips accepted
    // IDs from bodies and records them on the sub-bullets; a name with no
    // ` :` task stays unused.
    let mut has_link_task = false;
    if matches!(kind, CaptureKind::ProjectNote { .. }) {
        let mut pass = ProjectTaskPass::new();
        if let Some(message) = pass.check_parent(&parent_outcome.body) {
            return Err(message);
        }
        for (index, child) in sub_bullets.iter_mut().enumerate() {
            let line_number = sub_bullet_lines[index];
            match pass.check_child(&child.body, child.depth, line_number) {
                ChildTaskOutcome::Ignore => {}
                ChildTaskOutcome::Unfinished { sigil } => {
                    return Err(unfinished_project_task_id_error(sigil));
                }
                ChildTaskOutcome::Error { message, .. } => {
                    return Err(message);
                }
                ChildTaskOutcome::Accept {
                    sigil,
                    id,
                    stripped,
                } => {
                    child.body = stripped;
                    child.task_id = Some(ProjectTaskId {
                        block_id: id,
                        link: sigil == ':',
                    });
                }
            }
        }
        has_link_task = pass.has_link;
    }
    if let CaptureKind::ProjectNote {
        pomodoro_name: Some(name),
        ..
    } = &kind
        && !has_link_task
    {
        return Err(unused_project_note_pomodoro_error(name));
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

/// Decide whether a resolved `@route+block-id[#name]` marker stays an
/// ordinary sub-bullet capture or becomes a task toggle, once the finished
/// item -- not just the marker's own token -- is known. A toggle needs an
/// empty parent body, no authored children, and no other item-wide marker;
/// an item that looks like a toggle but fails one of those still needs text,
/// exactly like it always has. Every other kind passes through unchanged.
pub(super) fn resolve_sub_bullet_kind(
    kind: CaptureKind,
    parent_body_is_empty: bool,
    sub_bullets_is_empty: bool,
    no_other_item_markers: bool,
) -> Result<CaptureKind, String> {
    if matches!(kind, CaptureKind::TaskToggle { .. }) {
        if parent_body_is_empty && sub_bullets_is_empty && no_other_item_markers
        {
            return Ok(kind);
        }
        return Err(EXPLICIT_TOGGLE_ONLY_MARKER_ERROR.to_string());
    }
    let CaptureKind::SubBullet {
        target: SubBulletTarget::BlockId(block_id),
        section,
    } = kind
    else {
        return Ok(kind);
    };
    if !parent_body_is_empty {
        if let Some(section) = &section
            && !is_selector_component(&section.text)
        {
            return Err(SUB_BULLET_SECTION_ERROR.to_string());
        }
        return Ok(CaptureKind::SubBullet {
            target: SubBulletTarget::BlockId(block_id),
            section,
        });
    }
    if sub_bullets_is_empty && no_other_item_markers {
        return Ok(CaptureKind::TaskToggle {
            block_id,
            pomodoro_name: section.map(|selector| selector.text),
            intent: TaskToggleIntent::EnsureNext,
        });
    }
    Err(missing_text_error())
}

pub(super) fn parsed_capture_item_outcome<'a>(
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

/// One small lexer for whole-item Pomodoro session operators: one sign
/// resizes, two identical signs shift, and the ASCII-digit count is
/// optional. Returns the operator, the digit text, and the token length in
/// bytes, or `None` when `text` does not start with an operator token.
///
/// A token is a sign run of exactly `+`, `-`, `++`, or `--` followed by
/// zero or more ASCII digits. Longer or mixed runs (`+++`, `---`, `+-`,
/// `-+3`) are not tokens, so they stay ordinary prose.
pub(crate) fn session_operator_token(
    text: &str,
) -> Option<(SessionOperator, &str, usize)> {
    let bytes = text.as_bytes();
    if bytes.len() >= 2 && text.starts_with("++") {
        if bytes
            .get(2)
            .is_some_and(|byte| *byte == b'+' || *byte == b'-')
        {
            return None;
        }
        let mut len = 2;
        while len < bytes.len() && bytes[len].is_ascii_digit() {
            len += 1;
        }
        return Some((
            SessionOperator::Shift { later: true },
            &text[2..len],
            len,
        ));
    }
    if bytes.len() >= 2 && text.starts_with("--") {
        if bytes
            .get(2)
            .is_some_and(|byte| *byte == b'+' || *byte == b'-')
        {
            return None;
        }
        let mut len = 2;
        while len < bytes.len() && bytes[len].is_ascii_digit() {
            len += 1;
        }
        return Some((
            SessionOperator::Shift { later: false },
            &text[2..len],
            len,
        ));
    }
    if bytes
        .first()
        .is_some_and(|byte| *byte == b'+' || *byte == b'-')
    {
        if bytes
            .get(1)
            .is_some_and(|byte| *byte == b'+' || *byte == b'-')
        {
            return None;
        }
        let plus = bytes[0] == b'+';
        let mut len = 1;
        while len < bytes.len() && bytes[len].is_ascii_digit() {
            len += 1;
        }
        return Some((SessionOperator::Resize { plus }, &text[1..len], len));
    }
    None
}

/// A whole-item `=`-family token: either a close (`=x`) or a start
/// (`=` plus an `se<X>`-shaped suffix, plus an optional `#name` part).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum EqualsToken {
    /// A leading `=x`/`=X`. The caller keeps today's close recognition
    /// (exact token, first-token near miss, otherwise prose) because `x`
    /// is never a start-suffix character. `=x#…` still lexes as `Close`.
    Close,
    /// `=` plus the longest `[0-9]*(-[0-9]*)?` run, plus an optional
    /// `#name` part. `suffix` excludes the `=`, `counted` is true when the
    /// suffix holds at least one digit, `name` is `Some` when a `#`
    /// immediately follows the suffix (possibly empty, as in `=#`), and
    /// `len` is the token's byte length including the name part.
    Start {
        suffix: String,
        counted: bool,
        len: usize,
        name: Option<String>,
    },
}

/// Lex the `=`-family token at the start of trimmed item text, mirroring
/// [`session_operator_token`]. Returns `None` when the text does not start
/// with `=`, so ordinary parsing continues. A bare token (`=`, `=-`) has
/// an empty or digit-free suffix; a counted token (`=3`, `=-2`, `=3-`,
/// `=2-1`, `=0`) carries at least one digit. An optional `#name` part
/// immediately after the suffix extends the token to the next ASCII
/// whitespace (or end of line); the `#` must come immediately after the
/// suffix.
pub(crate) fn session_equals_token(text: &str) -> Option<EqualsToken> {
    let rest = text.strip_prefix('=')?;
    if rest
        .as_bytes()
        .first()
        .is_some_and(|byte| byte.eq_ignore_ascii_case(&b'x'))
    {
        return Some(EqualsToken::Close);
    }
    let bytes = rest.as_bytes();
    let mut len = 0;
    while len < bytes.len() && bytes[len].is_ascii_digit() {
        len += 1;
    }
    if bytes.get(len) == Some(&b'-') {
        len += 1;
        while len < bytes.len() && bytes[len].is_ascii_digit() {
            len += 1;
        }
    }
    let suffix = rest[..len].to_string();
    let counted = suffix.bytes().any(|byte| byte.is_ascii_digit());
    let mut token_len = len + 1;
    let mut name: Option<String> = None;
    if rest.as_bytes().get(len) == Some(&b'#') {
        let after_hash = &rest[len + 1..];
        let name_len = after_hash
            .find(|character: char| character.is_ascii_whitespace())
            .unwrap_or(after_hash.len());
        name = Some(after_hash[..name_len].to_string());
        token_len += 1 + name_len;
    }
    Some(EqualsToken::Start {
        suffix,
        counted,
        len: token_len,
        name,
    })
}

/// Whether a whitespace-separated token can take part in a same-line
/// session-operator chain. A token qualifies exactly when the whole-item
/// session parsers would claim it as a standalone one-line item: either
/// [`parse_pomodoro_equals_item`] or [`parse_pomodoro_adjust_item`] returns
/// something other than `Ok(None)` for it. "Claimed" therefore includes
/// both valid tokens and near misses (zero magnitudes, overflows, counted
/// tokens with extra text, selection-shaped close fragments), so a broken
/// token inside a chain gets its own precise family diagnostic on its own
/// range instead of a vague shape error. A same-line chain has already been
/// split into single-token items upstream in `draft.rs`, so these parsers
/// only ever see exact single-token items.
/// [`parse_pomodoro_equals_item`]: parse_pomodoro_equals_item
/// [`parse_pomodoro_adjust_item`]: parse_pomodoro_adjust_item
pub(super) fn is_session_chain_token(token: &str) -> bool {
    if let Some((_, digits, len)) = session_operator_token(token) {
        return len == token.len() || !digits.is_empty();
    }
    match session_equals_token(token) {
        Some(EqualsToken::Start {
            counted, len, name, ..
        }) => {
            if name.is_some() {
                return true;
            }
            len == token.len() || counted
        }
        Some(EqualsToken::Close) => {
            token.eq_ignore_ascii_case("=x")
                || whole_item_close_after_x(token).is_some()
                || is_close_hash_token(token)
        }
        None => false,
    }
}

/// Whether a whitespace-free token is a `=x#…` close near miss: `=x`/`=X`
/// immediately followed by `#`.
pub(super) fn is_close_hash_token(token: &str) -> bool {
    let bytes = token.as_bytes();
    bytes.len() >= 3
        && bytes[0] == b'='
        && bytes[1].eq_ignore_ascii_case(&b'x')
        && bytes[2] == b'#'
}

/// Whole-item session-operator grammar: one sign resizes, two signs shift.
///
/// A same-line chain has already been split into single-token items
/// upstream in `draft.rs`, so this parser only ever sees an exact
/// single-token item here.
///
/// Returns `Ok(None)` when the item does not start with an operator token
/// and ordinary parsing should continue. Returns `Ok(Some(outcome))` for
/// an exact single-token operator (an omitted count means 1). Returns
/// `Err` for every operator-shaped near miss: a zero magnitude, an
/// overflow, a counted token with extra text, or a bare-or-counted token
/// with child lines. Near misses are never allowed to fall through as
/// ordinary tasks. A bare sign run followed by more text on the same line
/// (`- foo`, `++ plan`), a longer or mixed run (`+++`, `+-`), and any
/// mid-body token (`Plan ++3`, `C++`) stay ordinary prose.
pub(super) fn parse_pomodoro_adjust_item<'a>(
    item: &CaptureItem<'a>,
    parent_line: &ItemLine<'a>,
    forced_route: Option<&str>,
    forced_section: Option<&str>,
) -> Result<Option<ParsedCaptureItemOutcome<'a>>, String> {
    let parent_trimmed = parent_line.raw.text.trim();
    let Some((operator, digits, prefix_len)) =
        session_operator_token(parent_trimmed)
    else {
        return Ok(None);
    };
    let exact = parent_trimmed.len() == prefix_len && item.lines.len() == 1;
    if !exact {
        // A counted token claims the item even with extra text: `+5 more`,
        // `++3@work`, `--2 s:1`, `++3++`. A bare token claims only a child
        // line; bare text on the same line (`- foo`, `-- aside`) stays
        // prose.
        if !digits.is_empty() {
            return Err(match operator {
                SessionOperator::Resize { .. } => {
                    POMODORO_ADJUST_SHAPE_ERROR.to_string()
                }
                SessionOperator::Shift { .. } => {
                    POMODORO_SHIFT_SHAPE_ERROR.to_string()
                }
            });
        }
        if parent_trimmed.len() == prefix_len && item.lines.len() > 1 {
            return Err(match operator {
                SessionOperator::Resize { .. } => {
                    POMODORO_ADJUST_SHAPE_ERROR.to_string()
                }
                SessionOperator::Shift { .. } => {
                    POMODORO_SHIFT_SHAPE_ERROR.to_string()
                }
            });
        }
        return Ok(None);
    }
    let units = if digits.is_empty() {
        1
    } else {
        digits.parse::<u64>().map_err(|_| match operator {
            SessionOperator::Resize { .. } => {
                POMODORO_ADJUST_OVERFLOW_ERROR.to_string()
            }
            SessionOperator::Shift { .. } => {
                POMODORO_SHIFT_OVERFLOW_ERROR.to_string()
            }
        })?
    };
    if units == 0 {
        return Err(match operator {
            SessionOperator::Resize { .. } => {
                POMODORO_ADJUST_ZERO_ERROR.to_string()
            }
            SessionOperator::Shift { .. } => {
                POMODORO_SHIFT_ZERO_ERROR.to_string()
            }
        });
    }
    if forced_route.is_some() || forced_section.is_some() {
        return Err(match operator {
            SessionOperator::Resize { .. } => {
                POMODORO_ADJUST_FORCED_ERROR.to_string()
            }
            SessionOperator::Shift { .. } => {
                POMODORO_SHIFT_FORCED_ERROR.to_string()
            }
        });
    }
    let raw = parent_trimmed.to_string();
    let kind = match operator {
        SessionOperator::Resize { plus } => CaptureKind::PomodoroAdjust {
            spec: PomodoroAdjustSpec {
                raw: raw.clone(),
                plus,
                units,
            },
        },
        SessionOperator::Shift { later } => CaptureKind::PomodoroShift {
            spec: PomodoroShiftSpec {
                raw: raw.clone(),
                later,
                units,
            },
        },
    };
    Ok(Some(parsed_capture_item_outcome(
        item,
        ParsedCaptureText {
            body: raw,
            clip: None,
            route: None,
            kind,
            scheduled_offset: None,
            priority_level: None,
            sub_bullets: Vec::new(),
        },
        Vec::new(),
        None,
    )))
}

/// A named start token's own error, in E1/E3/E2/overflow order. `None`
/// when the token itself is well-formed (callers then report E4 for extra
/// text or accept the exact token).
fn named_token_error(token: &str, suffix: &str, name: &str) -> Option<String> {
    if name.is_empty() {
        return Some(pomodoro_named_start_incomplete_error(token));
    }
    if let Some((before, after)) = name.split_once('=')
        && suffix.is_empty()
        && parse_pomodoro_start_suffix(after).is_ok()
    {
        return Some(pomodoro_named_start_order_error(token, before, after));
    }
    if !is_pomodoro_selector_component(name) {
        return Some(pomodoro_named_start_name_error(name, token));
    }
    if let Err(message) = parse_pomodoro_start_suffix(suffix) {
        return Some(message);
    }
    None
}

/// E4 for a well-formed named token with extra text or child lines.
pub(super) fn named_shape_error(
    token: &str,
    suffix: &str,
    name: &str,
    parent_trimmed: &str,
    len: usize,
) -> String {
    if name.is_empty()
        && let Some(nospace) = named_nospace_error(suffix, parent_trimmed, len)
    {
        return nospace;
    }
    let mut message = pomodoro_named_start_shape_error(token, suffix, name);
    if !name.is_empty() && parent_trimmed.len() > len {
        let extra = parent_trimmed[len..].trim_start();
        let words: Vec<&str> = extra.split_whitespace().collect();
        if !words.is_empty()
            && words
                .iter()
                .all(|word| is_pomodoro_selector_component(word))
        {
            message.push_str(&pomodoro_named_start_multiword_hint(
                suffix, name, &words,
            ));
        }
    }
    message
}

/// E4 no-space variant for `=# <word>`: the name must follow `#` directly.
pub(super) fn named_nospace_error(
    suffix: &str,
    parent_trimmed: &str,
    len: usize,
) -> Option<String> {
    let extra = parent_trimmed.get(len..)?.trim_start();
    let word = extra.split_whitespace().next()?;
    if word.is_empty() {
        return None;
    }
    Some(pomodoro_named_start_nospace_error(suffix, word))
}

/// Absolute byte offset of a whole-item close token's `after_x` text (the
/// lists after `=x`/`=X`), so selection diagnostics point at the original
/// input. `first` is the parent line's first whitespace-delimited token.
fn close_after_x_offset(
    parent_line: &ItemLine<'_>,
    first: &str,
    after_x: &str,
) -> usize {
    let leading =
        parent_line.raw.text.len() - parent_line.raw.text.trim_start().len();
    parent_line.raw.start + leading + (first.len() - after_x.len())
}

/// Whole-item `=`-family grammar: `=x[<N>][!<M>]` closes, `=`/`=<X>` starts.
///
/// A same-line chain has already been split into single-token items
/// upstream in `draft.rs`, so this parser only ever sees an exact
/// single-token item here.
///
/// Runs first (before session operators, caret links, and ordinary
/// parsing). Returns `Ok(None)` when the item is not `=`-shaped and
/// ordinary parsing should continue. Returns `Ok(Some(outcome))` for an
/// exact single-token close or start. Returns `Err` for every near miss: a
/// leading `=x` token with extra text, markers, or child lines; a counted
/// start token (`=3`, `=-2`, `=3-`, `=2-1`, `=0`) with anything else; or an
/// exact start token with child lines. Near misses never fall through as
/// ordinary tasks. A bare token followed by more text on the same line
/// (`= foo`, `=- foo`, `==`, `=-)`), every other close shape (`=xx`, `=xa`,
/// `=x.`), and mid-body tokens (`Plan =3`, `a=3`, `Plan =x1`) stay ordinary
/// prose: a bare sign run followed by prose stays prose while a counted
/// token claims its item.
///
/// A selection-shaped first token (`=x`/`=X` followed by a digit, `,`, or
/// `!`) claims the item the same way: an exact single-line item lexes its
/// lists (a dangling separator is an incomplete error here), while anything
/// else reports the list's own diagnostic, the no-spaces hint when the
/// spaceless join forms a selection, or the close shape error.
pub(super) fn parse_pomodoro_equals_item<'a>(
    item: &CaptureItem<'a>,
    parent_line: &ItemLine<'a>,
    forced_route: Option<&str>,
    forced_section: Option<&str>,
) -> Result<Option<ParsedCaptureItemOutcome<'a>>, String> {
    let parent_trimmed = parent_line.raw.text.trim();
    let Some(token) = session_equals_token(parent_trimmed) else {
        return Ok(None);
    };
    match token {
        EqualsToken::Close => {
            let Some(first) = parent_trimmed.split_whitespace().next() else {
                return Ok(None);
            };
            if is_close_hash_token(first) {
                let name = first
                    .find('#')
                    .map(|hash| first[hash + 1..].to_string())
                    .unwrap_or_default();
                return Err(pomodoro_close_hash_error(&name));
            }
            let selection_after_x = whole_item_close_after_x(first);
            if selection_after_x.is_none() && !first.eq_ignore_ascii_case("=x")
            {
                return Ok(None);
            }
            let single_token_parent = parent_trimmed == first;
            if single_token_parent && item.lines.len() == 1 {
                if forced_route.is_some() || forced_section.is_some() {
                    return Err(POMODORO_CLOSE_FORCED_ERROR.to_string());
                }
                let raw = parent_trimmed.to_string();
                let spec = match selection_after_x {
                    None => PomodoroCloseSpec::plain(raw.clone()),
                    Some(after_x) => {
                        let offset =
                            close_after_x_offset(parent_line, first, after_x);
                        match lex_close_selection(after_x, offset, first) {
                            Ok(CloseSelectionOutcome::Valid(lex)) => {
                                close_spec_from_lex(raw.clone(), &lex)
                            }
                            Ok(CloseSelectionOutcome::Incomplete(
                                incomplete,
                            )) => {
                                return Err(close_selection_incomplete_error(
                                    first,
                                    incomplete.separator,
                                ));
                            }
                            Err(error) => return Err(error.message),
                        }
                    }
                };
                return Ok(Some(parsed_capture_item_outcome(
                    item,
                    ParsedCaptureText {
                        body: raw,
                        clip: None,
                        route: None,
                        kind: CaptureKind::PomodoroClose { spec },
                        scheduled_offset: None,
                        priority_level: None,
                        sub_bullets: Vec::new(),
                    },
                    Vec::new(),
                    None,
                )));
            }
            if single_token_parent {
                // A single-token parent with child lines: a broken list
                // reports its own diagnostic, anything else the shape error.
                if let Some(after_x) = selection_after_x {
                    let offset =
                        close_after_x_offset(parent_line, first, after_x);
                    if let Err(error) =
                        lex_close_selection(after_x, offset, first)
                    {
                        return Err(error.message);
                    }
                }
                return Err(POMODORO_CLOSE_SHAPE_ERROR.to_string());
            }
            // Extra text on the parent line: a broken first token reports
            // its own diagnostic first.
            if let Some(after_x) = selection_after_x {
                let offset = close_after_x_offset(parent_line, first, after_x);
                if let Err(error) = lex_close_selection(after_x, offset, first)
                {
                    return Err(error.message);
                }
            }
            // A spaceless join that forms a selection (`=x 1,3`, `=x1, 3`)
            // gets the no-spaces hint instead of the shape error.
            let nospace: String = parent_trimmed.split_whitespace().collect();
            if whole_item_close_after_x(&nospace).is_some_and(|after_x| {
                lex_close_selection(after_x, 0, &nospace).is_ok()
            }) {
                return Err(close_selection_no_spaces_error());
            }
            Err(POMODORO_CLOSE_SHAPE_ERROR.to_string())
        }
        EqualsToken::Start {
            suffix,
            counted,
            len,
            name,
        } => {
            if let Some(selector) = name {
                let token_text =
                    parent_trimmed.get(..len).unwrap_or(parent_trimmed);
                let exact =
                    parent_trimmed.len() == len && item.lines.len() == 1;
                if !exact {
                    let has_extra = parent_trimmed.len() > len;
                    let has_children = item.lines.len() > 1;
                    if has_extra || has_children {
                        if let Some(error) =
                            named_token_error(token_text, &suffix, &selector)
                        {
                            if selector.is_empty()
                                && has_extra
                                && let Some(nospace) = named_nospace_error(
                                    &suffix,
                                    parent_trimmed,
                                    len,
                                )
                            {
                                return Err(nospace);
                            }
                            return Err(error);
                        }
                        return Err(named_shape_error(
                            token_text,
                            &suffix,
                            &selector,
                            parent_trimmed,
                            len,
                        ));
                    }
                    return Ok(None);
                }
                if let Some(error) =
                    named_token_error(token_text, &suffix, &selector)
                {
                    return Err(error);
                }
                let spec = parse_pomodoro_start_suffix(&suffix)?;
                if forced_route.is_some() || forced_section.is_some() {
                    return Err(POMODORO_START_FORCED_ERROR.to_string());
                }
                let raw = parent_trimmed.to_string();
                return Ok(Some(parsed_capture_item_outcome(
                    item,
                    ParsedCaptureText {
                        body: raw,
                        clip: None,
                        route: None,
                        kind: CaptureKind::PomodoroStart {
                            spec,
                            pomodoro_name: Some(selector),
                        },
                        scheduled_offset: None,
                        priority_level: None,
                        sub_bullets: Vec::new(),
                    },
                    Vec::new(),
                    None,
                )));
            }
            let exact = parent_trimmed.len() == len && item.lines.len() == 1;
            if !exact {
                // A counted token claims the item even with extra text:
                // `=3 more`, `=-2 @work`, `=3s:1`, `=2-1-`, `=3x`. An
                // exact token with child lines claims it too, bare or
                // counted. A bare token with more text on the same line
                // (`= foo`, `==`) stays prose.
                if counted
                    || (parent_trimmed.len() == len && item.lines.len() > 1)
                {
                    let token_text = &parent_trimmed[..len];
                    return Err(pomodoro_start_shape_error(
                        token_text, &suffix,
                    ));
                }
                return Ok(None);
            }
            let spec = parse_pomodoro_start_suffix(&suffix)?;
            if forced_route.is_some() || forced_section.is_some() {
                return Err(POMODORO_START_FORCED_ERROR.to_string());
            }
            let raw = parent_trimmed.to_string();
            Ok(Some(parsed_capture_item_outcome(
                item,
                ParsedCaptureText {
                    body: raw,
                    clip: None,
                    route: None,
                    kind: CaptureKind::PomodoroStart {
                        spec,
                        pomodoro_name: None,
                    },
                    scheduled_offset: None,
                    priority_level: None,
                    sub_bullets: Vec::new(),
                },
                Vec::new(),
                None,
            )))
        }
    }
}
