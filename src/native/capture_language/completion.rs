//! Cursor completion for capture markers.

use super::dependencies::*;
use super::draft::*;
use super::editor_classify::*;
use super::editor_model::*;
use super::editor_parse::*;
use super::editor_pomodoro::*;
use super::line::*;
use super::markers::*;
use super::model::*;
use super::project_tasks::*;
use super::tokens::*;
use serde::Serialize;

/// Which discovery source a completion request should query.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CompletionContext {
    Route,
    Section,
    PomodoroBlockId,
    TaskBlockId,
    ProjectTaskBlockId,
    PomodoroName,
    PomodoroStartName,
    Task,
    TaskSection,
    ActiveTask,
    TaskLink,
    /// A bare-plus task search across the capture-target catalog. Accept
    /// inserts a complete `@route+block-id` parent marker.
    TaskParent,
    /// An `&` prerequisite modifier: vault-wide task search whose accept
    /// inserts a Bob-authored `&note:block-id` replacement. The candidate
    /// scan lands in the discovery phase; the lexical field (query plus the
    /// exact sigil-inclusive replacement range) is contract.
    TaskDependency,
    /// A whole-item `!` token: vault-wide open-task search whose accept
    /// inserts a Bob-authored `!note:block-id` replacement. The query is
    /// decoded like `&`; the replacement covers the whole token, sigil
    /// included, so refetching at `replacement.start` returns the
    /// unfiltered snapshot.
    TaskComplete,
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
    /// (`section`, `pomodoro_block_id`, `pomodoro_name`, `task`, and
    /// `task_section`).
    pub(crate) route: Option<String>,
    /// The already-typed block ID of a three-component
    /// `@route+id#section` or `@route:id#pomodoro` marker. Set only when
    /// `context` is `task_section` or `pomodoro_name`; always `None` for
    /// two-component contexts.
    pub(crate) block_id: Option<String>,
    /// The text already typed in this component, up to `cursor`.
    pub(crate) query: String,
    /// The half-open UTF-8 byte range of the whole component, which a
    /// completion replaces in full regardless of where the cursor sits
    /// inside it.
    pub(crate) replacement: (usize, usize),
}

pub(super) struct CompletionThird<'a> {
    pub(super) separator_len: usize,
    pub(super) part: &'a str,
    pub(super) context: CompletionContext,
}

pub(super) struct CompletionParts<'a> {
    pub(super) sigil_len: usize,
    pub(super) route_part: &'a str,
    pub(super) separator_len: usize,
    pub(super) right_part: &'a str,
    pub(super) right_context: Option<CompletionContext>,
    pub(super) third: Option<CompletionThird<'a>>,
}

/// Complete a solo leading `:` token: the query is the token text between
/// the sigil and the cursor (empty at or just after the sigil), and the
/// replacement covers the whole token including the sigil, because
/// accepting rewrites the query into the canonical `@route:block-id`
/// marker. The cursor may sit anywhere in `[token.start, token.end]`,
/// including just before the sigil so clients can refetch the full list at
/// `replacement.start`. Multi-token and multi-line items stay prose and
/// return `None`, exactly like the claim predicate.
pub(super) fn task_link_completion_field_at(
    item: &CaptureItem<'_>,
    cursor: usize,
) -> Option<CompletionField> {
    let token = task_link_query_token(item)?;
    if cursor < token.start || cursor > token.end {
        return None;
    }
    let relative = cursor - token.start;
    // A cursor just before the sigil refetches the full list with an empty
    // query; `get(1..0)` would be `None`, so spell that case directly.
    let query = if relative == 0 {
        String::new()
    } else {
        token.text.get(1..relative)?.to_string()
    };
    Some(CompletionField {
        context: CompletionContext::TaskLink,
        route: None,
        block_id: None,
        query,
        replacement: (token.start, token.end),
    })
}

