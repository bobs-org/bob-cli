//! Route, caret, and selector parsers.

use super::close_selection::*;
use super::editor_parse::*;
use super::item::*;
use super::markers::*;
use super::model::*;
use crate::native::collect_done;

/// Parse one whitespace-free token as an `@route` token, returning `None` when
/// it does not begin with `@` or its route part is not a valid route name. A
/// `#` suffix selects bullet mode and is split off before route validation.
pub(super) fn parse_route_token(token: &str) -> Option<RouteToken> {
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

pub(super) fn parse_terminal_route_token(
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
    if let Some(message) = retired_route_less_project_note_error(token) {
        return Err(message);
    }
    if is_pomodoro_marker_candidate(token) {
        return parse_pomodoro_route_token(token).map(Some);
    }
    Ok(parse_route_token(token))
}

pub(super) fn parse_sub_bullet_route_token(
    token: &str,
) -> Result<RouteToken, String> {
    if let Some(message) = explicit_toggle_unsupported_message(token) {
        return Err(message.to_string());
    }
    let (token, explicit_toggle) = match exact_explicit_toggle_prefix(token) {
        Some(prefix) => (prefix, true),
        None => (token, false),
    };
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
        // The strict task-section charset (no `+`) is enforced later, once
        // the finished item shows whether this stays a sub-bullet capture or
        // becomes a task toggle -- whose trailing name is a Pomodoro name and
        // takes the wider Pomodoro charset instead. See
        // `resolve_sub_bullet_kind`.
        Some(selector) if !is_pomodoro_selector_component(selector) => {
            return Err(SUB_BULLET_SECTION_ERROR.to_string());
        }
        Some(selector) => Some(TaskSectionSelector {
            text: selector.to_string(),
            exact: false,
        }),
    };

    if explicit_toggle {
        return Ok(RouteToken {
            route: Some(route.to_ascii_lowercase()),
            kind: CaptureKind::TaskToggle {
                block_id: block_id.to_string(),
                pomodoro_name: None,
                intent: TaskToggleIntent::Toggle,
            },
        });
    }

    Ok(RouteToken {
        route: Some(route.to_ascii_lowercase()),
        kind: CaptureKind::SubBullet {
            target: SubBulletTarget::BlockId(block_id.to_string()),
            section,
        },
    })
}

/// The prefix of an exact `@<route>+<block-id>!` token, without the terminal
/// `!`. `None` for every near-miss (`!!`, `#name!`, `@@...!`, incomplete).
pub(super) fn exact_explicit_toggle_prefix(token: &str) -> Option<&str> {
    let prefix = token.strip_suffix('!')?;
    if prefix.ends_with('!') || prefix.starts_with("@@") {
        return None;
    }
    let marker = prefix.strip_prefix('@')?;
    let (route, rest) = marker.split_once('+')?;
    if rest.contains('#') || route.is_empty() || rest.is_empty() {
        return None;
    }
    Some(prefix)
}

/// Focused diagnostic for an `@...!` token that is not the exact
/// marker-only explicit-toggle form. Ordinary prose `!` and `@!route:id` stay
/// out of this family.
pub(super) fn explicit_toggle_unsupported_message(
    token: &str,
) -> Option<&'static str> {
    if exact_explicit_toggle_prefix(token).is_some() {
        return None;
    }
    if !token.ends_with('!') {
        return None;
    }
    if token.starts_with("@!")
        && token.bytes().filter(|byte| *byte == b'!').count() == 1
    {
        return None;
    }
    if token.starts_with("@@") {
        return Some(EXPLICIT_TOGGLE_GLOBAL_ERROR);
    }
    let after_at = token.strip_prefix('@')?;
    if !after_at.contains('+') {
        return None;
    }
    if token.bytes().filter(|byte| *byte == b'!').count() > 1 {
        return Some(EXPLICIT_TOGGLE_REPEATED_ERROR);
    }
    if after_at.contains('#') {
        return Some(EXPLICIT_TOGGLE_NAMED_ERROR);
    }
    Some(EXPLICIT_TOGGLE_ONLY_MARKER_ERROR)
}

/// Return whether one already-whitespace-free selector component is typeable
/// as a task-section third component (`@route+id#section`).
///
/// ASCII alphanumerics plus `& ' ( ) , . / -`. The Pomodoro-name third
/// component (`@route:id#pomodoro`) is this grammar plus `+`; see
/// [`is_pomodoro_selector_component`].
pub(crate) fn is_selector_component(value: &str) -> bool {
    !value.is_empty() && value.bytes().all(is_selector_byte)
}

/// Return whether one already-whitespace-free selector component is typeable
/// as a Pomodoro-name third component (`@route:id#pomodoro`).
///
/// Same bytes as [`is_selector_component`], plus `+`.
pub(crate) fn is_pomodoro_selector_component(value: &str) -> bool {
    !value.is_empty() && value.bytes().all(is_pomodoro_selector_byte)
}

pub(super) fn is_selector_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric()
        || matches!(
            byte,
            b'&' | b'\'' | b'(' | b')' | b',' | b'.' | b'/' | b'-'
        )
}

pub(super) fn is_pomodoro_selector_byte(byte: u8) -> bool {
    is_selector_byte(byte) || byte == b'+'
}

