//! Route, caret, and selector parsers.

use super::bare_name::{start_name_spelling, NameSpelling};
use super::close_selection::*;
use super::dependencies::*;
use super::draft::*;
use super::editor_parse::*;
use super::item::*;
use super::markers::*;
use super::model::*;
use super::pomodoro_literal::{
    canonicalize_exact_name, expand_literal_token, lex_exact_name_after_hash,
    opens_exact_name,
};
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
    // A `#"NAME"` literal never names a task section: it names the
    // Pomodoro directly, so resolve to a task toggle now. (A literal
    // always errored here before, so nothing that parsed changes shape.)
    if let Some(selector) = section
        && opens_exact_name(selector)
    {
        let lex = lex_exact_name_after_hash(selector, selector.len())
            .filter(|lex| lex.complete)
            .ok_or_else(|| SUB_BULLET_SECTION_ERROR.to_string())?;
        if selector.len() != lex.literal_len {
            return Err(SUB_BULLET_SECTION_ERROR.to_string());
        }
        let canonical = canonicalize_exact_name(&lex.decoded)
            .ok_or_else(|| SUB_BULLET_SECTION_ERROR.to_string())?;
        return Ok(RouteToken {
            route: Some(route.to_ascii_lowercase()),
            kind: CaptureKind::TaskToggle {
                block_id: block_id.to_string(),
                pomodoro_name: Some(canonical),
                pomodoro_name_is_literal: true,
                intent: TaskToggleIntent::EnsureNext,
            },
        });
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
                pomodoro_name_is_literal: false,
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

/// A task-link picker query claim: a single-line item whose parent line is
/// one whitespace-delimited token starting with `:`. The returned token
/// covers the whole query including the sigil. Execution, the editor, and
/// completion all call this one predicate; nothing else re-derives the rule.
pub(super) fn task_link_query_token<'a>(
    item: &CaptureItem<'a>,
) -> Option<Token<'a>> {
    let [line] = item.lines.as_slice() else {
        return None;
    };
    let tokens = tokenize_line_with_spans(&line.raw);
    let [token] = tokens.as_slice() else {
        return None;
    };
    token.text.starts_with(':').then_some(*token)
}

/// A whole-draft `/query` Go To claim: a single-line, single-token item
/// whose token starts with `/`, holds no second `/`, and is the whole
/// draft (`item.source` trimmed). The returned token covers the whole
/// query including the sigil. Execution, the editor parse, and completion
/// all call this one predicate; nothing else re-derives the rule.
pub(super) fn task_goto_query_token<'a>(
    item: &CaptureItem<'a>,
) -> Option<Token<'a>> {
    let [line] = item.lines.as_slice() else {
        return None;
    };
    let tokens = tokenize_line_with_spans(&line.raw);
    let [token] = tokens.as_slice() else {
        return None;
    };
    if !token.text.starts_with('/') {
        return None;
    }
    if token
        .text
        .as_bytes()
        .iter()
        .filter(|byte| **byte == b'/')
        .count()
        != 1
    {
        return None;
    }
    (item.source.trim() == token.text).then_some(*token)
}

/// One whole-item `!` token claim, shared by execution, the editor parse,
/// and completion. A query is still being typed (`!`, `!fix`, `!sase:`);
/// a complete token names a task; anything extra on the item is a
/// teaching error, never an inbox task.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum BangClaim {
    Query {
        raw: String,
        start: usize,
        end: usize,
    },
    Complete(DependencyRef),
    Invalid(InvalidDependency),
}

/// Claim predicate for whole-item `!note:block-id` tokens. A leading `!`
/// claims the item unless the second byte is `[` or `!` (embeds, images,
/// and `!` runs stay prose). An incomplete token with other text on the
/// item, or on a multi-line item, stays prose like `:dee` plus a child;
/// a complete token with extra text or child lines is claimed invalid.
pub(crate) fn claim_bang_item(item: &CaptureItem<'_>) -> Option<BangClaim> {
    let (parent, children) = item.lines.split_first()?;
    let text = parent.raw.text;
    let trimmed = text.trim();
    if !trimmed.starts_with('!') {
        return None;
    }
    if matches!(trimmed.as_bytes().get(1).copied(), Some(b'[') | Some(b'!')) {
        return None;
    }
    let base = parent.raw.start + (text.len() - text.trim_start().len());
    let has_children =
        children.iter().any(|line| !line.raw.text.trim().is_empty());
    match scan_bang_token(trimmed, base) {
        ScannedDependency::Complete(dependency) => {
            let token_len = dependency.end - base;
            let rest = trimmed[token_len..].trim();
            if rest.is_empty() {
                if has_children {
                    return Some(BangClaim::Invalid(InvalidDependency {
                        raw: dependency.raw.clone(),
                        message: bang_invalid_error(
                            &dependency.raw,
                            "its child lines",
                        ),
                        start: dependency.start,
                        end: dependency.end,
                    }));
                }
                return Some(BangClaim::Complete(dependency));
            }
            let extra = rest.to_string();
            let hinted =
                rest.split_whitespace().any(|token| token.starts_with('!'));
            let mut message =
                bang_invalid_error(&dependency.raw, &format!("`{extra}`"));
            if hinted {
                message.push_str(&format!(" ({})", BANG_BULK_HINT));
            }
            Some(BangClaim::Invalid(InvalidDependency {
                raw: dependency.raw.clone(),
                message,
                start: dependency.start,
                end: base + trimmed.len(),
            }))
        }
        ScannedDependency::Partial { raw, start, end } => {
            if end - base == trimmed.len() && !has_children {
                Some(BangClaim::Query { raw, start, end })
            } else {
                // `!wow this works`, `! foo`, or an incomplete token plus
                // a child line: ordinary prose.
                None
            }
        }
        ScannedDependency::Invalid(invalid) => {
            let mut message = invalid.message.clone();
            if let Some(tail) = invalid.raw.split_once(':').map(|(_, t)| t)
                && (tail.contains('=') || tail.contains('#'))
            {
                message.push_str(&format!(" ({BANG_SUFFIX_HINT})"));
            }
            Some(BangClaim::Invalid(InvalidDependency {
                raw: invalid.raw.clone(),
                message: bang_invalid_wrapped(&invalid.raw, &message),
                start: invalid.start,
                end: invalid.end,
            }))
        }
        ScannedDependency::Escaped { .. } => None,
    }
}