/// Complete an `&` prerequisite modifier: the cursor inside a directive
/// complete or partial modifier (leading or trailing run, outside
/// protected spans) yields the `task_dependency` context. The query is the
/// text after the sigil up to the cursor (one opening quote stripped,
/// `\"`/`\\` decoded); the replacement covers exactly the whole modifier
/// including its sigil and quoted note, so accepting rewrites the query
/// into Bob's canonical `&note:block-id` form and refetching at
/// `replacement.start` returns the unfiltered snapshot. Literal mid-line
/// ampersands, `\&` escapes, and malformed modifiers yield nothing.
pub(super) fn dependency_completion_field_at(
    line: &RawLine<'_>,
    cursor: usize,
    parent: bool,
) -> Option<CompletionField> {
    if cursor < line.start || cursor > line.end {
        return None;
    }
    let scanned = scan_line_dependencies(line.text, line.start);
    let target = scanned.iter().find(|scan| {
        !matches!(
            scan,
            ScannedDependency::Escaped { .. } | ScannedDependency::Invalid(_)
        ) && scan.start() <= cursor
            && cursor <= scan.end()
    })?;
    // The modifier must be directive (a marker-run member), not literal
    // mid-line prose that merely looks like one.
    let tokens = tokenize_line_with_spans(line);
    let (_, found) =
        extract_line_dependencies(tokens, line.text, line.start, parent, true);
    let directive = found
        .entries
        .iter()
        .any(|entry| entry.start <= cursor && cursor <= entry.end)
        || found
            .partials
            .iter()
            .any(|(start, end)| *start <= cursor && cursor <= *end);
    if !directive {
        return None;
    }
    let (start, end) = (target.start(), target.end());
    // A cursor just before the sigil refetches the full list with an empty
    // query; slicing past the sigil would invert the range, so spell that
    // case directly (mirroring the `:` picker contract).
    let typed = if cursor == start {
        String::new()
    } else {
        let slice =
            line.text.get(start + 1 - line.start..cursor - line.start)?;
        decode_dependency_query(slice)
    };
    Some(CompletionField {
        context: CompletionContext::TaskDependency,
        route: None,
        block_id: None,
        query: typed,
        replacement: (start, end),
    })
}

/// Complete a solo leading `!` token: the query runs from just after
/// the sigil to the cursor (decoded like `&`, empty at or just before
/// the sigil) and the replacement covers the whole token including the
/// sigil, because accepting rewrites the query into the canonical
/// `!note:block-id` marker. The cursor may sit anywhere in
/// `[token.start, token.end]`, including just before the sigil so
/// clients can refetch the full list at `replacement.start`. Claimed
/// invalid tokens offer nothing: they carry a teaching diagnostic, not
/// a picker. Multi-line items stay prose, exactly like the claim
/// predicate.
pub(super) fn task_complete_completion_field_at(
    item: &CaptureItem<'_>,
    cursor: usize,
) -> Option<CompletionField> {
    match claim_bang_item(item)? {
        BangClaim::Query { raw, start, end } => {
            bang_completion_field(&raw, start, end, cursor)
        }
        BangClaim::Complete(dependency) => bang_completion_field(
            &dependency.raw,
            dependency.start,
            dependency.end,
            cursor,
        ),
        BangClaim::Invalid(_) => None,
    }
}

/// Claimed whole-item `!` complete tokens across a batch draft, in
/// source order: the item range plus the decoded note and block ID.
/// The picker-contract catalog consumes this instead of re-deriving
/// the claim; claimed invalid tokens, queries, and prose never appear
/// here.
pub(crate) fn claimed_task_complete_tokens(
    raw_text: &str,
) -> Vec<(usize, usize, String, String)> {
    split_capture_draft(raw_text)
        .items
        .iter()
        .filter_map(|item| match claim_bang_item(item)? {
            BangClaim::Complete(dependency) => Some((
                item.start,
                item.end,
                dependency.note.clone(),
                dependency.block_id.clone(),
            )),
            BangClaim::Query { .. } | BangClaim::Invalid(_) => None,
        })
        .collect()
}

fn bang_completion_field(
    raw: &str,
    start: usize,
    end: usize,
    cursor: usize,
) -> Option<CompletionField> {
    if cursor < start || cursor > end {
        return None;
    }
    // A cursor just before the sigil refetches the full list with an
    // empty query; slicing past the sigil would invert the range, so
    // spell that case directly (mirroring the `:` picker contract).
    let query = if cursor == start {
        String::new()
    } else {
        decode_dependency_query(raw.get(1..cursor - start)?)
    };
    Some(CompletionField {
        context: CompletionContext::TaskComplete,
        route: None,
        block_id: None,
        query,
        replacement: (start, end),
    })
}