pub(super) fn is_sub_bullet_marker_candidate(token: &str) -> bool {
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

pub(super) fn parse_task_block_id_route_token(
    token: &str,
) -> Result<RouteToken, String> {
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
    // Split the part after `^` at the first `#` into the block part and an
    // optional Pomodoro name. The project-note `+` sits immediately after
    // the block ID: with it, a `+` after `#name` is ordinary Pomodoro-name
    // charset (so `@r^id+#c++` names `c++`); without it, a trailing `+` is
    // a misordered marker and a bare `#name` is missing the `+`.
    if let Some((block_part, name)) = block_id.split_once('#') {
        let Some(block_core) = block_part.strip_suffix('+') else {
            if let Some(name_core) = name.strip_suffix('+') {
                return Err(project_note_misordered_error(
                    route, block_part, name_core,
                ));
            }
            return Err(project_note_name_without_plus_error(
                route, block_part, name,
            ));
        };
        if block_core.ends_with('+') {
            return Err(TASK_BLOCK_ID_ERROR.to_string());
        }
        // A `=` anywhere after the `+` is a session suffix on a project
        // note, which stays rejected.
        if block_id.contains('=') {
            let raw = block_id
                .split_once('=')
                .map(|(_, raw)| raw)
                .unwrap_or_default();
            if raw.eq_ignore_ascii_case("x")
                || link_close_after_x(raw).is_some()
            {
                return Err(POMODORO_CLOSE_PROJECT_NOTE_ERROR.to_string());
            }
            return Err(POMODORO_START_PROJECT_NOTE_ERROR.to_string());
        }
        if block_core.is_empty() || !is_block_id(block_core) {
            return Err(TASK_BLOCK_ID_ERROR.to_string());
        }
        if name.is_empty() {
            return Err(project_note_name_required_error(route, block_core));
        }
        if !is_pomodoro_selector_component(name) {
            return Err(POMODORO_NAME_ERROR.to_string());
        }
        return Ok(RouteToken {
            route: Some(route.to_ascii_lowercase()),
            kind: CaptureKind::ProjectNote {
                block_id: block_core.to_string(),
                pomodoro_name: Some(name.to_string()),
            },
        });
    }
    if block_id.contains('=') {
        let (before_eq, raw) = block_id.split_once('=').unwrap_or_default();
        // The project-note `=` message applies only when the text before
        // the first `=` ends with the project-note `+` (for example
        // `@dev^foo+=3`); otherwise this is an ordinary invalid block ID,
        // as before the epic.
        if before_eq.ends_with('+') {
            if raw.eq_ignore_ascii_case("x")
                || link_close_after_x(raw).is_some()
            {
                return Err(POMODORO_CLOSE_PROJECT_NOTE_ERROR.to_string());
            }
            return Err(POMODORO_START_PROJECT_NOTE_ERROR.to_string());
        }
        return Err(TASK_BLOCK_ID_ERROR.to_string());
    }
    // A single trailing `+` immediately after the block ID is the
    // project-note sigil. `is_block_id` accepts only letters, digits, and
    // `-`, so the slot was previously a hard error and takes nothing away.
    let (block_id, project_note) = match block_id.strip_suffix('+') {
        Some(stripped) => (stripped, true),
        None => (block_id, false),
    };
    if !is_block_id(block_id) {
        return Err(TASK_BLOCK_ID_ERROR.to_string());
    }

    Ok(RouteToken {
        route: Some(route.to_ascii_lowercase()),
        kind: if project_note {
            CaptureKind::ProjectNote {
                block_id: block_id.to_string(),
                pomodoro_name: None,
            }
        } else {
            CaptureKind::TaskWithBlockId {
                block_id: block_id.to_string(),
            }
        },
    })
}

/// Return whether a terminal token belongs to the ordinary task-with-ID
/// marker grammar. A caret that follows `#` remains part of a bullet
/// section prefix, a caret that follows `+` remains part of the
/// sub-bullet block-ID component, and a caret that follows `:` remains
/// part of a Pomodoro block ID.
pub(super) fn is_task_block_id_marker_candidate(token: &str) -> bool {
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
pub(super) fn is_retired_double_colon_marker_candidate(token: &str) -> bool {
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

pub(super) struct ColonLinkParts {
    pub(super) block_id: String,
    pub(super) pomodoro_name: Option<String>,
    pub(super) start: Option<PomodoroStartSpec>,
    pub(super) close: Option<PomodoroCloseSpec>,
}

/// Shared post-sigil `@route:…` / `^route:…` component parser: block ID,
/// optional `#pomodoro` name, and optional session suffix (`=<X>` start or
/// `=x` close). Both sigils share this so their validation stays identical.
/// `typed_token` is the whole marker as typed (for the retired-form error);
/// `caret` selects the `^route:…` callers, whose `+` form gets the
/// caret-specific message instead of the retirement teaching.
pub(super) fn parse_colon_link_tail(
    typed_token: &str,
    route: &str,
    rest: &str,
    caret: bool,
) -> Result<ColonLinkParts, String> {
    // A trailing `+` immediately after the block ID is the retired
    // project-note sigil. It is checked before the session suffix so the
    // retired form always teaches the replacement. A `+` inside the
    // Pomodoro name is ordinary Pomodoro-name charset and stays untouched,
    // so `@sase:deep-fix#bugs+` keeps naming the Pomodoro `bugs+`.
    let before_eq = rest
        .split_once('=')
        .map(|(before, _)| before)
        .unwrap_or(rest);
    let (block_probe, name_probe): (&str, Option<&str>) =
        match before_eq.split_once('#') {
            Some((block, name)) => (block, Some(name)),
            None => (before_eq, None),
        };
    if let Some(stripped) = block_probe.strip_suffix('+') {
        if stripped.is_empty() {
            return Err(if name_probe.is_some() {
                format!(
                    "Pomodoro capture requires a block ID before the Pomodoro name: `@<route>:<block-id>#<pomodoro>` (run `bob capture-tasks -r {}` to list task block IDs)",
                    route.to_ascii_lowercase()
                )
            } else {
                POMODORO_BLOCK_ID_ERROR.to_string()
            });
        }
        if caret {
            return Err(POMODORO_LINK_PROJECT_ERROR.to_string());
        }
        return Err(retired_project_note_marker_error(
            typed_token,
            route,
            stripped,
            name_probe,
        ));
    }
    // Split the additive session suffix before `#` handling: the first `=`
    // separates the old marker from the suffix. `x`/`X` is a close; anything
    // else follows the `=<X>` start shape. Any extra `=` inside is malformed.
    let (rest_before_start, suffix) = match rest.split_once('=') {
        Some((before, suffix)) => (before, Some(suffix)),
        None => (rest, None),
    };
    let session: Option<SessionSuffix> = match suffix {
        None => None,
        Some(raw)
            if raw.eq_ignore_ascii_case("x")
                || link_close_after_x(raw).is_some() =>
        {
            let display = format!("={raw}");
            let spec = match link_close_after_x(raw) {
                None => PomodoroCloseSpec::plain(display.clone()),
                Some(after_x) => {
                    match lex_close_selection(after_x, 0, &display) {
                        Ok(CloseSelectionOutcome::Valid(lex)) => {
                            close_spec_from_lex(display.clone(), &lex)
                        }
                        Ok(CloseSelectionOutcome::Incomplete(incomplete)) => {
                            return Err(close_selection_incomplete_error(
                                &display,
                                incomplete.separator,
                            ));
                        }
                        Err(error) => return Err(error.message),
                    }
                }
            };
            Some(SessionSuffix::Close(spec))
        }
        Some(raw) => {
            Some(SessionSuffix::Start(parse_pomodoro_start_suffix(raw)?))
        }
    };
    let (start, close) = match session {
        None => (None, None),
        Some(SessionSuffix::Start(spec)) => (Some(spec), None),
        Some(SessionSuffix::Close(spec)) => (None, Some(spec)),
    };
    let (block_id, pomodoro_name) = match rest_before_start.split_once('#') {
        Some((block_id, name)) => (block_id, Some(name)),
        None => (rest_before_start, None),
    };
    if block_id.is_empty() {
        return Err(if pomodoro_name.is_some() {
            format!(
                "Pomodoro capture requires a block ID before the Pomodoro name: `@<route>:<block-id>#<pomodoro>` (run `bob capture-tasks -r {}` to list task block IDs)",
                route.to_ascii_lowercase()
            )
        } else {
            POMODORO_BLOCK_ID_ERROR.to_string()
        });
    }
    if !is_block_id(block_id) {
        return Err(POMODORO_BLOCK_ID_ERROR.to_string());
    }
    let pomodoro_name = match pomodoro_name {
        None => None,
        Some("") => {
            return Err(POMODORO_NAME_REQUIRED_ERROR.to_string());
        }
        Some(name) if !is_pomodoro_selector_component(name) => {
            return Err(POMODORO_NAME_ERROR.to_string());
        }
        Some(name) => Some(name.to_string()),
    };

    if close.is_some() && pomodoro_name.is_some() {
        let name = pomodoro_name.as_deref().unwrap_or_default();
        return Err(format!(
            "`=x` always closes the running Pomodoro; remove `#{name}` (drop `=x` to link under a named Pomodoro instead)"
        ));
    }
    Ok(ColonLinkParts {
        block_id: block_id.to_string(),
        pomodoro_name,
        start,
        close,
    })
}

pub(super) fn parse_pomodoro_route_token(
    token: &str,
) -> Result<RouteToken, String> {
    let marker = token
        .strip_prefix("@!")
        .or_else(|| token.strip_prefix('@'))
        .ok_or_else(|| POMODORO_SHAPE_ERROR.to_string())?;
    let Some((route, rest)) = marker.split_once(':') else {
        return Err(POMODORO_SHAPE_ERROR.to_string());
    };
    if !is_route_token(route) {
        return Err(POMODORO_ROUTE_ERROR.to_string());
    }
    let parts = parse_colon_link_tail(token, route, rest, false)?;
    Ok(RouteToken {
        route: Some(route.to_ascii_lowercase()),
        kind: CaptureKind::Pomodoro {
            block_id: parts.block_id,
            pomodoro_name: parts.pomodoro_name,
            start: parts.start,
            close: parts.close,
        },
    })
}

pub(super) fn parse_pomodoro_start_suffix(
    raw: &str,
) -> Result<PomodoroStartSpec, String> {
    if raw.contains('=') || raw.contains('#') || raw.contains(':') {
        return Err(POMODORO_START_SHAPE_ERROR.to_string());
    }
    if raw.is_empty() {
        return Ok(PomodoroStartSpec {
            raw: raw.to_string(),
            duration_units: 5,
            offset_units: 0,
        });
    }
    if raw == "-" {
        return Ok(PomodoroStartSpec {
            raw: raw.to_string(),
            duration_units: 5,
            offset_units: 1,
        });
    }
    if let Some(after_dash) = raw.strip_prefix('-') {
        if after_dash.is_empty()
            || !after_dash.bytes().all(|byte| byte.is_ascii_digit())
        {
            return Err(POMODORO_START_SHAPE_ERROR.to_string());
        }
        let offset_units = after_dash
            .parse::<u64>()
            .map_err(|_| POMODORO_START_OVERFLOW_ERROR.to_string())?;
        return Ok(PomodoroStartSpec {
            raw: raw.to_string(),
            duration_units: 5,
            offset_units,
        });
    }
    if let Some((duration_text, offset_text)) = raw.split_once('-') {
        if duration_text.is_empty()
            || !duration_text.bytes().all(|byte| byte.is_ascii_digit())
            || offset_text.contains('-')
            || (!offset_text.is_empty()
                && !offset_text.bytes().all(|byte| byte.is_ascii_digit()))
        {
            return Err(POMODORO_START_SHAPE_ERROR.to_string());
        }
        let duration_units = duration_text
            .parse::<u64>()
            .map_err(|_| POMODORO_START_OVERFLOW_ERROR.to_string())?;
        let offset_units = if offset_text.is_empty() {
            1
        } else {
            offset_text
                .parse::<u64>()
                .map_err(|_| POMODORO_START_OVERFLOW_ERROR.to_string())?
        };
        return Ok(PomodoroStartSpec {
            raw: raw.to_string(),
            duration_units,
            offset_units,
        });
    }
    if !raw.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(POMODORO_START_SHAPE_ERROR.to_string());
    }
    let duration_units = raw
        .parse::<u64>()
        .map_err(|_| POMODORO_START_OVERFLOW_ERROR.to_string())?;
    Ok(PomodoroStartSpec {
        raw: raw.to_string(),
        duration_units,
        offset_units: 0,
    })
}

/// Whether a `^route:…` tail carries a close-shaped `=x…` suffix. A trailing
/// `!` on such a tail is a dangling close separator (an incomplete close),
/// not an explicit-toggle marker.
fn caret_tail_has_close_suffix(tail: &str) -> bool {
    let Some((_, raw_suffix)) = tail.split_once('=') else {
        return false;
    };
    raw_suffix.eq_ignore_ascii_case("x")
        || link_close_after_x(raw_suffix).is_some()
}

/// Parse a `^route:…` token's components with the shared post-sigil parser.
/// Returns the route plus the shared parts. `+` (project note) and trailing
/// `!` (explicit toggle) are rejected with caret-specific messages.
pub(super) fn parse_caret_link_token(
    token: &str,
) -> Result<(String, ColonLinkParts), String> {
    let marker = token
        .strip_prefix('^')
        .ok_or_else(|| POMODORO_LINK_SHAPE_ERROR.to_string())?;
    let Some((route, rest)) = marker.split_once(':') else {
        return Err(POMODORO_LINK_INCOMPLETE_ERROR.to_string());
    };
    if route.is_empty() || !is_route_token(route) {
        return Err(POMODORO_LINK_SHAPE_ERROR.to_string());
    }
    if rest.is_empty() {
        return Err(POMODORO_LINK_INCOMPLETE_ERROR.to_string());
    }
    if rest.ends_with('!') && !caret_tail_has_close_suffix(rest) {
        return Err(POMODORO_LINK_TOGGLE_ERROR.to_string());
    }
    match parse_colon_link_tail(token, route, rest, true) {
        Ok(parts) => Ok((route.to_ascii_lowercase(), parts)),
        Err(message) => {
            if message == POMODORO_NAME_REQUIRED_ERROR {
                return Err(POMODORO_LINK_NAME_INCOMPLETE_ERROR.to_string());
            }
            Err(message)
        }
    }
}

/// Editor reading of one leading `^...` token. Unlike execution this never
/// fails: partial shapes are mid-typing states, and near misses carry their
/// message for an `invalid_pomodoro_link` diagnostic. Token-relative offsets
/// (`link_end`, `name_range`, `start_offset`) are byte offsets into the
/// token text, so both `parse_editor_item` spans and completion replacement
/// ranges derive from one classification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum CaretTokenShape {
    /// Ordinary prose, never a link: a lookalike (`^_^`, `^^`, `^.`) or an
    /// invalid route prefix.
    Prose,
    /// A near miss (`^... +`, `^...!`, a bad component, a malformed start)
    /// with the message execution fails with.
    Invalid(String),
    /// Lone `^`, `^fragment`, or `^route:`: still typing the route:block-id
    /// part. The route is set once a valid `route:` prefix is typed.
    Partial { route: Option<String> },
    /// `^route:block-id#`: still typing the Pomodoro name.
    NamePartial { route: String, block_id: String },
    /// A complete `^route:block-id[#name][=<X>]` token. `link_end` ends the
    /// `route:block-id` part (before any `#`/`=`); `name_range` covers the
    /// name text after `#`; `start_offset` starts the `=<X>`/`=x` suffix.
    Complete {
        route: String,
        block_id: String,
        pomodoro_name: Option<String>,
        start: Option<PomodoroStartSpec>,
        close: Option<PomodoroCloseSpec>,
        link_end: usize,
        name_range: Option<(usize, usize)>,
        start_offset: Option<usize>,
        /// Token-relative ranges of the `<N>`, `!<M>`, and `~<K>` lists
        /// inside a selection-bearing close suffix, for the
        /// `pomodoro_close_in_progress`/`pomodoro_close_complete`/
        /// `pomodoro_close_drop` spans.
        close_in_progress: Option<(usize, usize)>,
        close_complete: Option<(usize, usize)>,
        close_drop: Option<(usize, usize)>,
    },
    /// A `^route:block-id=x…` token whose selection ends in a dangling
    /// separator: an editing state, not a mistake. `suffix_offset` starts
    /// the `=`, `separator_range` covers the dangling `,`/`!`/`~`, and the
    /// list ranges cover what was typed so far.
    CloseIncomplete {
        route: String,
        block_id: String,
        close: PomodoroCloseSpec,
        link_end: usize,
        suffix_offset: usize,
        close_in_progress: Option<(usize, usize)>,
        close_complete: Option<(usize, usize)>,
        close_drop: Option<(usize, usize)>,
        separator_range: (usize, usize),
    },
    /// A `^route:block-id=x…` token whose selection is lexically invalid,
    /// with the precise token-relative range of the failure.
    CloseInvalid {
        message: String,
        range: (usize, usize),
    },
}

pub(super) fn is_caret_word_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-')
}