/// Wrap a malformed single-token message in the whole-item teaching frame.
fn bang_invalid_wrapped(raw: &str, detail: &str) -> String {
    format!(
        "`{raw}` completes an existing task and must be the whole capture item; {detail}"
    )
}

/// One eligible bare-plus task selector on an item's parent line or an
/// authored child line. The token is the exact `+query` span; offsets are
/// draft-global UTF-8 bytes. A lone `+` on a one-line parent item is also a
/// valid Pomodoro adjustment and is marked for the caller to preserve.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct ParentTaskSelectorToken<'a> {
    pub(super) token: Token<'a>,
    pub(super) line_index: usize,
    pub(super) dual_use_adjustment: bool,
}

/// Shared lexical claim for bare-plus task selectors. A selector is the
/// final remaining whitespace-delimited token on the parent line, or on an
/// eligible authored child line, after relationship modifiers leave.
/// Numeric and double-plus tokens remain Pomodoro operators; protected
/// Markdown, global declarations, and Work Log text are excluded by the
/// shared capture/link paths.
pub(super) fn parent_task_selector_tokens<'a>(
    item: &CaptureItem<'a>,
) -> Vec<ParentTaskSelectorToken<'a>> {
    let mut selectors = Vec::new();
    for (line_index, item_line) in item.lines.iter().enumerate() {
        let line = item_line.raw;
        let scan_line = if line_index == 0 {
            line
        } else {
            let AuthoredLineClass::Item(authored) =
                classify_authored_line(line)
            else {
                continue;
            };
            if authored.depth == AuthoredDepth::Nested
                && !item.lines[1..line_index].iter().any(|previous| {
                    matches!(
                        classify_authored_line(previous.raw),
                        AuthoredLineClass::Item(found)
                            if found.depth == AuthoredDepth::First
                    )
                })
            {
                continue;
            }
            RawLine {
                text: authored.body,
                start: authored.body_start,
                end: line.end,
            }
        };

        let whole_line_tokens = tokenize_line_with_spans(&line);
        // A global declaration is a separate grammar. Do not reinterpret
        // any plus-looking terminal token on a declaration line.
        if whole_line_tokens
            .iter()
            .any(|token| token.text.starts_with("@@"))
        {
            continue;
        }

        let tokens = tokenize_line_with_spans(&scan_line);
        // Relationship modifiers leave before the selector claim, so
        // `+ &foo:bar` and `&foo:bar +` share one remaining `+` token.
        let tokens = extract_line_dependencies(
            tokens,
            scan_line.text,
            scan_line.start,
            line_index == 0,
            true,
        )
        .0;
        let Some(token) = tokens.last().copied() else {
            continue;
        };
        let Some(query) = token.text.strip_prefix('+') else {
            continue;
        };
        // Operator runs and numeric prefixes still belong to the
        // adjustment/shift grammar. A typed query such as `+bank` is a
        // picker seed; `+-` remains ordinary prose like other mixed runs.
        if query.starts_with('+')
            || query.starts_with('-')
            || query
                .as_bytes()
                .first()
                .is_some_and(|byte| byte.is_ascii_digit())
            || crate::native::capture_links::is_protected_markup_range(
                item.source,
                token.start,
                token.end,
            )
        {
            continue;
        }

        let dual_use_adjustment =
            line_index == 0 && item.lines.len() == 1 && line.text.trim() == "+";
        selectors.push(ParentTaskSelectorToken {
            token,
            line_index,
            dual_use_adjustment,
        });
    }
    selectors
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
        if opens_exact_name(name) {
            // `#"NAME"` exact-name literal: runs through the closing
            // quote; unterminated refuses like a missing name.
            let Some(lex) = lex_exact_name_after_hash(name, name.len()) else {
                return Err(POMODORO_NAME_ERROR.to_string());
            };
            if !lex.complete {
                return Err(project_note_name_required_error(
                    route, block_core,
                ));
            }
            let canonical = canonicalize_exact_name(&lex.decoded)
                .ok_or_else(|| POMODORO_NAME_ERROR.to_string())?;
            if name.len() != lex.literal_len {
                return Err(POMODORO_NAME_ERROR.to_string());
            }
            return Ok(RouteToken {
                route: Some(route.to_ascii_lowercase()),
                kind: CaptureKind::ProjectNote {
                    block_id: block_core.to_string(),
                    pomodoro_name: Some(canonical),
                    pomodoro_name_is_literal: true,
                },
            });
        }
        if !is_pomodoro_selector_component(name) {
            return Err(POMODORO_NAME_ERROR.to_string());
        }
        return Ok(RouteToken {
            route: Some(route.to_ascii_lowercase()),
            kind: CaptureKind::ProjectNote {
                block_id: block_core.to_string(),
                pomodoro_name: Some(name.to_string()),
                pomodoro_name_is_literal: false,
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
                pomodoro_name_is_literal: false,
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
    pub(super) pomodoro_name_is_literal: bool,
    pub(super) start: Option<PomodoroStartSpec>,
    pub(super) close: Option<PomodoroCloseSpec>,
    /// Token-relative end of the route:block-id portion.
    pub(super) link_end: usize,
    /// Token-relative name span, regardless of whether it appeared before
    /// or after the `=` suffix.
    pub(super) name_range: Option<(usize, usize)>,
    /// Token-relative start of the `=` suffix.
    pub(super) start_offset: Option<usize>,
    /// True when the suffix name is written as bare `=name` rather than
    /// `=#name` / `=3#name`.
    pub(super) bare_name: bool,
    /// The start operator's disjoint editor spans. A name in the suffix can
    /// split the start into a prefix (`=3`) and trailing timing (`@3`).
    pub(super) start_ranges: Vec<(usize, usize)>,
}

struct ParsedLinkStart {
    spec: PomodoroStartSpec,
    name: Option<String>,
    name_range: Option<(usize, usize)>,
    bare_name: bool,
    name_is_literal: bool,
    start_ranges: Vec<(usize, usize)>,
}

/// Parse the text following a link's `=` through the same whole-item start
/// lexer used by standalone `=name` starts. Link starts share its timing and
/// name spellings, while dropping from a link remains disallowed.
fn parse_link_start_suffix(
    typed_token: &str,
    raw: &str,
    equals_offset: usize,
) -> Result<ParsedLinkStart, String> {
    let start_token = format!("={raw}");
    let Some(EqualsToken::Start {
        suffix,
        len,
        name,
        name_spelling,
        name_is_literal,
        drop,
        timing,
        doubled,
        ..
    }) = session_equals_token(&start_token)
    else {
        return Err(POMODORO_START_SHAPE_ERROR.to_string());
    };
    if doubled {
        return Err(POMODORO_START_SHAPE_ERROR.to_string());
    }
    if len != start_token.len() {
        if let Some((message, _)) =
            glued_timing_diagnostic(&start_token, len, &suffix)
        {
            return Err(message);
        }
        return Err(POMODORO_START_SHAPE_ERROR.to_string());
    }
    if name.as_deref() == Some("") {
        return Err(POMODORO_NAME_REQUIRED_ERROR.to_string());
    }
    if name_is_literal {
        // A literal's spaces are the point: validate the canonical name,
        // never the slug charset.
        if name
            .as_deref()
            .is_none_or(|name| canonicalize_exact_name(name).is_none())
        {
            return Err(POMODORO_NAME_ERROR.to_string());
        }
    } else if name
        .as_deref()
        .is_some_and(|name| !is_pomodoro_selector_component(name))
    {
        return Err(POMODORO_NAME_ERROR.to_string());
    }

    let effective = effective_start_timing(&suffix, &timing);
    // Overflow and invalid timing win over duplicate/dangling checks,
    // mirroring `parse_pomodoro_equals_item`'s `named_token_error` order.
    let spec = parse_pomodoro_start_suffix(&effective).map_err(|message| {
        // A glued timing (`=3bugs`) already returned above via the length
        // check; any remaining effective failure is overflow/invalid timing.
        message
    })?;
    // Both timings win over the dangling-`@` incomplete state, including a
    // dangling second timing (`=3#bugs@`). Reuse the standalone teaching
    // rule rather than drifting.
    if !suffix.is_empty()
        && let Some((_, timing_text)) = timing.as_ref()
    {
        let effective_twice = if !timing_text.is_empty() {
            timing_text.as_str()
        } else {
            suffix.as_str()
        };
        if let Some(name_str) = name.as_deref() {
            return Err(start_timing_twice_error(
                typed_token,
                "=",
                effective_twice,
                name_str,
            ));
        }
        return Err(start_timing_twice_unnamed_error(
            typed_token,
            "=",
            effective_twice,
            "",
        ));
    }
    // Dangling `@` never produces an executable default spec.
    if let Some((_, timing_text)) = timing.as_ref()
        && timing_text.is_empty()
    {
        return Err(start_timing_incomplete_error(typed_token));
    }
    if let Some((_, list)) = drop {
        return Err(format!(
            "Pomodoro link start `{typed_token}` takes a name and timing, not a `~` drop list; remove `~{list}`"
        ));
    }
    // A literal's span covers its quotes: recompute the typed length
    // rather than the canonical name's.
    let literal_typed_len: Option<usize> = if name_is_literal {
        raw.find('#').and_then(|hash| {
            lex_exact_name_after_hash(&raw[hash + 1..], raw.len() - hash - 1)
                .filter(|lex| lex.complete)
                .map(|lex| lex.literal_len)
        })
    } else {
        None
    };
    let name_range =
        name.as_ref().zip(name_spelling).map(|(name, spelling)| {
            let start =
                equals_offset + named_start_name_offset(suffix.len(), spelling);
            let len = literal_typed_len.unwrap_or(name.len());
            (start, start + len)
        });
    let mut start_ranges = Vec::new();
    if let Some((name_start, name_end)) = name_range {
        // A hash delimiter remains unpainted, as it does in the existing
        // `#name=3` spelling. The `=` and timing before it belong to the
        // start span; timing after the name gets its own start span.
        let name_spelling = name_spelling.expect("named start spelling");
        let prefix_end = match name_spelling {
            NameSpelling::Hash => name_start - 1,
            NameSpelling::Bare => name_start,
        };
        if equals_offset < prefix_end {
            start_ranges.push((equals_offset, prefix_end));
        }
        if name_end < equals_offset + start_token.len() {
            start_ranges.push((name_end, equals_offset + start_token.len()));
        }
    } else if equals_offset < equals_offset + start_token.len() {
        start_ranges.push((equals_offset, equals_offset + start_token.len()));
    }
    Ok(ParsedLinkStart {
        spec,
        name,
        name_range,
        bare_name: name_spelling == Some(NameSpelling::Bare),
        name_is_literal,
        start_ranges,
    })
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
    // A `=` inside a `#"…"` literal never separates: find the literal span
    // first and split outside it.
    let literal_span: Option<(usize, usize)> =
        rest.find('#').and_then(|hash| {
            let after_hash = &rest[hash + 1..];
            if !opens_exact_name(after_hash) {
                return None;
            }
            lex_exact_name_after_hash(after_hash, after_hash.len()).and_then(
                |lex| {
                    lex.complete.then(|| (hash + 1, hash + 1 + lex.literal_len))
                },
            )
        });
    let split_at = match literal_span {
        Some((start, end)) => rest[..start]
            .find('=')
            .or_else(|| rest[end..].find('=').map(|offset| end + offset)),
        None => rest.find('='),
    };
    let (rest_before_start, suffix) = match split_at {
        Some(index) => (&rest[..index], Some(&rest[index + 1..])),
        None => (rest, None),
    };
    let sigil_len = if typed_token.starts_with("@!") { 2 } else { 1 };
    let link_base = sigil_len + route.len() + 1;
    let equals_offset = suffix.map(|_| link_base + rest_before_start.len());
    let mut parsed_suffix = None;
    let session: Option<SessionSuffix> = match suffix {
        None => None,
        Some(raw)
            if raw.eq_ignore_ascii_case("x")
                || link_close_after_x(raw).is_some() =>
        {
            let display = format!("={raw}");
            if let Some(rewrite) = retired_link_alias_rewrite(raw) {
                return Err(retired_link_close_alias_error(&display, &rewrite));
            }
            let spec = match classify_link_close(raw) {
                None => PomodoroCloseSpec::plain(display.clone()),
                Some((after_x, x_len)) if after_x.is_empty() => {
                    debug_assert_eq!(x_len, 1);
                    PomodoroCloseSpec::plain(display.clone())
                }
                Some((after_x, x_len)) => {
                    let base = 1 + x_len;
                    match lex_close_selection(after_x, base, &display) {
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
            let parsed = parse_link_start_suffix(
                typed_token,
                raw,
                equals_offset.expect("suffix has an equals offset"),
            )?;
            let spec = parsed.spec.clone();
            parsed_suffix = Some(parsed);
            Some(SessionSuffix::Start(spec))
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
    let mut prefix_is_literal = false;
    let mut prefix_literal_len: Option<usize> = None;
    let prefix_name = match pomodoro_name {
        None => None,
        Some("") => {
            return Err(POMODORO_NAME_REQUIRED_ERROR.to_string());
        }
        Some(name) if opens_exact_name(name) => {
            // `#"NAME"` exact-name literal: runs through the closing
            // quote; unterminated refuses like a missing name.
            prefix_is_literal = true;
            let lex = lex_exact_name_after_hash(name, name.len())
                .filter(|lex| lex.complete)
                .ok_or_else(|| POMODORO_NAME_REQUIRED_ERROR.to_string())?;
            if name.len() != lex.literal_len {
                return Err(POMODORO_NAME_ERROR.to_string());
            }
            prefix_literal_len = Some(lex.literal_len);
            Some(
                canonicalize_exact_name(&lex.decoded)
                    .ok_or_else(|| POMODORO_NAME_ERROR.to_string())?,
            )
        }
        Some(name) if !is_pomodoro_selector_component(name) => {
            return Err(POMODORO_NAME_ERROR.to_string());
        }
        Some(name) => Some(name.to_string()),
    };

    if prefix_name.is_some()
        && parsed_suffix
            .as_ref()
            .and_then(|parsed| parsed.name.as_ref())
            .is_some()
    {
        let suffix_name = parsed_suffix
            .as_ref()
            .and_then(|parsed| parsed.name.as_deref())
            .unwrap_or_default();
        let sigil = if typed_token.starts_with('^') {
            "^"
        } else if typed_token.starts_with("@!") {
            "@!"
        } else {
            "@"
        };
        return Err(format!(
            "`{typed_token}` names the Pomodoro both before and after `=`; name it once, for example `{sigil}{route}:{block_id}={}` or `{sigil}{route}:{block_id}#{}=`",
            start_name_spelling(suffix_name),
            prefix_name.as_deref().unwrap_or_default(),
        ));
    }
    let suffix_name = parsed_suffix
        .as_ref()
        .and_then(|parsed| parsed.name.clone());
    let pomodoro_name = prefix_name.or(suffix_name);

    if close.is_some() && pomodoro_name.is_some() {
        let name = pomodoro_name.as_deref().unwrap_or_default();
        return Err(format!(
            "`=x` always closes the running Pomodoro; remove `#{name}` (drop `=x` to link under a named Pomodoro instead)"
        ));
    }
    // A literal's span covers its quotes: use the typed length rather
    // than the canonical name's.
    let prefix_typed_len: Option<usize> = if prefix_is_literal {
        prefix_literal_len
    } else {
        None
    };
    let prefix_name_range = pomodoro_name
        .as_ref()
        .filter(|_| {
            parsed_suffix
                .as_ref()
                .and_then(|parsed| parsed.name.as_ref())
                .is_none()
        })
        .map(|name| {
            let name_start = link_base + block_id.len() + 1;
            let len = prefix_typed_len.unwrap_or(name.len());
            (name_start, name_start + len)
        });
    let name_range = parsed_suffix
        .as_ref()
        .and_then(|parsed| parsed.name_range)
        .or(prefix_name_range);
    let start_ranges = parsed_suffix
        .as_ref()
        .map(|parsed| parsed.start_ranges.clone())
        .unwrap_or_else(|| {
            equals_offset
                .map(|offset| vec![(offset, typed_token.len())])
                .unwrap_or_default()
        });
    Ok(ColonLinkParts {
        block_id: block_id.to_string(),
        pomodoro_name,
        pomodoro_name_is_literal: prefix_is_literal
            || parsed_suffix
                .as_ref()
                .is_some_and(|parsed| parsed.name_is_literal),
        start,
        close,
        link_end: link_base + block_id.len(),
        name_range,
        start_offset: equals_offset,
        bare_name: parsed_suffix
            .as_ref()
            .is_some_and(|parsed| parsed.bare_name),
        start_ranges,
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
            pomodoro_name_is_literal: parts.pomodoro_name_is_literal,
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
            drop: Vec::new(),
            duration_units: 5,
            offset_units: 0,
        });
    }
    if raw == "-" {
        return Ok(PomodoroStartSpec {
            raw: raw.to_string(),
            drop: Vec::new(),
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
            drop: Vec::new(),
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
            drop: Vec::new(),
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
        drop: Vec::new(),
    })
}

/// Whether a `^route:…` tail carries a close-shaped suffix (`=x…`, or the
/// retired `=*…`/`=!…` aliases, which report a teaching error). A trailing
/// `!` inside such a suffix is a complete-all group (a valid close), not an
/// explicit-toggle marker.
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
    NamePartial {
        route: String,
        block_id: String,
        link_end: usize,
        name_range: (usize, usize),
        start_offset: Option<usize>,
        start_ranges: Vec<(usize, usize)>,
        start: Option<PomodoroStartSpec>,
    },
    /// A `^route:block-id=bugs@` token with a dangling `@`: an editing
    /// state, not a mistake. The typed name stays visible, no executable
    /// spec is produced, and one placeholder covers the `@`.
    TimingIncomplete {
        route: String,
        block_id: String,
        pomodoro_name: String,
        link_end: usize,
        name_range: (usize, usize),
        start_offset: usize,
        start_ranges: Vec<(usize, usize)>,
        separator_range: (usize, usize),
    },
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
        bare_name: bool,
        start_ranges: Vec<(usize, usize)>,
        /// Token-relative ranges of the `<N>`, `*<P>`, `!<M>`, and `~<K>`
        /// lists inside a selection-bearing close suffix, for the
        /// `pomodoro_close_in_progress`/`pomodoro_close_park`/
        /// `pomodoro_close_complete`/`pomodoro_close_drop` spans.
        close_in_progress: Option<(usize, usize)>,
        close_park: Option<(usize, usize)>,
        close_complete: Option<(usize, usize)>,
        close_drop: Option<(usize, usize)>,
    },
    /// A `^route:block-id=x…` token whose selection ends in a dangling
    /// separator: an editing state, not a mistake. `suffix_offset` starts
    /// the `=`, `separator_range` covers the dangling `,`/`~`, and
    /// the list ranges cover what was typed so far. A trailing `*`/`!`
    /// means all remaining numbered links instead of dangling.
    CloseIncomplete {
        route: String,
        block_id: String,
        close: PomodoroCloseSpec,
        link_end: usize,
        suffix_offset: usize,
        close_in_progress: Option<(usize, usize)>,
        close_park: Option<(usize, usize)>,
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
    let link_base = 1 + route_part.len() + 1;
    // A close-shaped `=` suffix (`x`, `x` plus lists, or a retired `*`/`!`
    // alias) lexes through the shared selection lexer with token-relative
    // offsets, so the editor can span the lists, report precise
    // diagnostics, and surface the dangling separator as an incomplete
    // state. Every other suffix keeps today's `=<X>` start handling below.
    if let Some((before_eq, raw_suffix)) = tail.split_once('=')
        && (raw_suffix.eq_ignore_ascii_case("x")
            || link_close_after_x(raw_suffix).is_some())
    {
        return classify_caret_close_suffix(route_part, before_eq, raw_suffix);
    }
    match parse_colon_link_tail(text, route_part, tail, true) {
        Ok(parts) => {
            let route = route_part.to_ascii_lowercase();
            CaretTokenShape::Complete {
                route,
                block_id: parts.block_id,
                pomodoro_name: parts.pomodoro_name,
                start: parts.start,
                close: parts.close,
                link_end: parts.link_end,
                name_range: parts.name_range,
                start_offset: parts.start_offset,
                bare_name: parts.bare_name,
                start_ranges: parts.start_ranges,
                close_in_progress: None,
                close_park: None,
                close_complete: None,
                close_drop: None,
            }
        }
        Err(message) => {
            if message.contains("is incomplete: type the timing after `@`") {
                let (before_eq, raw_after) = match tail.split_once('=') {
                    Some((before, after)) => (before, Some(after)),
                    None => (tail, None),
                };
                let block = before_eq
                    .split_once('#')
                    .map(|(block, _)| block)
                    .unwrap_or(before_eq);
                if block.is_empty()
                    || !is_block_id(block)
                    || raw_after.is_none()
                {
                    return CaretTokenShape::Invalid(message);
                }
                let raw = raw_after.expect("has suffix");
                let start_token = format!("={raw}");
                let Some(
                    crate::native::capture_language::item::EqualsToken::Start {
                        suffix,
                        name,
                        name_spelling,
                        timing,
                        ..
                    },
                ) = crate::native::capture_language::item::session_equals_token(
                    &start_token,
                )
                else {
                    return CaretTokenShape::Invalid(message);
                };
                if !timing.as_ref().is_some_and(|(_, text)| text.is_empty())
                    || !suffix.is_empty()
                {
                    return CaretTokenShape::Invalid(message);
                }
                let name_text = name.unwrap_or_default();
                if name_text.is_empty()
                    || !is_pomodoro_selector_component(&name_text)
                {
                    return CaretTokenShape::Invalid(message);
                }
                let spelling = name_spelling.expect("named spelling");
                let link_end = link_base + block.len();
                let start_offset = link_base + before_eq.len();
                let name_start = start_offset
                    + crate::native::capture_language::item::named_start_name_offset(
                        suffix.len(),
                        spelling,
                    );
                let name_range = (name_start, name_start + name_text.len());
                let mut start_ranges = Vec::new();
                if start_offset < name_start {
                    let prefix_end = match spelling {
                        crate::native::capture_language::bare_name::NameSpelling::Hash => {
                            name_start - 1
                        }
                        crate::native::capture_language::bare_name::NameSpelling::Bare => {
                            name_start
                        }
                    };
                    if start_offset < prefix_end {
                        start_ranges.push((start_offset, prefix_end));
                    }
                }
                let separator_range =
                    (text.len().saturating_sub(1), text.len());
                return CaretTokenShape::TimingIncomplete {
                    route: route_part.to_ascii_lowercase(),
                    block_id: block.to_string(),
                    pomodoro_name: name_text,
                    link_end,
                    name_range,
                    start_offset,
                    start_ranges,
                    separator_range,
                };
            }
            if message == POMODORO_NAME_REQUIRED_ERROR {
                let (before_start, raw_after) = match tail.split_once('=') {
                    Some((before, after)) => (before, Some(after)),
                    None => (tail, None),
                };
                let block = before_start
                    .split_once('#')
                    .map(|(block, _)| block)
                    .unwrap_or(before_start);
                let link_end = link_base + block.len();
                let (hash_offset, start_offset) = match tail.split_once('=') {
                    Some((before, after)) => {
                        let name_position = before
                            .split_once('#')
                            .map(|(block, _)| block.len())
                            .unwrap_or(before.len());
                        let hash_offset = if before.contains('#') {
                            link_base + name_position
                        } else {
                            link_base
                                + before.len()
                                + 1
                                + after
                                    .find('#')
                                    .unwrap_or(after.len().saturating_sub(1))
                        };
                        (hash_offset, Some(link_base + before.len()))
                    }
                    None => (link_base + block.len(), None),
                };
                // An empty name must not discard a valid timing prefix or
                // suffix. Lex the `=` suffix so `=3`, `=`, and `@3` stay
                // visible, mirroring the `@` incomplete path.
                let mut start_ranges: Vec<(usize, usize)> = Vec::new();
                let mut partial_start: Option<PomodoroStartSpec> = None;
                if let (Some(offset), Some(raw)) = (start_offset, raw_after) {
                    let start_token = format!("={raw}");
                    if let Some(
                        crate::native::capture_language::item::EqualsToken::Start {
                            suffix,
                            timing,
                            ..
                        },
                    ) = crate::native::capture_language::item::session_equals_token(
                        &start_token,
                    ) {
                        if !suffix.is_empty() {
                            start_ranges.push((
                                offset,
                                offset + 1 + suffix.len(),
                            ));
                        } else if raw.starts_with('#')
                            || raw.starts_with('@')
                            || raw.is_empty()
                        {
                            let prefix_empty = before_start.contains('#');
                            if !prefix_empty {
                                start_ranges.push((offset, offset + 1));
                            } else if !suffix.is_empty() {
                                start_ranges.push((
                                    offset,
                                    offset + 1 + suffix.len(),
                                ));
                            } else {
                                // Prefix-empty `#=3` paints `=3` here via
                                // the suffix length below; when the suffix
                                // itself is empty (`#=#` edge) keep `=`.
                                start_ranges.push((offset, offset + 1));
                            }
                            // For prefix-empty `#=3` the `=3` span was not
                            // pushed above (suffix non-empty branch missed
                            // because we took the bare `=` branch). Push it
                            // now when the raw timing is a plain suffix.
                            if prefix_empty
                                && !suffix.is_empty()
                                && start_ranges.len() == 1
                                && start_ranges[0].1 == offset + 1
                            {
                                start_ranges[0].1 = offset + 1 + suffix.len();
                            }
                        }
                        if let Some((timing_offset, timing_text)) =
                            timing.as_ref()
                            && !timing_text.is_empty()
                        {
                            start_ranges.push((
                                offset + timing_offset,
                                offset + timing_offset + 1 + timing_text.len(),
                            ));
                        }
                        // Prefix-empty `#=3` with a plain numeric suffix
                        // (`suffix` non-empty, no `@` timing) still needs its
                        // `=3` span when the bare-`=` branch above skipped it.
                        if start_ranges.is_empty()
                            && !suffix.is_empty()
                            && let Some(off) = start_offset
                        {
                            start_ranges.push((off, off + 1 + suffix.len()));
                        }
                        let effective =
                            crate::native::capture_language::item::effective_start_timing(
                                &suffix, &timing,
                            );
                        partial_start =
                            parse_pomodoro_start_suffix(&effective).ok();
                    } else {
                        // Unlexable suffix: keep the old single-span fallback
                        // so missing-route/block states stay intact.
                        let end = tail
                            .find('#')
                            .map(|hash| link_base + hash)
                            .unwrap_or(offset + 1);
                        if offset < end {
                            start_ranges.push((offset, end));
                        }
                    }
                }
                return CaretTokenShape::NamePartial {
                    route: route_part.to_ascii_lowercase(),
                    block_id: block.to_string(),
                    link_end,
                    name_range: (hash_offset + 1, hash_offset + 1),
                    start_offset,
                    start_ranges,
                    start: partial_start,
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
                link_end: link_base + block_id.len(),
                name_range: (
                    link_base + block_id.len() + 1,
                    link_base + block_id.len() + 1,
                ),
                start_offset: None,
                start_ranges: Vec::new(),
                start: None,
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
    let display = format!("={raw_suffix}");
    if let Some(rewrite) = retired_link_alias_rewrite(raw_suffix) {
        return CaretTokenShape::CloseInvalid {
            message: retired_link_close_alias_error(&display, &rewrite),
            range: (suffix_offset, suffix_offset + display.len()),
        };
    }
    let (after_x, x_len) = classify_link_close(raw_suffix)
        .map(|(selection, x_len)| (selection, x_len))
        .unwrap_or(("", 1));
    let after_x_offset = suffix_offset + 1 + x_len;
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
            bare_name: false,
            start_ranges: Vec::new(),
            close_in_progress: lex.in_progress_range,
            close_park: lex.park_range,
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
                close_park: incomplete.park_range,
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
    "Pomodoro link capture cannot be combined with $<N>";

/// Remaining parent tokens after relationship modifiers (and, when those
/// were present, trailing `$`/`s:`/`%` markers) leave. Used so a bare
/// `^route:id` owner alias works in both leading and trailing position.
fn relation_stripped_parent_tokens<'a>(
    parent_line: RawLine<'a>,
) -> (Vec<Token<'a>>, LineDependencies) {
    let tokens = tokenize_line_with_spans(&parent_line);
    extract_line_dependencies(
        tokens,
        parent_line.text,
        parent_line.start,
        true,
        true,
    )
}

fn line_has_relations(deps: &LineDependencies) -> bool {
    !deps.entries.is_empty()
        || !deps.blocked_entries.is_empty()
        || !deps.relation_entries.is_empty()
        || !deps.partials.is_empty()
        || !deps.blocked_partials.is_empty()
        || !deps.relation_target_partials.is_empty()
        || !deps.relation_label_partials.is_empty()
}

fn remainder_is_caret_owner(tokens: &[Token<'_>]) -> bool {
    let mut remaining = tokens.to_vec();
    let _ = extract_terminal_markers(&mut remaining, true);
    matches!(
        remaining.as_slice(),
        [token] if parse_caret_link_token(token.text).is_ok()
    )
}

/// Editor reading of an item whose remaining parent body (after
/// relationship modifiers) starts with `^`. Returns `None` when the item
/// stays on the ordinary path (no `^` first remaining token, a lookalike,
/// or a partial shape with other text on the item, all of which stay
/// prose). Otherwise the `^` token claims the item: a complete shape
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
        bare_name: bool,
        start_ranges: Vec<(usize, usize)>,
        close_in_progress: Option<(usize, usize)>,
        close_park: Option<(usize, usize)>,
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
        close_park: Option<(usize, usize)>,
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
        link_end: usize,
        name_range: (usize, usize),
        start_offset: Option<usize>,
        start_ranges: Vec<(usize, usize)>,
        start: Option<PomodoroStartSpec>,
    },
    TimingIncomplete {
        route: String,
        block_id: String,
        pomodoro_name: String,
        link_end: usize,
        name_range: (usize, usize),
        start_offset: usize,
        start_ranges: Vec<(usize, usize)>,
        separator_range: (usize, usize),
    },
    Invalid {
        message: String,
    },
}

pub(super) fn classify_caret_item<'a>(
    item: &CaptureItem<'a>,
    parent_line: RawLine<'a>,
) -> Option<CaretItem<'a>> {
    let original = tokenize_line_with_spans(&parent_line);
    let (stripped, deps) = relation_stripped_parent_tokens(parent_line);
    let has_relations = line_has_relations(&deps);
    let parent_tokens = if has_relations {
        let mut remaining = stripped;
        let _ = extract_terminal_markers(&mut remaining, true);
        remaining
    } else {
        original
    };
    let first = parent_tokens.first()?;
    if !first.text.starts_with('^') {
        return None;
    }
    let solo_parent = parent_tokens.len() == 1;
    // Relationship extras (`$<N>`/`s:`/`%`/children) are not caret-link
    // conflicts: ownership emits `relationship_needs_text` instead.
    let single_line = if has_relations {
        true
    } else {
        item.lines.len() == 1
    };
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
        CaretTokenShape::NamePartial {
            route,
            block_id,
            link_end,
            name_range,
            start_offset,
            start_ranges,
            start,
        } => {
            if solo_parent && single_line {
                Some(CaretItem {
                    token: *first,
                    solo_parent,
                    kind: CaretItemKind::NamePartial {
                        route,
                        block_id,
                        link_end,
                        name_range,
                        start_offset,
                        start_ranges,
                        start,
                    },
                })
            } else {
                None
            }
        }
        CaretTokenShape::TimingIncomplete {
            route,
            block_id,
            pomodoro_name,
            link_end,
            name_range,
            start_offset,
            start_ranges,
            separator_range,
        } => {
            if solo_parent && single_line {
                Some(CaretItem {
                    token: *first,
                    solo_parent,
                    kind: CaretItemKind::TimingIncomplete {
                        route,
                        block_id,
                        pomodoro_name,
                        link_end,
                        name_range,
                        start_offset,
                        start_ranges,
                        separator_range,
                    },
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
            bare_name,
            start_ranges,
            close_in_progress,
            close_park,
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
                    bare_name,
                    start_ranges,
                    close_in_progress,
                    close_park,
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
            close_park,
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
                    close_park,
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
                } else if is_priority_marker_token(extra.text) {
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
    let parent_raw = parent_line.raw;
    let (stripped, deps) = relation_stripped_parent_tokens(parent_raw);
    if line_has_relations(&deps) && remainder_is_caret_owner(&stripped) {
        // Bare `^route:id` beside `&`/`*` is an owner alias, not a
        // whole-item ledger link. The generic path classifies it.
        return Ok(None);
    }
    let parent_trimmed = parent_raw.text.trim();
    if !parent_trimmed.starts_with('^') {
        return Ok(None);
    }
    let tokens: Vec<&str> = parent_trimmed.split_whitespace().collect();
    let Some(first_token) = tokens.first().copied() else {
        return Ok(None);
    };
    if !first_token.starts_with('^') {
        return Ok(None);
    }
    // A `#"…"` literal may hold spaces: extend the marker through its
    // closing quote so the name lexes whole.
    let (first, first_len) = expand_literal_token(parent_trimmed, first_token);
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
    // Tokens consumed by a spaced literal are part of the marker, not
    // extra text.
    let is_solo_parent = parent_trimmed.len() == first_len;
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
                    for extra in &tokens[1..] {
                        if parse_schedule_token(extra).is_some() {
                            return Err(
                                "Pomodoro link capture cannot be combined with s:<N>".to_string(),
                            );
                        }
                        if is_priority_marker_token(extra) {
                            return Err(
                                "Pomodoro link capture cannot be combined with $<N>".to_string(),
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
                        pomodoro_name_is_literal: parts
                            .pomodoro_name_is_literal,
                        start: parts.start,
                        close: parts.close,
                        spelling: PomodoroLinkSpelling::Caret,
                    },
                    scheduled_offset: None,
                    priority_level: None,
                    sub_bullets: Vec::new(),
                    dependencies: Vec::new(),
                    blocked_tasks: Vec::new(),
                    relations: Vec::new(),
                    dependency_target: None,
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
            let close_shaped = first.contains("=x")
                || first.contains("=X")
                || first.contains("=*")
                || first.contains("=!");
            if close_shaped && !is_solo_item {
                if !is_solo_parent {
                    for extra in &tokens[1..] {
                        if parse_schedule_token(extra).is_some() {
                            return Err(
                                "Pomodoro link capture cannot be combined with s:<N>"
                                    .to_string(),
                            );
                        }
                        if is_priority_marker_token(extra) {
                            return Err(
                                "Pomodoro link capture cannot be combined with $<N>"
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