/// Complete a solo leading `^` token: the `route:block-id` part (up to the
/// cursor, so typed suffixes survive an accept) offers vault-wide active
/// tasks, while a cursor after `#` offers Pomodoro names for the resolved
/// route and block ID. Anything else on the parent line is prose, a cursor
/// inside `=<X>` offers nothing, and partial shapes on multi-line items
/// stay prose, so those return `None`.
pub(super) fn caret_completion_field_at(
    item: &CaptureItem<'_>,
    line: RawLine<'_>,
    cursor: usize,
) -> Option<CompletionField> {
    let tokens = tokenize_line_with_spans(&line);
    let first = tokens.first()?;
    if !first.text.starts_with('^') || tokens.len() != 1 {
        return None;
    }
    if cursor <= first.start || cursor > first.end {
        return None;
    }
    let relative = cursor - first.start;
    match classify_caret_token(first.text) {
        CaretTokenShape::Prose
        | CaretTokenShape::Invalid(_)
        | CaretTokenShape::CloseInvalid { .. } => None,
        // A dangling separator still completes the link part it hangs off;
        // a cursor inside the typed suffix (or the separator) offers
        // nothing, so the typed lists survive an accept.
        CaretTokenShape::CloseIncomplete {
            route,
            block_id,
            link_end,
            suffix_offset,
            ..
        } => caret_link_field(
            first,
            route,
            block_id,
            link_end,
            Some((link_end, link_end)),
            cursor,
        )
        .filter(|_| cursor <= first.start + suffix_offset),
        CaretTokenShape::Partial { .. } => {
            if item.lines.len() != 1 {
                return None;
            }
            let query = first.text.get(1..relative)?.to_string();
            Some(CompletionField {
                context: CompletionContext::ActiveTask,
                route: None,
                block_id: None,
                query,
                replacement: (first.start + 1, first.end),
            })
        }
        CaretTokenShape::NamePartial { route, block_id } => {
            if item.lines.len() != 1 {
                return None;
            }
            // The token ends with a bare `#`: the link part ends before it
            // and the name is an empty insertion point just after it.
            let hash = first.text.find('#').unwrap_or(first.text.len());
            caret_link_field(
                first,
                route,
                block_id,
                hash,
                Some((hash + 1, hash + 1)),
                cursor,
            )
        }
        CaretTokenShape::Complete {
            route,
            block_id,
            link_end,
            name_range,
            start_offset,
            ..
        } => caret_link_field(
            first,
            route,
            block_id,
            link_end,
            name_range.or(Some((link_end, link_end))),
            cursor,
        )
        .filter(|_| {
            start_offset.is_none_or(|offset| cursor <= first.start + offset)
        }),
    }
}

/// Build the completion field for the `route:block-id[#name]` portion of a
/// `^` token: inside the link part the query runs from just after `^` to
/// the cursor and the replacement ends before any `#`/`=`; at or after `#`
/// the query is the typed name and the replacement covers the name part, so
/// accepting a candidate preserves a typed `#name`/`=<X>` suffix. `link_end`
/// and `name_range` are token-relative offsets from `classify_caret_token`.
pub(super) fn caret_link_field(
    token: &Token<'_>,
    route: String,
    block_id: String,
    link_end: usize,
    name_range: Option<(usize, usize)>,
    cursor: usize,
) -> Option<CompletionField> {
    let link_end_abs = token.start + link_end;
    if cursor <= link_end_abs {
        let query = token.text.get(1..cursor - token.start)?.to_string();
        return Some(CompletionField {
            context: CompletionContext::ActiveTask,
            route: None,
            block_id: None,
            query,
            replacement: (token.start + 1, link_end_abs),
        });
    }
    let (name_start, name_end) = name_range?;
    let name_start_abs = token.start + name_start;
    let name_end_abs = token.start + name_end;
    if cursor < name_start_abs || cursor > token.end {
        return None;
    }
    let query = token
        .text
        .get(name_start..cursor - token.start)?
        .to_string();
    Some(CompletionField {
        context: CompletionContext::PomodoroName,
        route: Some(route),
        block_id: Some(block_id),
        query,
        replacement: (name_start_abs, name_end_abs),
    })
}