pub(super) fn classify_caret_token(text: &str) -> CaretTokenShape {
    if text == "^" {
        return CaretTokenShape::Partial { route: None };
    }
    let Some(rest) = text.strip_prefix('^') else {
        return CaretTokenShape::Prose;
    };
    if rest.contains('^') {
        return CaretTokenShape::Prose;
    }
    if !rest.bytes().next().is_some_and(is_caret_word_byte) {
        return CaretTokenShape::Prose;
    }
    if !rest.contains(':') {
        if rest.bytes().all(is_caret_word_byte) {
            return CaretTokenShape::Partial { route: None };
        }
        return CaretTokenShape::Prose;
    }
    let (route_part, tail) = rest.split_once(':').expect("contains colon");
    if route_part.is_empty()
        || !is_route_token(route_part)
        || !route_part.bytes().any(|byte| byte.is_ascii_alphabetic())
    {
        return CaretTokenShape::Prose;
    }
    if tail.is_empty() {
        return CaretTokenShape::Partial {
            route: Some(route_part.to_ascii_lowercase()),
        };
    }
    if tail.ends_with('!') && !caret_tail_has_close_suffix(tail) {
        return CaretTokenShape::Invalid(
            POMODORO_LINK_TOGGLE_ERROR.to_string(),
        );
    }
    // A close-shaped `=` suffix (`x`, `x` plus lists) lexes through the
    // shared selection lexer with token-relative offsets, so the editor can
    // span the lists, report precise diagnostics, and surface the dangling
    // separator as an incomplete state. Every other suffix keeps today's
    // `=<X>` start handling below.
    if let Some((before_eq, raw_suffix)) = tail.split_once('=')
        && (raw_suffix.eq_ignore_ascii_case("x")
            || link_close_after_x(raw_suffix).is_some())
    {
        return classify_caret_close_suffix(route_part, before_eq, raw_suffix);
    }
    match parse_colon_link_tail(text, route_part, tail, true) {
        Ok(parts) => {
            let route = route_part.to_ascii_lowercase();
            let link_base = 1 + route_part.len() + 1;
            let (before_start, start_offset) = match tail.split_once('=') {
                Some((before, _)) => (before, Some(link_base + before.len())),
                None => (tail, None),
            };
            let (block_raw, name_raw) = match before_start.split_once('#') {
                Some((block, name)) => (block, Some(name)),
                None => (before_start, None),
            };
            let link_end = link_base + block_raw.len();
            let name_range = name_raw.map(|name| {
                let name_start = link_end + 1;
                (name_start, name_start + name.len())
            });
            CaretTokenShape::Complete {
                route,
                block_id: parts.block_id,
                pomodoro_name: parts.pomodoro_name,
                start: parts.start,
                close: parts.close,
                link_end,
                name_range,
                start_offset,
                close_in_progress: None,
                close_complete: None,
                close_drop: None,
            }
        }
        Err(message) => {
            if message == POMODORO_NAME_REQUIRED_ERROR {
                let block = tail
                    .split_once('#')
                    .map(|(block, _)| block)
                    .unwrap_or(tail);
                return CaretTokenShape::NamePartial {
                    route: route_part.to_ascii_lowercase(),
                    block_id: block.to_string(),
                };
            }
            CaretTokenShape::Invalid(message)
        }
    }
}

