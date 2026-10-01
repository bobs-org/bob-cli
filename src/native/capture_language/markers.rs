//! Terminal-marker extraction, schedules, priorities, clips, and marker messages.

use super::editor_model::*;
use super::line::*;
use super::model::*;
use super::tokens::*;
use crate::native::capture_clip;
use std::num::NonZeroUsize;

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

pub(super) fn parse_clip_token(token: &str) -> Option<ClipRequest> {
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
pub(super) fn extract_terminal_marker(
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

pub(super) fn is_route_marker(token: &str) -> bool {
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
pub(super) fn is_pomodoro_note_marker(token: &str) -> bool {
    token == "#"
}

/// Reject the retired standalone bullet marker forms so they fail clearly
/// instead of silently capturing literal `#...` text. The marker is honored
/// only when appended to an `@route` token (`@foo#bar`).
///
/// Two terminal positions are rejected: a final token that itself starts with
/// `#`, and (when `allow_route`) a final plain `@route` token preceded by a
/// `#...` token. A `#tag` anywhere else stays literal task text.
pub(super) fn reject_legacy_bullet_markers(
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

pub(super) fn normalize_forced_route(route: &str) -> Result<String, String> {
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

pub(super) const GLOBAL_DESTINATION_SHAPE_ERROR: &str =
    "global destination must be @@<route> or @@<route>+<block-id>";

pub(super) const GLOBAL_DESTINATION_ROUTE_ERROR: &str =
    "global destination route must contain only A-Z, a-z, 0-9, '_' or '-'";

pub(super) const GLOBAL_DESTINATION_BLOCK_ID_ERROR: &str =
    "global destination block ID must be non-empty and contain only A-Z, a-z, 0-9 or '-'";

pub(super) const MISSING_CAPTURE_ITEM_ERROR: &str =
    "global destination declaration has no capture item; add a capture item to this draft";

pub(super) const EXPLICIT_TOGGLE_ONLY_MARKER_ERROR: &str = "explicit toggle `!` is only valid on a marker-only `@<route>+<block-id>!` capture; it cannot be combined with body text, authored children, clipboard, schedule, priority, or a Pomodoro name";

pub(super) const EXPLICIT_TOGGLE_NAMED_ERROR: &str = "explicit toggle `!` cannot be combined with a `#<pomodoro>` selector; use `@<route>+<block-id>!`";

pub(super) const EXPLICIT_TOGGLE_REPEATED_ERROR: &str =
    "explicit toggle `!` cannot be repeated; use a single `@<route>+<block-id>!`";

pub(super) const EXPLICIT_TOGGLE_GLOBAL_ERROR: &str = "explicit toggle `!` cannot be used on a `@@` destination; use a marker-only `@<route>+<block-id>!` item";

pub(super) const SUB_BULLET_SHAPE_ERROR: &str =
    "sub-bullet capture markers must use @<route>+<block-id> or @<route>+<block-id>#<section>";

pub(super) const SUB_BULLET_ROUTE_ERROR: &str =
    "sub-bullet capture route must contain only A-Z, a-z, 0-9, '_' or '-'";

pub(super) const SUB_BULLET_BLOCK_ID_ERROR: &str =
    "sub-bullet capture block ID must be non-empty and contain only A-Z, a-z, 0-9 or '-'";

pub(super) const SUB_BULLET_SECTION_ERROR: &str =
    "sub-bullet capture section must contain only A-Z, a-z, 0-9 or & ' ( ) , . / -";

pub(super) const TASK_BLOCK_ID_ROUTE_ERROR: &str =
    "task block-ID capture route must contain only A-Z, a-z, 0-9, '_' or '-'";

pub(super) const TASK_BLOCK_ID_ERROR: &str =
    "task block-ID capture block ID must be non-empty and contain only A-Z, a-z, 0-9 or '-'";

pub(super) const RETIRED_DOUBLE_COLON_ERROR: &str = "'@<route>::<block-id>' is no longer accepted; use '@<route>^<block-id>' to create an ordinary task with an authored block ID";

pub(super) const POMODORO_SHAPE_ERROR: &str =
    "Pomodoro capture markers must use @<route>:<block-id> or @<route>:<block-id>#<pomodoro>";

pub(super) const POMODORO_ROUTE_ERROR: &str =
    "Pomodoro capture route must contain only A-Z, a-z, 0-9, '_' or '-'";

pub(super) const POMODORO_BLOCK_ID_ERROR: &str =
    "Pomodoro capture block ID must be non-empty and contain only A-Z, a-z, 0-9 or '-'";

pub(super) const POMODORO_NAME_ERROR: &str =
    "Pomodoro capture name must contain only A-Z, a-z, 0-9 or \
`& ' ( ) + , . / -`";

pub(super) const POMODORO_NAME_REQUIRED_ERROR: &str = "Pomodoro capture requires a Pomodoro name: `@<route>:<block-id>#<pomodoro>` (run `bob capture-pomodoros` to list today's Pomodoros)";

/// Misordered project-note marker: the `+` must sit right after the block
/// ID, because a `+` after `#name` is part of the Pomodoro name.
pub(super) fn project_note_misordered_error(
    route: &str,
    block_id: &str,
    name: &str,
) -> String {
    format!(
        "put the project-note `+` right after the block ID: `@{route}^{block_id}+#{name}` (a `+` after `#{name}` would be part of the Pomodoro name)"
    )
}

/// Pomodoro name on a `^` project-note marker without the `+` sigil.
pub(super) fn project_note_name_without_plus_error(
    route: &str,
    block_id: &str,
    name: &str,
) -> String {
    format!(
        "`#{name}` after `@{route}^{block_id}` needs the project-note `+` (`@{route}^{block_id}+#{name}`); to link a task under a Pomodoro, use `@{route}:{block_id}#{name}`"
    )
}

/// Empty Pomodoro name on a `^` project-note marker (`@route^id+#`).
pub(super) fn project_note_name_required_error(
    route: &str,
    block_id: &str,
) -> String {
    format!(
        "project-note capture requires a Pomodoro name: `@{route}^{block_id}+#<pomodoro>` (run `bob capture-pomodoros` to list today's Pomodoros)"
    )
}

/// Retired `:` project-note form. A project note never links its own `^prj`
/// task; the caller passes the typed token plus its route, block, and name
/// parts so the message can teach the `^` replacement.
pub(super) fn retired_project_note_marker_error(
    typed: &str,
    route: &str,
    block_id: &str,
    pomodoro_name: Option<&str>,
) -> String {
    let route = if route.is_empty() { "<route>" } else { route };
    let replacement = match pomodoro_name {
        Some(name) => format!("@{route}^{block_id}+#{name}"),
        None => format!("@{route}^{block_id}+"),
    };
    format!(
        "`{typed}` is retired: a project note never links its own `^prj` task. Write `{replacement}` and end each task bullet you want in the Pomodoro with ` :<id>`"
    )
}

pub(super) const UNUSED_PROJECT_NOTE_POMODORO_ERROR_PREFIX: &str =
    "picks the Pomodoro for ` :<id>` task links, but no task bullet ends with ` :<id>`";

/// A project-note `#pomodoro` name with no ` :` task to link.
pub(crate) fn unused_project_note_pomodoro_error(name: &str) -> String {
    format!(
        "`#{name}` {UNUSED_PROJECT_NOTE_POMODORO_ERROR_PREFIX}; add one or remove `#{name}`"
    )
}

pub(super) const POMODORO_START_SHAPE_ERROR: &str = "Pomodoro start suffix must mirror se<X>: use `=<X>` where <X> is empty, digits, `-`, `-digits`, or `digits-` with optional digits (for example `@<route>:<block-id>=`, `@<route>:<block-id>=3`, `@<route>:<block-id>=-2`)";

pub(super) const POMODORO_START_OVERFLOW_ERROR: &str =
    "Pomodoro start suffix is too large; use a smaller duration or offset";

pub(super) const POMODORO_START_PROJECT_NOTE_ERROR: &str = "Pomodoro start suffix `=<X>` applies only to `@<route>:<block-id>` task captures, not project-note `@<route>^<block-id>+` forms";

pub(crate) const POMODORO_START_SCHEDULE_CONFLICT_ERROR: &str = "Pomodoro start suffix `=<X>` cannot be combined with `s:<N>`; a scheduled Blocked task cannot start its session";

pub(crate) const POMODORO_START_PRIORITY_CONFLICT_ERROR: &str = "Pomodoro start suffix `=<X>` cannot be combined with `p:<N>`; a scheduled Blocked task cannot start its session";

pub(crate) const POMODORO_LINK_SHAPE_ERROR: &str = "`^route:block-id` must be the whole capture item; to create a new Pomodoro-linked task use `<text> @route:block-id`";

pub(super) const POMODORO_LINK_INCOMPLETE_ERROR: &str = "incomplete Pomodoro link; finish the marker `^<route>:<block-id>` (for example `^sase:deep-fix`)";

pub(super) const POMODORO_LINK_NAME_INCOMPLETE_ERROR: &str = "incomplete Pomodoro link; finish the Pomodoro name `^<route>:<block-id>#<pomodoro>`";

pub(super) const POMODORO_LINK_PROJECT_ERROR: &str = "`^route:block-id+` is not a capture form; create a project note with `@route^block-id+`";

pub(super) const POMODORO_LINK_TOGGLE_ERROR: &str = "Pomodoro link `^route:block-id!` is a task toggle; use `@route+block-id!` for the explicit toggle";

/// T1: a `:` picker query whose suffix is not a complete caret link. The
/// query is only a picker search; it is never captured.
pub(super) fn task_link_picker_error(token: &str) -> String {
    format!(
        "`{token}` opens the task picker; pick a task to insert its `@<route>:<block-id>` link, or write the link yourself (for example `@sase:deep-fix`)"
    )
}

/// T2: a `:` picker query whose suffix parses as a complete caret link
/// (`route:block-id[#name][=<X>|=x…]`). It teaches the `@` spelling.
pub(super) fn task_link_picker_at_error(token: &str, query: &str) -> String {
    format!(
        "`{token}` opens the task picker; to link that task, write `@{query}`"
    )
}

pub(super) const POMODORO_ADJUST_ZERO_ERROR: &str = "Pomodoro adjustment magnitude must be positive; `+0` and `-0` adjust nothing (for example `+5` extends by 25 minutes)";

pub(super) const POMODORO_ADJUST_OVERFLOW_ERROR: &str =
    "Pomodoro adjustment is too large; use a smaller unit count";

pub(super) const POMODORO_ADJUST_SHAPE_ERROR: &str = "Pomodoro adjustment items must contain only the signed count (for example `+5` or `-`); remove extra text, markers, or child lines";

pub(super) const POMODORO_ADJUST_FORCED_ERROR: &str = "Pomodoro adjustment `+N`/`-N` cannot be combined with --route, --section, --task, --task-ref, --task-section, or --clip; capture the adjustment alone";

pub(super) const POMODORO_SHIFT_ZERO_ERROR: &str = "Pomodoro shift magnitude must be positive; `++0` and `--0` move nothing (for example `++3` moves today's Pomodoro 15 minutes later)";

pub(super) const POMODORO_SHIFT_OVERFLOW_ERROR: &str =
    "Pomodoro shift is too large; use a smaller unit count";

pub(super) const POMODORO_SHIFT_SHAPE_ERROR: &str = "Pomodoro shift items must contain only the operator (for example `++3` or `--`); remove extra text, markers, or child lines";

pub(super) const POMODORO_SHIFT_FORCED_ERROR: &str = "Pomodoro shift `++N`/`--N` cannot be combined with --route, --section, --task, --task-ref, --task-section, or --clip; capture the shift alone";

pub(crate) const POMODORO_CLOSE_INTERNAL_BULLETS_ERROR: &str =
    "a close never carries authored sub-bullets";

pub(crate) const POMODORO_START_FORCED_ERROR: &str = "Pomodoro start `=<X>` cannot be combined with --route, --section, --task, --task-ref, --task-section, or --clip; capture the start alone";

pub(super) const POMODORO_CLOSE_FORCED_ERROR: &str = "Pomodoro close `=x` cannot be combined with --route, --section, --task, --task-ref, --task-section, or --clip; capture the close alone";

/// Whole-item start shape error, formatted with the typed token: a counted
/// token with extra text, markers, or child lines, or an exact token with
/// child lines, is never a task.
pub(super) fn pomodoro_start_shape_error(token: &str, suffix: &str) -> String {
    format!(
        "Pomodoro start `{token}` must be the whole capture item; remove extra text, markers, or child lines (to start a task's session instead, use `^route:block-id={suffix}`)"
    )
}

/// Named-start E1: `=#` / `=3#` with an empty name part.
pub(super) fn pomodoro_named_start_incomplete_error(token: &str) -> String {
    format!(
        "`{token}` is incomplete: type a Pomodoro name after `#` (for example `{token}deep-work`)"
    )
}

/// Named-start E2: invalid name characters.
pub(super) fn pomodoro_named_start_name_error(
    name: &str,
    token: &str,
) -> String {
    format!(
        "Pomodoro name `{name}` in `{token}` may contain only A-Z, a-z, 0-9 or `& ' ( ) + , . / -`; write spaces as `-`"
    )
}

/// Named-start E4 base: extra text or child lines on a named start.
pub(super) fn pomodoro_named_start_shape_error(
    token: &str,
    suffix: &str,
    name: &str,
) -> String {
    format!(
        "Pomodoro start `{token}` must be the whole capture item; remove extra text, markers, or child lines (to start a task's session in a named Pomodoro instead, use `^route:block-id#{name}={suffix}`)"
    )
}

/// Named-start E4 variant when the name is empty and extra text follows
/// (`=# bugs`): no space after `#`.
pub(super) fn pomodoro_named_start_nospace_error(
    suffix: &str,
    word: &str,
) -> String {
    format!(
        "write the Pomodoro name right after `#`, with no space: `={suffix}#{word}`"
    )
}

/// Named-start E5: `=x#…` close near miss.
pub(super) fn pomodoro_close_hash_error(name: &str) -> String {
    if name.is_empty() {
        "`=x` always closes the running Pomodoro; remove `#`".to_string()
    } else {
        format!(
            "`=x` always closes the running Pomodoro; remove `#{name}`, or write `=x =#{name}` to close it and then start that Pomodoro"
        )
    }
}

pub(super) const POMODORO_CLOSE_PROJECT_NOTE_ERROR: &str = "Pomodoro close suffix `=x` applies only to `@<route>:<block-id>` task captures, not project-note `@<route>^<block-id>+` forms";

pub(crate) const POMODORO_CLOSE_SCHEDULE_CONFLICT_ERROR: &str = "Pomodoro close suffix `=x` cannot be combined with `s:<N>`; a scheduled task starts Blocked and cannot be worked in the closing session";

pub(crate) const POMODORO_CLOSE_PRIORITY_CONFLICT_ERROR: &str = "Pomodoro close suffix `=x` cannot be combined with `p:<N>`; a scheduled task starts Blocked and cannot be worked in the closing session";

// ---------------------------------------------------------------------------
// `=x[<N>][!<M>]` selection diagnostics
// ---------------------------------------------------------------------------

/// One shared source for every selection-list diagnostic, used by both the
/// execution and editor parsers so `bob capture` and `capture-parse` agree.
pub(super) fn close_selection_duplicate_error(
    number: u32,
    token: &str,
) -> String {
    format!("task {number} is listed twice in `{token}`")
}

pub(super) fn close_selection_overlap_error(
    number: u32,
    token: &str,
) -> String {
    format!(
        "task {number} cannot both stay in progress and complete in `{token}`"
    )
}

pub(super) fn close_selection_overlap_in_progress_drop_error(
    number: u32,
    token: &str,
) -> String {
    format!("task {number} cannot both stay in progress and drop in `{token}`")
}

pub(super) fn close_selection_overlap_complete_drop_error(
    number: u32,
    token: &str,
) -> String {
    format!("task {number} cannot both complete and drop in `{token}`")
}

pub(super) fn close_selection_zero_alone_error() -> String {
    "`0` means no task stays in progress; use it alone, as `=x0`, `=x0!2`, or `=x0~2`"
        .to_string()
}

pub(super) fn close_selection_starts_at_one_error() -> String {
    "task numbers start at 1".to_string()
}

pub(super) fn close_selection_expected_number_error() -> String {
    "expected a task number before `,`".to_string()
}

pub(super) fn close_selection_expected_number_after_error() -> String {
    "expected a task number after `,`".to_string()
}

pub(super) fn close_selection_one_bang_error() -> String {
    "use one `!` list: `=x1!2,3`".to_string()
}

pub(super) fn close_selection_one_tilde_error() -> String {
    "use one `~` list: `=x1~2,3`".to_string()
}

pub(super) fn close_selection_bad_list_error(token: &str) -> String {
    format!(
        "`{token}` is not a task list: write `=x`, then comma-separated task numbers, then optionally `!` and the numbers to complete and `~` and the numbers to drop (for example `=x1,3!2~4`)"
    )
}

pub(super) fn close_selection_too_large_error(number_text: &str) -> String {
    format!("task number {number_text} is too large")
}

pub(super) fn close_selection_no_spaces_error() -> String {
    "write the task numbers right after `=x`, with no spaces (for example `=x1,3!2~4`)".to_string()
}

/// Execution rejection for a dangling separator: `separator` is the `,` or
/// `!` the user still has to follow with a task number.
pub(super) fn close_selection_incomplete_error(
    token: &str,
    separator: char,
) -> String {
    format!("`{token}` is incomplete: type a task number after `{separator}`")
}

// ---------------------------------------------------------------------------
// `=x` Work Log bullet diagnostics (shared by `bob capture` and `capture-parse`)
// ---------------------------------------------------------------------------

/// Any token after the close token on its parent line. `bullet` is the
/// echoed `- <n> <text>` bullet when the tail starts with a positive number
/// (after an optional stray `-`/`*`/`+` token), or `None` for the generic
/// hint.
pub(super) fn close_parent_text_error(bullet: Option<&str>) -> String {
    match bullet {
        Some(bullet) => format!(
            "`=x` takes no text on its line; put each Work Log entry on its own line below it, as a bullet: `{bullet}`"
        ),
        None => "`=x` takes no text on its line; log work as bullets below it (for example `- 1 wrote the tests`); to link a task while closing, use `^route:block-id=x`"
            .to_string(),
    }
}

/// A Work Log bullet whose first token is not a task number. `example` is
/// the `- <n> <text>` fix that echoes the bullet's own text with the
/// smallest loggable number.
pub(super) fn close_log_missing_number_error(example: &str) -> String {
    format!(
        "start each Work Log bullet with the number of the task it logs to: `{example}`"
    )
}

/// A tail index that is not worked by this close. `list_suggestion` adds the
/// index to `<N>` (for example `=x2,3`) and `complete_suggestion` adds it to
/// `!<M>` (for example `=x2!3`).
pub(super) fn close_log_not_worked_error(
    index: u32,
    token: &str,
    list_suggestion: &str,
    complete_suggestion: &str,
) -> String {
    format!(
        "task {index} isn't worked by `{token}`; list it (`{list_suggestion}`) or complete it (`{complete_suggestion}`) to log to it"
    )
}

/// A tail index that is dropped by `~<K>`: `drop` renders the typed drop
/// list including the `~` (for example `~2`).
pub(super) fn close_log_dropped_error(index: u32, drop: &str) -> String {
    format!("task {index} is dropped by `{drop}`, so it can't take a Work Log entry")
}

/// A dangling bullet at execution: the bullet head (for example `- 1`).
pub(super) fn close_log_dangling_error(
    bullet_head: &str,
    index: u32,
) -> String {
    format!(
        "`{bullet_head}` is incomplete: type the Work Log text after task {index}"
    )
}

/// A block link inside a Work Log bullet.
pub(super) fn close_log_block_link_error(link: &str) -> String {
    format!("a Work Log bullet can't contain the block link `{link}`; the close would treat it as a Task Link")
}

/// Bullet text that starts with a code fence.
pub(super) fn close_log_fence_error() -> String {
    "a Work Log bullet can't start with a code fence".to_string()
}

// ---------------------------------------------------------------------------
// `=[<X>][#name]~<K>` start-drop diagnostics
// ---------------------------------------------------------------------------

/// A second `~` in a start drop list points at the second tilde.
pub(super) fn start_drop_one_tilde_error() -> String {
    "use one `~` list: `=~2,3`".to_string()
}

/// A bad byte in a start drop list fails from that byte through the token end.
pub(super) fn start_drop_bad_list_error(token: &str) -> String {
    format!(
        "`{token}` is not a drop list: write `~`, then comma-separated task numbers (for example `=~2,3`)"
    )
}

/// `!` never belongs on a start: it completes links when closing.
pub(super) fn start_drop_bang_error() -> String {
    "a start can only drop Task Links; `!` completes them when you close (`=x!3`)".to_string()
}

/// A `#name` typed after the drop list belongs before it. `suggestion` keeps
/// the typed `<X>` (for example `=3#bugs~2`).
pub(super) fn start_drop_hash_error(suggestion: &str, token: &str) -> String {
    format!(
        "write the drop list after the name: `{suggestion}` instead of `{token}`"
    )
}

/// A spaceless join that lexes as a start drop list gets this hint instead
/// of the shape error.
pub(super) fn start_drop_no_spaces_error() -> String {
    "write the task numbers right after `~`, with no spaces (for example `=~2,3`)".to_string()
}

// ---------------------------------------------------------------------------
// Editor-facing parse
// ---------------------------------------------------------------------------