/// Lexed facts for the named-start (`=<X>#name` or `==<X>#name`) token
/// whose name part holds `cursor`: the doubled-sigil flag, the raw `<X>`
/// suffix, the name text, and the absolute name-part span. `None` when the
/// cursor is not inside such a name part: just after `#` through the end
/// of the name part. A cursor on `=<X>` or at the `#` byte itself yields
/// nothing, and `#` is never inside the span. The lexer decides what
/// counts as a named start; validity never matters here, so near misses
/// lex the same way valid tokens do.
fn named_start_name_at(
    item: &CaptureItem<'_>,
    cursor: usize,
) -> Option<(bool, String, String, (usize, usize))> {
    let parent = item.lines.first()?;
    let text = parent.raw.text;
    let trimmed = text.trim();
    let super::item::EqualsToken::Start {
        suffix,
        name: Some(name),
        r#override,
        ..
    } = super::item::session_equals_token(trimmed)?
    else {
        return None;
    };
    let leading = text.len() - text.trim_start().len();
    let token_start = parent.raw.start + leading;
    // The `==` sigil is one byte longer than `=`, so a `==#` name field
    // starts one byte later.
    let sigil_len = if trimmed.starts_with("==") { 2 } else { 1 };
    let prefix_len = sigil_len + suffix.len();
    let name_start = token_start + prefix_len + 1;
    let name_end = name_start + name.len();
    if cursor < name_start || cursor > name_end {
        return None;
    }
    Some((r#override, suffix, name, (name_start, name_end)))
}

/// Completion field for the name part of a `=<X>#name` named Pomodoro
/// start. The cursor must lie inside `[name_start, name_end]`: just after
/// `#` through the end of the name part. A cursor on `=<X>` or at the `#`
/// byte itself yields nothing, and `#` is never inside the replacement.
/// The replacement covers the whole name part regardless of where the
/// cursor sits inside it.
fn pomodoro_start_name_field(
    item: &CaptureItem<'_>,
    cursor: usize,
) -> Option<CompletionField> {
    let (_, _, name, (name_start, name_end)) =
        named_start_name_at(item, cursor)?;
    let query = name.get(..cursor - name_start)?.to_string();
    Some(CompletionField {
        context: CompletionContext::PomodoroStartName,
        route: None,
        block_id: None,
        query,
        replacement: (name_start, name_end),
    })
}

/// For a `pomodoro_start_name` field, whether the name belongs to a `==`
/// override token: `Some(true)` when the `<X>` suffix is empty (a swap
/// keeps the running ledger) and `Some(false)` for a fresh `<X>` timing.
/// `None` for plain `=` tokens and when the cursor is not inside a
/// named-start name part.
pub(crate) fn pomodoro_start_override_at(
    raw_text: &str,
    cursor: usize,
) -> Option<bool> {
    if cursor_in_close_log_text(raw_text, cursor) {
        return None;
    }
    let draft = split_capture_draft(raw_text);
    let item = draft.items.iter().find(|item| {
        item.lines
            .iter()
            .any(|line| cursor >= line.raw.start && cursor <= line.raw.end)
    })?;
    let (is_override, suffix, _, _) = named_start_name_at(item, cursor)?;
    is_override.then_some(suffix.is_empty())
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
    // Work Log lines are literal text: no marker completion at all,
    // including the global `@@` path below. Note and heading wikilink
    // completion keeps working through the separate wikilink path.
    if cursor_in_close_log_text(raw_text, cursor) {
        return None;
    }
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
    // The name part of a `=<X>#name` named Pomodoro start completes
    // start-aware session rows, whether the token is valid, a near miss,
    // or the `=<X>#` incomplete state. This runs before the session-item
    // early return below so named starts keep their name completion.
    if let Some(field) = pomodoro_start_name_field(item, cursor) {
        return Some(field);
    }
    // Bare-plus task search is checked before the adjustment early return:
    // a literal `+` is both the existing +5m action and a task-picker
    // gesture, while numeric/double-plus operator tokens remain actions.
    if let Some(field) =
        parent_task_completion_field_at(item, line_index, cursor)
    {
        return Some(field);
    }
    // A whole-item `+[N]`/`-[N]` adjustment, `++[N]`/`--[N]` shift,
    // `=x` close, or `=`/`=<X>` start is an action, never a routed
    // capture: it requests no route or task completion candidates. The
    // `=`-family parser covers both closes and starts.
    if parse_editor_adjust_item(item).is_some()
        || parse_editor_close_item(item).is_some()
    {
        return None;
    }
    // A trailing ` :id` / ` ^id` on a first-level project-note bullet
    // completes the task ID itself; a cursor elsewhere on the line falls
    // through to the `@` marker path below.
    if line_index > 0
        && let Some(field) = project_task_completion_field(item, line, cursor)
    {
        return Some(field);
    }
    let leading = line_index == 0;

    // A solo leading `!` token completes vault-wide completable
    // tasks instead of offering `@` route completion. This runs before
    // the `:` and `^` paths; all three only fire on the item's leading
    // line and their claims are disjoint.
    if leading
        && let Some(field) = task_complete_completion_field_at(item, cursor)
    {
        return Some(field);
    }

    // A solo leading `:` token completes vault-wide linkable tasks
    // instead of offering `@` route completion: the query runs from just
    // after the sigil to the cursor and the replacement covers the whole
    // token including the sigil, so accepting rewrites the query into the
    // canonical link. This runs before the `^` path; both only fire on the
    // item's leading line.
    if leading && let Some(field) = task_link_completion_field_at(item, cursor)
    {
        return Some(field);
    }

    // A solo leading `^` token completes active tasks vault-wide instead of
    // offering `@` route completion: the cursor inside the `route:block-id`
    // part (including an empty part) requests the `active_task` context, a
    // cursor after `#` requests Pomodoro names, and a cursor inside `=<X>`
    // or `=x` requests nothing so a typed suffix survives an accept.
    if leading
        && let Some(field) = caret_completion_field_at(item, line, cursor)
    {
        return Some(field);
    }

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

    // A cursor inside an `&note:block-id` modifier (complete or still
    // typed) completes vault-wide prerequisite tasks instead of offering
    // `@` route completion. This runs before marker completion so the
    // modifier's own range wins over any neighboring marker.
    if let Some(field) =
        dependency_completion_field_at(&scan_line, cursor, leading)
    {
        return Some(field);
    }

    let mut tokens = tokenize_line_with_spans(&scan_line);
    take_global_declarations(&mut tokens);
    // Leading/trailing `&...` modifiers are not marker candidates: strip
    // them with the shared extractor (before terminal markers, exactly
    // like the editor parse) so an interleaved marker still completes.
    let (stripped, _) = extract_line_dependencies(
        tokens,
        scan_line.text,
        scan_line.start,
        leading,
        true,
    );
    tokens = stripped;
    extract_terminal_markers(&mut tokens, true);

    let index = completion_marker_index(&tokens, leading)?;
    let token = tokens[index];
    if cursor < token.start || cursor > token.end {
        return None;
    }

    // A `#` after `@route+block-id` completes a Pomodoro name instead of a
    // task section exactly when the finished item is (or is one keystroke
    // from becoming) a task toggle -- mirroring `parse_editor_item`'s own
    // toggle decision, since only that whole-item view knows whether the
    // body, authored children, and other item-wide markers allow it.
    let resolved = parse_editor_item(item).item;
    let sub_bullet_is_toggle = matches!(resolved.mode, EditorMode::TaskToggle)
        || (resolved.mode == EditorMode::Incomplete
            && resolved.needs == [Need::PomodoroName]);

    marker_field_at_cursor(&token, cursor, sub_bullet_is_toggle)
}

/// Build the vault-wide parent-task field for the shared terminal-plus
/// classifier. Its replacement owns the complete `+query` token so a
/// candidate can replace the gesture with `@route+block-id`.
fn parent_task_completion_field_at(
    item: &CaptureItem<'_>,
    line_index: usize,
    cursor: usize,
) -> Option<CompletionField> {
    let selector =
        parent_task_selector_tokens(item)
            .into_iter()
            .find(|selector| {
                selector.line_index == line_index
                    && cursor >= selector.token.start
                    && cursor <= selector.token.end
            })?;
    let relative = cursor - selector.token.start;
    let query = if relative <= 1 {
        String::new()
    } else {
        selector.token.text.get(1..relative)?.to_string()
    };
    Some(CompletionField {
        context: CompletionContext::TaskParent,
        route: None,
        block_id: None,
        query,
        replacement: (selector.token.start, selector.token.end),
    })
}

/// Trailing ` :id` / ` ^id` completion on a first-level project-note child.
///
/// On a first-level child line of an item whose editor parse is a project
/// note with a resolved route and project ID, a cursor inside the trailing
/// task-ID token (from just after the sigil to the token end) yields the
/// `project_task_block_id` context. The replacement is the ID span, which is
/// empty at the cursor for a lone sigil. The same `project_tasks` lexer and
/// last-body-word-after-markers rule backs both this and the editor spans,
/// so completion never disagrees with highlighting. Parent lines, nested
/// lines, non-project items, items without a resolved route or project ID,
/// and cursors outside the trailing token yield nothing.
fn project_task_completion_field(
    item: &CaptureItem<'_>,
    line: RawLine<'_>,
    cursor: usize,
) -> Option<CompletionField> {
    let info = project_task_token_at(item, line, cursor)?;
    let query = info
        .token
        .text
        .get(1..cursor - info.token.start)?
        .to_string();
    Some(CompletionField {
        context: CompletionContext::ProjectTaskBlockId,
        route: Some(info.stem),
        block_id: None,
        query,
        replacement: (info.token.start + 1, info.token.end),
    })
}

/// One trailing task-ID token with absolute offsets plus its project stem.
struct ProjectTaskToken<'a> {
    token: Token<'a>,
    stem: String,
}

/// Locate the trailing task-ID token at `cursor` on a first-level child
/// line of a project note with a resolved route and project ID. An item
/// left `incomplete` by a lone ` :` / ` ^` still qualifies: its marker is
/// resolved and only the ID is missing.
fn project_task_token_at<'a>(
    item: &CaptureItem<'a>,
    line: RawLine<'a>,
    cursor: usize,
) -> Option<ProjectTaskToken<'a>> {
    let AuthoredLineClass::Item(authored) = classify_authored_line(line) else {
        return None;
    };
    if authored.depth != AuthoredDepth::First || cursor < authored.body_start {
        return None;
    }
    let resolved = parse_editor_item(item).item;
    let is_project_note = matches!(
        resolved.mode,
        EditorMode::ProjectNote | EditorMode::PomodoroProjectNote
    ) || (resolved.mode == EditorMode::Incomplete
        && resolved.needs == [Need::BlockId]);
    if !is_project_note {
        return None;
    }
    let (Some(route), Some(project_id)) = (resolved.route, resolved.block_id)
    else {
        return None;
    };
    let child_line = RawLine {
        text: authored.body,
        start: authored.body_start,
        end: line.end,
    };
    let child_tokens = tokenize_line_with_spans(&child_line);
    let child_parse = parse_editor_line(
        child_tokens,
        child_line.text,
        child_line.start,
        false,
    );
    let token = child_parse.body_tokens.last()?;
    // The lexer decides what counts as a trailing task-ID token; prose
    // (`:)`, `10:30`) yields nothing.
    lex_project_task_id(token.text)?;
    if cursor < token.start + 1 || cursor > token.end {
        return None;
    }
    Some(ProjectTaskToken {
        token: *token,
        stem: format!("{}_{}", route, project_id.replace('-', "_")),
    })
}