/// Editor reading of a `^route:block-id=x…` close suffix. The link part is
/// validated exactly like [`parse_colon_link_tail`] (so execution and the
/// editor agree on block/name failures) while the lists lex through the
/// shared selection lexer with token-relative offsets: a valid selection
/// reports `Complete` with span ranges, a dangling separator reports
/// `CloseIncomplete`, and a malformed list reports `CloseInvalid` with its
/// precise range.
fn classify_caret_close_suffix(
    route_part: &str,
    before_eq: &str,
    raw_suffix: &str,
) -> CaretTokenShape {
    let route = route_part.to_ascii_lowercase();
    let link_base = 1 + route_part.len() + 1;
    let (block_raw, name_raw) = match before_eq.split_once('#') {
        Some((block, name)) => (block, Some(name)),
        None => (before_eq, None),
    };
    let (block_id, project_note) = match block_raw.strip_suffix('+') {
        Some(stripped) => (stripped, true),
        None => (block_raw, false),
    };
    if project_note {
        return CaretTokenShape::Invalid(
            POMODORO_LINK_PROJECT_ERROR.to_string(),
        );
    }
    if block_id.is_empty() {
        return CaretTokenShape::Invalid(if name_raw.is_some() {
            format!(
                "Pomodoro capture requires a block ID before the Pomodoro name: `@<route>:<block-id>#<pomodoro>` (run `bob capture-tasks -r {route}` to list task block IDs)"
            )
        } else {
            POMODORO_BLOCK_ID_ERROR.to_string()
        });
    }
    if !is_block_id(block_id) {
        return CaretTokenShape::Invalid(POMODORO_BLOCK_ID_ERROR.to_string());
    }
    match name_raw {
        None => {}
        Some("") => {
            return CaretTokenShape::NamePartial {
                route,
                block_id: block_id.to_string(),
            };
        }
        Some(name) if !is_pomodoro_selector_component(name) => {
            return CaretTokenShape::Invalid(POMODORO_NAME_ERROR.to_string());
        }
        Some(name) => {
            return CaretTokenShape::Invalid(format!(
                "`=x` always closes the running Pomodoro; remove `#{name}` (drop `=x` to link under a named Pomodoro instead)"
            ));
        }
    }
    let suffix_offset = link_base + before_eq.len();
    let after_x_offset = suffix_offset + 2;
    let display = format!("={raw_suffix}");
    let after_x = link_close_after_x(raw_suffix).unwrap_or("");
    let link_end = link_base + block_id.len();
    match lex_close_selection(after_x, after_x_offset, &display) {
        Ok(CloseSelectionOutcome::Valid(lex)) => CaretTokenShape::Complete {
            route,
            block_id: block_id.to_string(),
            pomodoro_name: None,
            start: None,
            close: Some(close_spec_from_lex(display, &lex)),
            link_end,
            name_range: None,
            start_offset: Some(suffix_offset),
            close_in_progress: lex.in_progress_range,
            close_complete: lex.complete_range,
            close_drop: lex.drop_range,
        },
        Ok(CloseSelectionOutcome::Incomplete(incomplete)) => {
            CaretTokenShape::CloseIncomplete {
                route,
                block_id: block_id.to_string(),
                close: close_spec_from_incomplete(display, &incomplete),
                link_end,
                suffix_offset,
                close_in_progress: incomplete.in_progress_range,
                close_complete: incomplete.complete_range,
                close_drop: incomplete.drop_range,
                separator_range: incomplete.separator_range,
            }
        }
        Err(error) => CaretTokenShape::CloseInvalid {
            message: error.message,
            range: error.range,
        },
    }
}

pub(super) const POMODORO_LINK_CHILD_CONFLICT_ERROR: &str =
    "Pomodoro link capture cannot be combined with authored child bullets";

pub(super) const POMODORO_LINK_CLIP_CONFLICT_ERROR: &str =
    "Pomodoro link capture cannot be combined with % clipboard markers";

pub(super) const POMODORO_LINK_SCHEDULE_CONFLICT_ERROR: &str =
    "Pomodoro link capture cannot be combined with s:<N>";

pub(super) const POMODORO_LINK_PRIORITY_CONFLICT_ERROR: &str =
    "Pomodoro link capture cannot be combined with p:<N>";

/// Editor reading of an item whose parent line starts with `^`. Returns
/// `None` when the item stays on the ordinary path (no `^` first token, a
/// lookalike, or a partial shape with other text on the item, all of which
/// stay prose). Otherwise the `^` token claims the item: a complete shape
/// reports `pomodoro_link` (plus an `invalid_pomodoro_link` diagnostic when
/// anything else is on the item), partial shapes report `incomplete`, and
/// near misses report `pomodoro_link` with the diagnostic.
pub(super) struct CaretItem<'a> {
    pub(super) token: Token<'a>,
    pub(super) solo_parent: bool,
    pub(super) kind: CaretItemKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum CaretItemKind {
    Complete {
        route: String,
        block_id: String,
        pomodoro_name: Option<String>,
        start: Option<PomodoroStartSpec>,
        close: Option<PomodoroCloseSpec>,
        link_end: usize,
        name_range: Option<(usize, usize)>,
        start_offset: Option<usize>,
        close_in_progress: Option<(usize, usize)>,
        close_complete: Option<(usize, usize)>,
        close_drop: Option<(usize, usize)>,
        conflict: Option<String>,
    },
    /// A selection-bearing close suffix ending in a dangling separator.
    /// Ranges are token-relative; `conflict` mirrors `Complete`'s extra
    /// text/child-line conflicts (a conflict wins over the pending state).
    CloseIncomplete {
        route: String,
        block_id: String,
        close: PomodoroCloseSpec,
        link_end: usize,
        suffix_offset: usize,
        close_in_progress: Option<(usize, usize)>,
        close_complete: Option<(usize, usize)>,
        close_drop: Option<(usize, usize)>,
        separator_range: (usize, usize),
        conflict: Option<String>,
    },
    /// A selection-bearing close suffix that is lexically invalid, with the
    /// precise token-relative range of the failure. `conflict` mirrors
    /// `Complete`'s extra text/child-line conflicts (a conflict wins over
    /// the suffix failure, exactly like execution).
    CloseInvalid {
        message: String,
        range: (usize, usize),
        conflict: Option<String>,
    },
    Partial {
        route: Option<String>,
    },
    NamePartial {
        route: String,
        block_id: String,
    },
    Invalid {
        message: String,
    },
}