/// One already-typed project task ID elsewhere in the same item, for the
/// `project_task_block_id` used list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProjectTaskUsedId {
    pub(crate) id: String,
    pub(crate) line: usize,
    pub(crate) text: String,
}

/// Detail behind the `project_task_block_id` block-ID object: the
/// project-note stem, the sigil, the whole-token marker range, the cursor
/// line's bullet body without its ID or leading checkbox, and `prj` plus
/// every other accepted task ID in the item, in source order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProjectTaskBlockIdDetail {
    pub(crate) stem: String,
    pub(crate) marker: char,
    pub(crate) marker_range: (usize, usize),
    pub(crate) body: String,
    pub(crate) used: Vec<ProjectTaskUsedId>,
}

/// Collect the `project_task_block_id` block-ID detail at `cursor`.
/// `replacement` is the ID span the detection step reported; a caller
/// passing a foreign range gets nothing rather than a mismatched object.
pub(crate) fn project_task_block_id_detail(
    raw_text: &str,
    cursor: usize,
    replacement: (usize, usize),
) -> Option<ProjectTaskBlockIdDetail> {
    let draft = split_capture_draft(raw_text);
    let (item, line_index, line) = draft.items.iter().find_map(|item| {
        item.lines
            .iter()
            .enumerate()
            .find(|(_, line)| {
                cursor >= line.raw.start && cursor <= line.raw.end
            })
            .map(|(line_index, line)| (item, line_index, line.raw))
    })?;
    if line_index == 0 {
        return None;
    }
    let info = project_task_token_at(item, line, cursor)?;
    if replacement != (info.token.start + 1, info.token.end) {
        return None;
    }
    let marker = info.token.text.chars().next()?;
    if marker != ':' && marker != '^' {
        return None;
    }
    let resolved = parse_editor_item(item).item;
    let parent_line_number =
        item.lines.first().map(|line| line.line_number).unwrap_or(1);
    let mut used = vec![ProjectTaskUsedId {
        id: "prj".to_string(),
        line: parent_line_number,
        text: clean_project_task_body(&resolved.body),
    }];
    // Run the shared per-item pass in source order so duplicate accounting
    // matches the editor spans; only accepted IDs join the used list, and
    // the cursor line never does.
    let mut pass = ProjectTaskPass::new();
    for (index, child) in item.lines.iter().enumerate().skip(1) {
        let AuthoredLineClass::Item(authored) =
            classify_authored_line(child.raw)
        else {
            continue;
        };
        let child_line = RawLine {
            text: authored.body,
            start: authored.body_start,
            end: child.raw.end,
        };
        let child_tokens = tokenize_line_with_spans(&child_line);
        let child_parse = parse_editor_line(
            child_tokens,
            child_line.text,
            child_line.start,
            false,
        );
        if let ChildTaskOutcome::Accept { id, stripped, .. } = pass.check_child(
            &child_parse.body,
            authored.depth,
            child.line_number,
        ) && index != line_index
        {
            used.push(ProjectTaskUsedId {
                id,
                line: child.line_number,
                text: clean_project_task_body(&stripped),
            });
        }
    }
    // The cursor line's body without its trailing token or checkbox. The
    // accepted-ID strip and the plain last-word strip coincide, so one
    // path covers valid, invalid, and lone-sigil tokens alike.
    let current = &item.lines[line_index];
    let AuthoredLineClass::Item(current_authored) =
        classify_authored_line(current.raw)
    else {
        return None;
    };
    let current_line = RawLine {
        text: current_authored.body,
        start: current_authored.body_start,
        end: current.raw.end,
    };
    let current_tokens = tokenize_line_with_spans(&current_line);
    let current_parse = parse_editor_line(
        current_tokens,
        current_line.text,
        current_line.start,
        false,
    );
    let stripped = strip_task_id_suffix(&current_parse.body);
    Some(ProjectTaskBlockIdDetail {
        stem: info.stem,
        marker,
        marker_range: (info.token.start, info.token.end),
        body: clean_project_task_body(&stripped),
        used,
    })
}