pub(super) fn classify_caret_item<'a>(
    item: &CaptureItem<'a>,
    parent_line: RawLine<'a>,
) -> Option<CaretItem<'a>> {
    let parent_tokens = tokenize_line_with_spans(&parent_line);
    let first = parent_tokens.first()?;
    if !first.text.starts_with('^') {
        return None;
    }
    let solo_parent = parent_tokens.len() == 1;
    let single_line = item.lines.len() == 1;
    match classify_caret_token(first.text) {
        CaretTokenShape::Prose => None,
        CaretTokenShape::Invalid(message) => Some(CaretItem {
            token: *first,
            solo_parent,
            kind: CaretItemKind::Invalid { message },
        }),
        CaretTokenShape::Partial { route } => {
            if solo_parent && single_line {
                Some(CaretItem {
                    token: *first,
                    solo_parent,
                    kind: CaretItemKind::Partial { route },
                })
            } else {
                None
            }
        }
        CaretTokenShape::NamePartial { route, block_id } => {
            if solo_parent && single_line {
                Some(CaretItem {
                    token: *first,
                    solo_parent,
                    kind: CaretItemKind::NamePartial { route, block_id },
                })
            } else {
                None
            }
        }
        CaretTokenShape::Complete {
            route,
            block_id,
            pomodoro_name,
            start,
            close,
            link_end,
            name_range,
            start_offset,
            close_in_progress,
            close_complete,
            close_drop,
        } => {
            let conflict =
                caret_item_conflict(&parent_tokens, solo_parent, single_line);
            Some(CaretItem {
                token: *first,
                solo_parent,
                kind: CaretItemKind::Complete {
                    route,
                    block_id,
                    pomodoro_name,
                    start,
                    close,
                    link_end,
                    name_range,
                    start_offset,
                    close_in_progress,
                    close_complete,
                    close_drop,
                    conflict,
                },
            })
        }
        CaretTokenShape::CloseIncomplete {
            route,
            block_id,
            close,
            link_end,
            suffix_offset,
            close_in_progress,
            close_complete,
            close_drop,
            separator_range,
        } => {
            let conflict =
                caret_item_conflict(&parent_tokens, solo_parent, single_line);
            Some(CaretItem {
                token: *first,
                solo_parent,
                kind: CaretItemKind::CloseIncomplete {
                    route,
                    block_id,
                    close,
                    link_end,
                    suffix_offset,
                    close_in_progress,
                    close_complete,
                    close_drop,
                    separator_range,
                    conflict,
                },
            })
        }
        CaretTokenShape::CloseInvalid { message, range } => {
            let conflict =
                caret_item_conflict(&parent_tokens, solo_parent, single_line);
            Some(CaretItem {
                token: *first,
                solo_parent,
                kind: CaretItemKind::CloseInvalid {
                    message,
                    range,
                    conflict,
                },
            })
        }
    }
}

/// Extra-text and child-line conflicts for a claimed `^` item: `None` for an
/// exact solo link, the marker conflict for extra parent-line tokens, and
/// the child conflict for authored children. Shared by every complete and
/// close-suffix shape so the editor agrees with execution on which failure
/// wins.
fn caret_item_conflict(
    parent_tokens: &[Token<'_>],
    solo_parent: bool,
    single_line: bool,
) -> Option<String> {
    if solo_parent && single_line {
        None
    } else if !solo_parent {
        parent_tokens[1..]
            .iter()
            .find_map(|extra| {
                if parse_schedule_token(extra.text).is_some() {
                    Some(POMODORO_LINK_SCHEDULE_CONFLICT_ERROR.to_string())
                } else if parse_priority_token(extra.text).is_some() {
                    Some(POMODORO_LINK_PRIORITY_CONFLICT_ERROR.to_string())
                } else if extra.text.starts_with('%') {
                    Some(POMODORO_LINK_CLIP_CONFLICT_ERROR.to_string())
                } else {
                    None
                }
            })
            .or_else(|| Some(POMODORO_LINK_SHAPE_ERROR.to_string()))
    } else {
        Some(POMODORO_LINK_CHILD_CONFLICT_ERROR.to_string())
    }
}

/// Whole-item `^route:block-id[#pomodoro][=<X>]` grammar.
///
/// Returns `Ok(None)` when the item does not start with `^` and ordinary
/// parsing should continue. Returns `Ok(Some(outcome))` for an exact solo
/// link. Returns `Err` for complete-shape near misses, partial solo shapes,
/// and solo conflicts. Partial shapes followed by other text, and lookalikes
/// (`^_^`, `^^`, `^.`, …), stay ordinary prose (`Ok(None)`).
pub(super) fn parse_pomodoro_link_item<'a>(
    item: &CaptureItem<'a>,
    parent_line: &ItemLine<'a>,
    forced_route: Option<&str>,
    forced_section: Option<&str>,
) -> Result<Option<ParsedCaptureItemOutcome<'a>>, String> {
    let parent_trimmed = parent_line.raw.text.trim();
    if !parent_trimmed.starts_with('^') {
        return Ok(None);
    }
    let tokens: Vec<&str> = parent_trimmed.split_whitespace().collect();
    let Some(first) = tokens.first().copied() else {
        return Ok(None);
    };
    if !first.starts_with('^') {
        return Ok(None);
    }
    if first == "^" {
        if tokens.len() == 1 && item.lines.len() == 1 {
            return Err(POMODORO_LINK_INCOMPLETE_ERROR.to_string());
        }
        return Ok(None);
    }
    let rest = &first[1..];
    if rest.contains('^') {
        return Ok(None);
    }
    let second = rest.as_bytes().first().copied().unwrap_or(0);
    if !(second.is_ascii_alphanumeric() || matches!(second, b'_' | b'-')) {
        return Ok(None);
    }
    if !rest.contains(':') {
        if !rest
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
        {
            return Ok(None);
        }
        if tokens.len() == 1 && item.lines.len() == 1 {
            return Err(POMODORO_LINK_INCOMPLETE_ERROR.to_string());
        }
        return Ok(None);
    }
    let (route_part, tail) = rest.split_once(':').expect("contains colon");
    if route_part.is_empty()
        || !is_route_token(route_part)
        || !route_part.bytes().any(|b| b.is_ascii_alphabetic())
    {
        return Ok(None);
    }
    if tail.is_empty() {
        if tokens.len() == 1 && item.lines.len() == 1 {
            return Err(POMODORO_LINK_INCOMPLETE_ERROR.to_string());
        }
        return Ok(None);
    }
    let is_solo_parent = tokens.len() == 1;
    let is_solo_item = is_solo_parent && item.lines.len() == 1;
    match parse_caret_link_token(first) {
        Ok((route, parts)) => {
            if forced_route.is_some() || forced_section.is_some() {
                return Err(
                    "Pomodoro link capture cannot be combined with --route, --section, --task, --task-ref, --task-section, or --clip; capture the link alone".to_string(),
                );
            }
            if !is_solo_item {
                if !is_solo_parent {
                    // A solo link with nothing but the weekly-bet tag has no
                    // new task text to tag.
                    if item.lines.len() == 1
                        && tokens.len() == 2
                        && is_now_tag(tokens[1])
                    {
                        return Err(now_tag_body_error());
                    }
                    for extra in &tokens[1..] {
                        if parse_schedule_token(extra).is_some() {
                            return Err(
                                "Pomodoro link capture cannot be combined with s:<N>".to_string(),
                            );
                        }
                        if parse_priority_token(extra).is_some() {
                            return Err(
                                "Pomodoro link capture cannot be combined with p:<N>".to_string(),
                            );
                        }
                        if extra.starts_with('%') {
                            return Err(
                                "Pomodoro link capture cannot be combined with % clipboard markers"
                                    .to_string(),
                            );
                        }
                    }
                    return Err(POMODORO_LINK_SHAPE_ERROR.to_string());
                }
                return Err(
                    "Pomodoro link capture cannot be combined with authored child bullets"
                        .to_string(),
                );
            }
            let marker_text = first.to_string();
            return Ok(Some(parsed_capture_item_outcome(
                item,
                ParsedCaptureText {
                    body: String::new(),
                    clip: None,
                    route: Some(route),
                    kind: CaptureKind::PomodoroLink {
                        block_id: parts.block_id,
                        pomodoro_name: parts.pomodoro_name,
                        start: parts.start,
                        close: parts.close,
                        spelling: PomodoroLinkSpelling::Caret,
                    },
                    scheduled_offset: None,
                    priority_level: None,
                    sub_bullets: Vec::new(),
                },
                Vec::new(),
                Some(marker_text),
            )));
        }
        Err(message) => {
            if message == POMODORO_LINK_PROJECT_ERROR
                || message == POMODORO_LINK_TOGGLE_ERROR
            {
                return Err(message);
            }
            if message == POMODORO_LINK_NAME_INCOMPLETE_ERROR {
                if is_solo_item {
                    return Err(message);
                }
                return Ok(None);
            }
            if message == POMODORO_LINK_INCOMPLETE_ERROR {
                if is_solo_item {
                    return Err(message);
                }
                return Ok(None);
            }
            if is_solo_item {
                return Err(message);
            }
            // A close-shaped suffix with an item conflict reports the
            // conflict exactly like the editor: a conflict wins over the
            // lexical close diagnostic.
            let close_shaped = first.contains("=x") || first.contains("=X");
            if close_shaped && !is_solo_item {
                if !is_solo_parent {
                    for extra in &tokens[1..] {
                        if parse_schedule_token(extra).is_some() {
                            return Err(
                                "Pomodoro link capture cannot be combined with s:<N>"
                                    .to_string(),
                            );
                        }
                        if parse_priority_token(extra).is_some() {
                            return Err(
                                "Pomodoro link capture cannot be combined with p:<N>"
                                    .to_string(),
                            );
                        }
                        if extra.starts_with('%') {
                            return Err(
                                "Pomodoro link capture cannot be combined with % clipboard markers"
                                    .to_string(),
                            );
                        }
                    }
                    // Extra text without a marker conflict still reports the
                    // shape error, matching the editor's conflict.
                    return Err(POMODORO_LINK_SHAPE_ERROR.to_string());
                }
                return Err(
                    "Pomodoro link capture cannot be combined with authored child bullets"
                        .to_string(),
                );
            }
            if tail.ends_with('#') {
                return Ok(None);
            }
            return Err(POMODORO_LINK_SHAPE_ERROR.to_string());
        }
    }
}

/// Route-less retired project-note shape (`@:<block-id>+`): the editor
/// teaches the `^` replacement, and execution must agree. `@:` and `@:<id>`
/// stay literal so interactive route-picking still diverges.
pub(super) fn retired_route_less_project_note_error(
    token: &str,
) -> Option<String> {
    let rest = token.strip_prefix("@:")?;
    if rest.is_empty() {
        return None;
    }
    let before_eq = rest
        .split_once('=')
        .map(|(before, _)| before)
        .unwrap_or(rest);
    let (block_probe, name_probe) = match before_eq.split_once('#') {
        Some((block, name)) => (block, Some(name)),
        None => (before_eq, None),
    };
    let stripped = block_probe.strip_suffix('+')?;
    if stripped.is_empty() {
        return None;
    }
    Some(retired_project_note_marker_error(
        token, "", stripped, name_probe,
    ))
}

/// Return whether a terminal token belongs to the Pomodoro-marker grammar.
/// A colon that follows `#` remains part of an ordinary bullet section prefix.
pub(super) fn is_pomodoro_marker_candidate(token: &str) -> bool {
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
pub(super) fn validate_special_terminal_markers_line(
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
        if let Some(message) = retired_route_less_project_note_error(token) {
            return Err(message);
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