/// Bullet body without a leading checkbox, for suggestion input and used
/// text. Mirrors the renderer's checkbox shape through the shared helper.
fn clean_project_task_body(stripped: &str) -> String {
    let (_, rest) = split_leading_checkbox(stripped.trim_start());
    rest.trim().to_string()
}

pub(super) fn has_previous_first_level_authored_item(
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
        !parse_editor_line(tokens, &normalized, 0, false)
            .body
            .is_empty()
    })
}

/// Mirror [`select_marker_token`]'s leading-then-trailing precedence, but
/// without its requires-body/single-token exclusion: a lone leading
/// `@route` fragment with no body text yet is still the token a user is
/// actively completing, even though `bob capture` would leave it literal.
/// `leading` is only ever set for the parent (first) physical line.
pub(super) fn completion_marker_index(
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
pub(super) fn marker_field_at_cursor(
    token: &Token<'_>,
    cursor: usize,
    sub_bullet_is_toggle: bool,
) -> Option<CompletionField> {
    let text = token.text;

    if is_sub_bullet_marker_candidate(text) {
        if let Some(prefix) = exact_explicit_toggle_prefix(text) {
            let bang_start = token.start + prefix.len();
            if cursor > bang_start {
                return None;
            }
        }
        let text = exact_explicit_toggle_prefix(text).unwrap_or(text);
        let marker = &text[1..];
        let (route_part, rest) =
            marker.split_once('+').expect("sub-bullet candidate");
        let third_context = if sub_bullet_is_toggle {
            CompletionContext::PomodoroName
        } else {
            CompletionContext::TaskSection
        };
        let (block_part, third) = match rest.split_once('#') {
            Some((block, section)) => (
                block,
                Some(CompletionThird {
                    separator_len: 1,
                    part: section,
                    context: third_context,
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
        // A `#name` after `@route^id+` completes the Pomodoro name that
        // picks the ` :` links' Pomodoro; the replacement is just the name.
        let (block_part, third) = match block_part.split_once('#') {
            Some((block, name)) => (
                block,
                Some(CompletionThird {
                    separator_len: 1,
                    part: name,
                    context: CompletionContext::PomodoroName,
                }),
            ),
            None => (block_part, None),
        };
        return completion_field_from_parts(
            token,
            CompletionParts {
                sigil_len: 1,
                route_part,
                separator_len: 1,
                right_part: block_part,
                right_context: Some(CompletionContext::TaskBlockId),
                third,
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
        let (route_part, rest, separator) = match marker.split_once(':') {
            Some((route, rest)) => (route, rest, true),
            None => (marker, "", false),
        };
        // The additive `=<X>` start or `=x` close suffix is never a
        // completable component: strip it before splitting `#` so the
        // block and name parts — and their replacement ranges — end
        // before `=`. A cursor inside the suffix offers no completion;
        // accepting a name candidate must leave the typed suffix in place.
        let (rest, start_len) = match rest.split_once('=') {
            Some((before, suffix)) => (before, Some(1 + suffix.len())),
            None => (rest, None),
        };
        if let Some(start_len) = start_len
            && cursor > token.end - start_len
        {
            return None;
        }
        let (block_part, third) = match rest.split_once('#') {
            Some((block, name)) => (
                block,
                Some(CompletionThird {
                    separator_len: 1,
                    part: name,
                    context: CompletionContext::PomodoroName,
                }),
            ),
            None => (rest, None),
        };
        // The retired `:` project-note form (`@route:id+`, with or without
        // a session suffix) offers no completion.
        if block_part
            .strip_suffix('+')
            .is_some_and(|core| !core.is_empty())
        {
            return None;
        }
        return completion_field_from_parts(
            token,
            CompletionParts {
                sigil_len,
                route_part,
                separator_len: usize::from(separator),
                right_part: block_part,
                right_context: Some(CompletionContext::PomodoroBlockId),
                third,
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

pub(super) fn global_completion_field_at(
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
pub(super) fn completion_field_from_parts(
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
    // A single trailing `+` immediately after the block-ID part is the
    // project-note sigil. It is never part of the block replacement: a
    // cursor inside the ID completes the ID, while a cursor just after the
    // sigil (and before any `#`) is an empty success. A `+` inside a
    // Pomodoro name stays ordinary name charset.
    let right_is_block_id = matches!(
        parts.right_context,
        Some(CompletionContext::PomodoroBlockId)
            | Some(CompletionContext::TaskBlockId)
    );
    let (right_core, has_plus) = if right_is_block_id {
        match parts.right_part.strip_suffix('+') {
            Some(core) => (core, true),
            None => (parts.right_part, false),
        }
    } else {
        (parts.right_part, false)
    };
    let core_end = right_start + right_core.len();
    if has_plus {
        if cursor <= core_end {
            let right_context = parts.right_context?;
            if !is_route_token(parts.route_part) {
                return None;
            }
            let split = cursor.clamp(right_start, core_end) - right_start;
            return Some(CompletionField {
                context: right_context,
                route: Some(parts.route_part.to_ascii_lowercase()),
                block_id: None,
                query: right_core[..split].to_string(),
                replacement: (right_start, core_end),
            });
        }
        if cursor <= right_end {
            return None;
        }
    }
    let in_right = parts.third.is_none() || cursor <= right_end;
    if in_right {
        // Past the first separator: complete the middle component when that
        // component is backed by a discovery source.
        let right_context = parts.right_context?;
        // The right-hand component only makes sense once the route it
        // belongs to already resolves.
        if !is_route_token(parts.route_part) {
            return None;
        }
        if has_plus {
            let split = cursor.clamp(right_start, core_end) - right_start;
            return Some(CompletionField {
                context: right_context,
                route: Some(parts.route_part.to_ascii_lowercase()),
                block_id: None,
                query: right_core[..split].to_string(),
                replacement: (right_start, core_end),
            });
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
    // Strip the project-note sigil from the block ID handed to Pomodoro-name
    // completion, so `@route^id+#name` reports `id` rather than `id+`.
    let third_block = if right_is_block_id {
        right_core
    } else {
        parts.right_part
    };
    Some(CompletionField {
        context: third.context,
        route: Some(parts.route_part.to_ascii_lowercase()),
        block_id: (!third_block.is_empty()).then(|| third_block.to_string()),
        query: third.part[..split].to_string(),
        replacement: (third_start, third_end),
    })
}

// ---------------------------------------------------------------------------
// Rule A1-A6: `bob capture-rewrite`'s bare `@@` absorption
// ---------------------------------------------------------------------------
