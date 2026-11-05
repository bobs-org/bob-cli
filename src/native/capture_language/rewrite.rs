//! Draft absorption and text edits.

use super::draft::*;
use super::editor_model::*;
use super::editor_parse::*;
use super::markers::is_route_token;
use super::model::*;
use super::project_tasks::{lex_project_task_id, TaskIdLex};
use super::tokens::is_block_id;

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
    SwitchBlockIdSeparator,
    AttachRelationLabel,
    ExpandSigilAlias,
}

impl RewriteRule {
    pub(crate) fn code(self) -> &'static str {
        match self {
            Self::AbsorbLocalMarker => "absorb_local_marker",
            Self::AbsorbDeclaration => "absorb_declaration",
            Self::SwitchBlockIdSeparator => "switch_block_id_separator",
            Self::AttachRelationLabel => "attach_relation_label",
            Self::ExpandSigilAlias => "expand_sigil_alias",
        }
    }
}

/// Apply the capture grammar's editor typing assists to `raw_text`.
///
/// With an explicit `cursor`, a complete plain `@route:id` / `@route^id` /
/// `@route+id` marker that just grew a supported separator (`^`, `:`, or
/// `+`) is rewritten to the trigger's spelling and the appended trigger is
/// dropped. A `+` after `@route^id` switches to `@route+id`, like every
/// other separator. That rule never searches the draft: no cursor means
/// absorption-only, matching the historical `@@` contract.
///
/// Apply Rule A1's absorption to the bare `@@` at (or before, when
/// no cursor is given, the last one in source order at) `cursor`: claim the
/// item's own absorbable local destination marker, or else the draft's one
/// other declaration token, rewriting the bare token to `@@<payload>` and
/// deleting the token(s) it absorbed. Never fails; an input with no eligible
/// bare `@@` -- or whose local marker cannot be expressed as a declaration
/// (Rule A5), or whose item already has more than one local marker
/// (Rule A6) -- returns `rule: None` with `text` unchanged.
pub(crate) fn rewrite_draft(
    raw_text: &str,
    cursor: Option<usize>,
) -> DraftRewrite {
    if super::template::analyze(raw_text).directive.is_some() {
        return unchanged_rewrite(raw_text, cursor, Vec::new());
    }
    if let Some(position) = cursor
        && let Some(rewrite) =
            switch_task_bullet_id_separator(raw_text, position)
    {
        return rewrite;
    }
    if let Some(position) = cursor
        && let Some(rewrite) = switch_block_id_separator(raw_text, position)
    {
        return rewrite;
    }
    if let Some(position) = cursor
        && let Some(rewrite) = attach_relation_label(raw_text, position)
    {
        return rewrite;
    }
    if let Some(position) = cursor
        && let Some(rewrite) = expand_sigil_alias(raw_text, position)
    {
        return rewrite;
    }

    absorb_bare_declaration(raw_text, cursor)
}

/// Typing `.`, `;`, `,`, or `\` in a position where its real sigil would open
/// a picker swaps that byte to `^`, `:`, `+`, or `#`. The candidate is checked
/// by the ordinary completion grammar, keeping the assist lexical and aligned
/// with picker eligibility. Route-scope contexts plus lone move-bullet
/// sigils are accepted; a `.` that completes a `- ^^` choice marker is also
/// accepted. A comma glued to a complete target-run member instead inserts
/// ` +` to open the next parent-task picker.
fn expand_sigil_alias(raw_text: &str, cursor: usize) -> Option<DraftRewrite> {
    let start = cursor.checked_sub(1)?;
    let alias = raw_text.get(start..cursor)?;
    if alias == "\\" {
        return expand_hash_alias(raw_text, start, cursor);
    }
    let token = tokenize_with_spans(raw_text)
        .into_iter()
        .find(|token| token.end == cursor)?;
    if alias == "," {
        return expand_parent_task_comma_alias(raw_text, start, cursor, token);
    }
    let (sigil, allowed_context) = match alias {
        "." => ('^', super::completion::CompletionContext::TaskBlockId),
        ";" => (':', super::completion::CompletionContext::PomodoroBlockId),
        _ => return None,
    };
    if token.end != cursor {
        return None;
    }
    let mut candidate = raw_text.to_string();
    candidate.replace_range(start..cursor, &sigil.to_string());
    if let Some(field) =
        super::completion::completion_field_at(&candidate, cursor)
    {
        let allowed =
            match alias {
                "." => (field.context == allowed_context
                    && field.replacement.0 == cursor)
                    || (field.context
                        == super::completion::CompletionContext::ProjectTaskMove
                        && field.replacement == (cursor, cursor)),
                ";" if field.context == allowed_context => {
                    field.replacement.0 == cursor
                }
                ";" => (field.context
                    == super::completion::CompletionContext::TaskLink
                    && field.replacement == (start, cursor))
                    || (field.context
                        == super::completion::CompletionContext::ProjectTaskMove
                        && field.replacement == (cursor, cursor)),
                _ => false,
            };
        if allowed {
            return Some(finish_sigil_alias(
                raw_text, &candidate, start, cursor, sigil, alias,
            ));
        }
    }
    // Choice alias: `.` that completes a `- ^^` marker while the picker is
    // closed. `;` never aliases here: `- ::` is the `move_choice_colon`
    // teaching error. Lexical only: the candidate line must be a first-level
    // bullet in a sealed project-note item whose move scan is a Choice
    // ending at the cursor.
    if alias != "." {
        return None;
    }
    let draft = split_capture_draft(&candidate);
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
    let parsed = parse_editor_item(item).item;
    if !super::completion::has_sealed_project_note_marker(&parsed) {
        return None;
    }
    let AuthoredLineClass::Item(authored) = classify_authored_line(line) else {
        return None;
    };
    if authored.depth != AuthoredDepth::First {
        return None;
    }
    match super::move_bullet::scan_move_bullet(
        authored.body,
        authored.body_start,
        &candidate,
    )? {
        super::move_bullet::MoveBulletHit::Choice { span }
            if span.end == cursor => {}
        _ => return None,
    }
    Some(finish_sigil_alias(
        raw_text, &candidate, start, cursor, sigil, alias,
    ))
}

/// Typing `,` where `+` would open a Parent Task picker aliases it to `+`.
/// A comma glued to the end of a complete target-run member instead becomes
/// ` +`, advancing to the next target without changing the capture grammar.
fn expand_parent_task_comma_alias(
    raw_text: &str,
    start: usize,
    cursor: usize,
    token: Token<'_>,
) -> Option<DraftRewrite> {
    if token.end != cursor {
        return None;
    }

    // In-place forms are accepted only when replacing the comma with `+`
    // opens the ordinary note-scoped task picker or a whole-token parent
    // picker. This keeps commas in prose, names, and close/start lists literal.
    let mut swapped = raw_text.to_string();
    swapped.replace_range(start..cursor, "+");
    if let Some(field) =
        super::completion::completion_field_at(&swapped, cursor)
    {
        use super::completion::CompletionContext;
        let allowed = match field.context {
            CompletionContext::Task => field.replacement.0 == cursor,
            CompletionContext::TaskParent => {
                field.replacement == (start, cursor)
            }
            _ => false,
        };
        if allowed {
            return Some(finish_sigil_alias(
                raw_text, &swapped, start, cursor, '+', ",",
            ));
        }
    }

    // A glued comma may continue a run only when the editor parser identifies
    // a complete target member ending exactly before it. Transfer slots and
    // drawer suffixes have different span kinds and cannot pass this gate.
    if token.start >= start {
        return None;
    }
    let mut stripped = raw_text.to_string();
    stripped.replace_range(start..cursor, "");
    let parsed = parse_for_editor(&stripped);
    if !parsed.spans.iter().any(|span| {
        span.end == start
            && matches!(
                span.kind,
                SpanKind::SubBulletBlockId
                    | SpanKind::TaskToggleBlockId
                    | SpanKind::TaskToggleExplicitToggle
            )
    }) {
        return None;
    }

    let mut continued = raw_text.to_string();
    continued.replace_range(start..cursor, " +");
    let continued_cursor = start + 2;
    let field =
        super::completion::completion_field_at(&continued, continued_cursor)?;
    if field.context != super::completion::CompletionContext::TaskParent
        || field.replacement != (start + 1, continued_cursor)
    {
        return None;
    }

    let edits = vec![TextEdit {
        start,
        end: cursor,
        replacement: " +".to_string(),
    }];
    Some(DraftRewrite {
        rule: Some(RewriteRule::ExpandSigilAlias),
        edits,
        text: continued,
        cursor: Some(continued_cursor),
        summary: Some("Typed + for ,".to_string()),
        notices: Vec::new(),
    })
}

/// Typing `\` where a real `#` would open a completion menu swaps that byte
/// to `#`. A `\` with only whitespace before it on its physical line never
/// aliases, so a blank draft keeps opening Go To and a later line's leading
/// `\` stays literal. The candidate is checked by the ordinary completion
/// grammar: wikilink headings first (matching the engine's precedence), then
/// the marker grammar's `#` menus.
fn expand_hash_alias(
    raw_text: &str,
    start: usize,
    cursor: usize,
) -> Option<DraftRewrite> {
    let line_start = raw_text[..start].rfind('\n').map_or(0, |index| index + 1);
    if raw_text
        .get(line_start..start)
        .is_some_and(|prefix| prefix.chars().all(|c| c == ' ' || c == '\t'))
    {
        return None;
    }
    if tokenize_with_spans(raw_text)
        .into_iter()
        .all(|token| token.end != cursor)
    {
        return None;
    }
    let mut candidate = raw_text.to_string();
    candidate.replace_range(start..cursor, "#");
    if let Some(field) = crate::native::capture_links::completion_field_at(
        &candidate,
        cursor,
        String::new(),
    ) {
        use super::completion::CompletionContext;
        if field.context == CompletionContext::WikilinkHeading
            && field.replacement.0 == cursor
        {
            return Some(finish_sigil_alias(
                raw_text, &candidate, start, cursor, '#', "\\",
            ));
        }
        return None;
    }
    let field = super::completion::completion_field_at(&candidate, cursor)?;
    use super::completion::CompletionContext;
    let allowed = match field.context {
        CompletionContext::TaskType | CompletionContext::Drawer => {
            field.replacement == (start, cursor)
        }
        CompletionContext::Section
        | CompletionContext::PomodoroName
        | CompletionContext::PomodoroPastName
        | CompletionContext::PomodoroSplitType
        | CompletionContext::PomodoroMergeDest => field.replacement.0 == cursor,
        CompletionContext::PomodoroStartName => {
            field.replacement.0 == cursor
                || field.replacement == (start, cursor)
        }
        _ => false,
    };
    if !allowed {
        return None;
    }
    Some(finish_sigil_alias(
        raw_text, &candidate, start, cursor, '#', "\\",
    ))
}

fn finish_sigil_alias(
    raw_text: &str,
    candidate: &str,
    start: usize,
    cursor: usize,
    sigil: char,
    alias: &str,
) -> DraftRewrite {
    let replacement = sigil.to_string();
    let edits = vec![TextEdit {
        start,
        end: cursor,
        replacement: replacement.clone(),
    }];
    let text = apply_text_edits(raw_text, &edits);
    debug_assert_eq!(text, candidate);
    DraftRewrite {
        rule: Some(RewriteRule::ExpandSigilAlias),
        edits,
        text,
        cursor: Some(cursor),
        summary: Some(format!("Typed {sigil} for {alias}")),
        notices: Vec::new(),
    }
}

fn absorb_bare_declaration(
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

    // A `>` transfer never absorbs `@@` — its destination is not a
    // declaration payload — so a bare `@@` on the transfer's own line is
    // refused here. Transfer items surface no declarations, so the normal
    // selection below cannot see these tokens.
    if let Some(rewrite) = transfer_bare_at_at_notice(
        &draft,
        &item_outcomes,
        &occurrences,
        raw_text,
        cursor,
    ) {
        return rewrite;
    }

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
            [] => {
                // A `%n` Pomodoro-note item owns no local destination
                // marker, but a bare `@@` still cannot absorb it.
                if item_outcomes[item_index].item.mode
                    == EditorMode::PomodoroNote
                {
                    return unchanged_rewrite(
                        raw_text,
                        cursor,
                        vec![format!(
                            "@@ cannot take a Pomodoro note: leave %n on this item, or delete it"
                        )],
                    );
                }
            }
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
                            vec![non_absorbable_marker_notice(
                                marker,
                                &item_outcomes[item_index].item,
                            )],
                        )
                    }
                };
            }
            // Rule A6: two or more local markers already put the draft in a
            // duplicate-marker error state that `capture-parse` reports. A
            // target run is the one exception: its members share one route
            // slot, so the run gets the existing non-absorbable-marker
            // notice instead of the silent no-op.
            _ => {
                let item = &item_outcomes[item_index].item;
                if !item.targets.is_empty()
                    && let Some(marker) = item.local_destination_markers.first()
                {
                    return unchanged_rewrite(
                        raw_text,
                        cursor,
                        vec![non_absorbable_marker_notice(marker, item)],
                    );
                }
                return unchanged_rewrite(raw_text, cursor, Vec::new());
            }
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

/// Typing the other task-ID sigil immediately after a complete ` :id` /
/// ` ^id` on a list or project-note task bullet switches the marker and
/// consumes the trigger. The candidate is reparsed so ordinary prose that
/// happens to end in a task-ID-shaped token is never rewritten.
fn switch_task_bullet_id_separator(
    raw_text: &str,
    cursor: usize,
) -> Option<DraftRewrite> {
    let token = tokenize_with_spans(raw_text)
        .into_iter()
        .find(|token| token.end == cursor)?;
    let trigger = *token.text.as_bytes().last()?;
    let new_sigil = match trigger {
        b':' => ':',
        b'^' => '^',
        _ => return None,
    };
    let current = token.text.get(..token.text.len().checked_sub(1)?)?;
    let TaskIdLex::Valid { sigil, id } = lex_project_task_id(current)? else {
        return None;
    };
    if sigil == new_sigil {
        return None;
    }
    let replacement = format!("{new_sigil}{id}");
    let mut candidate = String::with_capacity(raw_text.len() - 1);
    candidate.push_str(&raw_text[..token.start]);
    candidate.push_str(&replacement);
    candidate.push_str(&raw_text[token.end..]);
    if !candidate_selects_task_bullet_id(
        &candidate,
        token.start,
        token.start + replacement.len(),
        id.as_str(),
        new_sigil,
    ) {
        return None;
    }
    let edits = vec![TextEdit {
        start: token.start,
        end: token.end,
        replacement: replacement.clone(),
    }];
    let text = apply_text_edits(raw_text, &edits);
    Some(DraftRewrite {
        rule: Some(RewriteRule::SwitchBlockIdSeparator),
        edits,
        text,
        cursor: Some(token.start + replacement.len()),
        summary: Some(format!("Changed {current} to {replacement}")),
        notices: Vec::new(),
    })
}

fn candidate_selects_task_bullet_id(
    candidate: &str,
    start: usize,
    end: usize,
    id: &str,
    sigil: char,
) -> bool {
    let draft = split_capture_draft(candidate);
    let Some(item) = draft.items.iter().find(|item| {
        item.lines
            .iter()
            .any(|line| line.raw.start <= start && end <= line.raw.end)
    }) else {
        return false;
    };
    let Some(position) = item
        .lines
        .iter()
        .position(|line| line.raw.start <= start && end <= line.raw.end)
    else {
        return false;
    };
    let parsed = parse_editor_item(item).item;
    if !parsed.diagnostics.is_empty() {
        return false;
    }
    if item.list.is_some() {
        let expected_mode = if sigil == ':' {
            EditorMode::PomodoroTask
        } else {
            EditorMode::Task
        };
        return item.list.as_ref().is_some_and(|list| {
            list.kind == ListKind::Tasks && !list.ownerless
        }) && parsed.mode == expected_mode
            && parsed.block_id.as_deref() == Some(id);
    }
    if position == 0
        || !matches!(
            parsed.mode,
            EditorMode::ProjectNote | EditorMode::PomodoroProjectNote
        )
    {
        return false;
    }
    let has_id = parsed.spans.iter().any(|span| {
        span.kind == SpanKind::ProjectTaskBlockId
            && (span.start, span.end) == (start + 1, end)
    });
    let has_link = parsed.spans.iter().any(|span| {
        span.kind == SpanKind::ProjectTaskLinkMarker && span.start == start
    });
    has_id && has_link == (sigil == ':')
}

/// Typing `^` immediately after a complete plain `@route:id` or `@route+id`,
/// `:` after `@route^id` or `@route+id`, or `+` after `@route:id` consumes
/// that trigger and switches the separator. Typing `:` right after a sealed
/// `@route^id@` gives `@route:id@`, and `^` after `@route:id@` gives
/// `@route^id@`. The token must end at `cursor`; the stripped remainder
/// must be the line's selected destination marker, complete, and free of
/// `#`/`=`/`!` suffixes. The same separator, or `+`, after a seal stays
/// literal.
fn switch_block_id_separator(
    raw_text: &str,
    cursor: usize,
) -> Option<DraftRewrite> {
    let token = tokenize_with_spans(raw_text)
        .into_iter()
        .find(|token| token.end == cursor)?;
    let trigger = *token.text.as_bytes().last()?;
    let trigger_char = match trigger {
        b':' => ':',
        b'^' => '^',
        b'+' => '+',
        _ => return None,
    };
    let stripped_end = token.end - 1;
    if stripped_end <= token.start || !raw_text.is_char_boundary(stripped_end) {
        return None;
    }
    let token_start = token.start;
    let token_end = token.end;
    let stripped = &raw_text[token_start..stripped_end];
    let (separator, route, id, sealed) = plain_block_id_parts(stripped)?;
    let eligible = match trigger_char {
        // After a seal only the other sealed separator switches; the
        // same separator (or `+`) stays literal.
        ':' => matches!(separator, '^' | '+') && (!sealed || separator == '^'),
        '^' => matches!(separator, ':' | '+') && (!sealed || separator == ':'),
        // `+` after `@route^id` switches to `@route+id`, like every other
        // separator. `+` after a sealed marker stays literal.
        '+' => matches!(separator, ':' | '^') && !sealed,
        _ => false,
    };
    if !eligible {
        return None;
    }

    // Transfer slots (`>` sources and destinations) only take `+id`:
    // typing `+` after `@route:id` or `@route^id` there switches to the
    // transfer spelling. `:`/`^` triggers stay literal (they would break
    // the transfer), and the ordinary marker check below declines them.
    if !sealed
        && trigger_char == '+'
        && matches!(separator, ':' | '^')
        && let Some(rewrite) = switch_transfer_block_id_separator(
            raw_text,
            token_start,
            token_end,
            stripped,
            route,
            id,
        )
    {
        return Some(rewrite);
    }

    let mut candidate = String::with_capacity(raw_text.len() - 1);
    candidate.push_str(&raw_text[..stripped_end]);
    candidate.push_str(&raw_text[token_end..]);
    if !candidate_selects_plain_marker(
        &candidate,
        token_start,
        stripped_end,
        stripped,
        separator,
        sealed,
        trigger_char,
        route,
        id,
    ) {
        return None;
    }

    let replacement = if sealed {
        format!("@{route}{trigger_char}{id}@")
    } else {
        format!("@{route}{trigger_char}{id}")
    };
    let summary = format!("Changed {stripped} to {replacement}");
    let edits = vec![TextEdit {
        start: token_start,
        end: token_end,
        replacement: replacement.clone(),
    }];
    let text = apply_text_edits(raw_text, &edits);
    Some(DraftRewrite {
        rule: Some(RewriteRule::SwitchBlockIdSeparator),
        edits,
        text,
        cursor: Some(token_start + replacement.len()),
        summary: Some(summary),
        notices: Vec::new(),
    })
}

/// Typing `+` after `@route:id` or `@route^id` in a transfer source or
/// destination slot switches to `@route+id`. Eligibility is decided on the
/// switched draft (same length: one separator swaps for another): the token
/// must land exactly on a transfer source or destination there. Anything
/// else — prose, ordinary markers, partial destinations that stay partial
/// — falls through to the ordinary marker check.
fn switch_transfer_block_id_separator(
    raw_text: &str,
    token_start: usize,
    token_end: usize,
    stripped: &str,
    route: &str,
    id: &str,
) -> Option<DraftRewrite> {
    let switched_marker = format!("@{route}+{id}");
    let mut switched = String::with_capacity(raw_text.len());
    switched.push_str(raw_text.get(..token_start)?);
    switched.push_str(&switched_marker);
    switched.push_str(raw_text.get(token_end..)?);
    let switched_end = token_start + switched_marker.len();
    let draft = split_capture_draft(&switched);
    for item in &draft.items {
        for line in &item.lines {
            if !(line.raw.start <= token_start && switched_end <= line.raw.end)
            {
                continue;
            }
            let scan = super::transfer::scan_transfer_line(
                &line.raw,
                &switched,
                item.lines.len() > 1,
            );
            let covers = match scan {
                super::transfer::TransferScan::Valid {
                    sources,
                    destination,
                    ..
                } => {
                    sources.iter().any(|source| {
                        source.start == token_start
                            && source.end == switched_end
                    }) || (destination.start == token_start
                        && destination.end == switched_end)
                }
                _ => false,
            };
            if covers {
                let summary =
                    format!("Changed {stripped} to {switched_marker}");
                let edits = vec![TextEdit {
                    start: token_start,
                    end: token_end,
                    replacement: switched_marker.clone(),
                }];
                let text = apply_text_edits(raw_text, &edits);
                return Some(DraftRewrite {
                    rule: Some(RewriteRule::SwitchBlockIdSeparator),
                    edits,
                    text,
                    cursor: Some(token_start + switched_marker.len()),
                    summary: Some(summary),
                    notices: Vec::new(),
                });
            }
        }
    }
    None
}

/// Typing `|` (or `||`) after whitespace that follows a relation token
/// glues the pipe(s) to the token, so `~note:id |` becomes `~note:id|`
/// and `~note:id ||` becomes `~note:id||`. Only the whitespace-delimited
/// `|` / `||` token ending at `cursor` is examined. The run before it
/// must be non-empty horizontal whitespace (spaces or tabs, never
/// a newline) with a non-whitespace byte on the same physical line before
/// it. Eligibility is delegated to the completion grammar: the run is
/// deleted only when the glued draft offers a `relation_label` field whose
/// replacement starts where the run did. Without a cursor the rule is
/// skipped, matching `switch_block_id_separator`.
fn attach_relation_label(
    raw_text: &str,
    cursor: usize,
) -> Option<DraftRewrite> {
    let token = tokenize_with_spans(raw_text)
        .into_iter()
        .find(|token| token.end == cursor)?;
    if token.text != "|" && token.text != "||" {
        return None;
    }
    let pipe = token.start;
    let mut run_start = pipe;
    while run_start > 0
        && matches!(raw_text.as_bytes()[run_start - 1], b' ' | b'\t')
    {
        run_start -= 1;
    }
    if run_start == pipe || !raw_text.is_char_boundary(run_start) {
        return None;
    }
    if !raw_text[..run_start]
        .chars()
        .next_back()
        .is_some_and(|before| !before.is_whitespace())
    {
        return None;
    }
    let mut candidate = String::with_capacity(raw_text.len());
    candidate.push_str(&raw_text[..run_start]);
    candidate.push_str(&raw_text[pipe..]);
    let candidate_cursor = cursor - (pipe - run_start);
    let field =
        super::completion::completion_field_at(&candidate, candidate_cursor)?;
    if field.context != super::completion::CompletionContext::RelationLabel
        || field.replacement.0 != run_start
    {
        return None;
    }
    let (token_start, ..) =
        super::completion::relation_label_token_at(&candidate, run_start)?;
    let token_text = candidate.get(token_start..run_start)?;
    let edits = vec![TextEdit {
        start: run_start,
        end: pipe,
        replacement: String::new(),
    }];
    let text = apply_text_edits(raw_text, &edits);
    debug_assert_eq!(text, candidate);
    Some(DraftRewrite {
        rule: Some(RewriteRule::AttachRelationLabel),
        edits,
        text,
        cursor: Some(candidate_cursor),
        summary: Some(format!("Attached {} to {token_text}", token.text)),
        notices: Vec::new(),
    })
}

/// A complete plain `@route:id`, `@route^id`, or `@route+id` token —
/// or its sealed project-note form with one trailing `@` (`@route^id@`,
/// `@route:id@`): one `@`, exactly one separator, a valid route, a valid
/// block ID, and no suffix family. The returned flag reports the seal.
fn plain_block_id_parts(token: &str) -> Option<(char, &str, &str, bool)> {
    let rest = token.strip_prefix('@')?;
    if token.starts_with("@@") || token.starts_with("@!") {
        return None;
    }
    if rest.contains('#') || rest.contains('=') || rest.contains('!') {
        return None;
    }
    let mut separators = rest.match_indices([':', '^', '+']);
    let (index, separator_text) = separators.next()?;
    if separators.next().is_some() {
        return None;
    }
    let separator = separator_text.chars().next()?;
    let route = &rest[..index];
    let raw_id = &rest[index + separator_text.len()..];
    // One trailing `@` is the project-note seal; anything else holding
    // `@` is not a plain marker.
    let (id, sealed) = match raw_id.strip_suffix('@') {
        Some(core) => (core, true),
        None if raw_id.contains('@') => return None,
        None => (raw_id, false),
    };
    (is_route_token(route) && is_block_id(id))
        .then_some((separator, route, id, sealed))
}

fn candidate_selects_plain_marker(
    candidate: &str,
    start: usize,
    end: usize,
    stripped: &str,
    separator: char,
    sealed: bool,
    trigger: char,
    route_part: &str,
    id_part: &str,
) -> bool {
    let Some((_, route, block_id, _)) = plain_block_id_parts(stripped) else {
        return false;
    };
    let draft = split_capture_draft(candidate);
    for item in &draft.items {
        for (position, line) in item.lines.iter().enumerate() {
            if !(line.raw.start <= start && end <= line.raw.end) {
                continue;
            }
            let leading = position == 0;
            let (line_text, line_base, line_end) = if leading {
                (line.raw.text, line.raw.start, line.raw.end)
            } else {
                match classify_authored_line(line.raw) {
                    AuthoredLineClass::Item(authored) => {
                        (authored.body, authored.body_start, line.raw.end)
                    }
                    _ => return false,
                }
            };
            if start < line_base || end > line_end {
                return false;
            }
            if item.list.is_some() {
                if sealed || !matches!(separator, ':' | '^') {
                    return false;
                }
                let parsed = parse_editor_item(item).item;
                if !parsed.diagnostics.is_empty() {
                    return false;
                }
                let expected_mode = if separator == ':' {
                    EditorMode::PomodoroTask
                } else {
                    EditorMode::Task
                };
                return parsed.mode == expected_mode
                    && parsed.block_id.as_deref() == Some(block_id)
                    && parsed.route.as_deref() == Some(route)
                    && parsed.section.is_none();
            }
            let parsed = (position > 0).then(|| parse_editor_item(item).item);
            if let Some(parsed) = parsed.as_ref().filter(|parsed| {
                matches!(
                    parsed.mode,
                    EditorMode::ProjectNote | EditorMode::PomodoroProjectNote
                )
            }) {
                if sealed
                    || !matches!(separator, ':' | '^')
                    || !parsed.diagnostics.is_empty()
                {
                    return false;
                }
                let id_range = (start + 1, end);
                let project_task = matches!(
                    parsed.mode,
                    EditorMode::ProjectNote | EditorMode::PomodoroProjectNote
                ) && parsed.spans.iter().any(|span| {
                    span.kind == SpanKind::ProjectTaskBlockId
                        && (span.start, span.end) == id_range
                });
                let link = parsed.spans.iter().any(|span| {
                    span.kind == SpanKind::ProjectTaskLinkMarker
                        && span.start == start
                });
                return project_task && link == (separator == ':');
            }
            // A token that, once switched, becomes a member of a target run
            // is eligible, even though the stripped form is not the line's
            // single selected marker. Build the switched draft (same byte
            // length: one separator char replaces another) and ask the
            // editor's run probe whether the switched token joins a run.
            {
                let switched_marker = if sealed {
                    format!("@{route_part}{trigger}{id_part}@")
                } else {
                    format!("@{route_part}{trigger}{id_part}")
                };
                let mut switched = String::with_capacity(candidate.len());
                switched.push_str(candidate.get(..start).unwrap_or(""));
                switched.push_str(&switched_marker);
                switched.push_str(candidate.get(end..).unwrap_or(""));
                let switched_end = start + switched_marker.len();
                let switched_draft = split_capture_draft(&switched);
                for switched_item in &switched_draft.items {
                    let contains = switched_item.lines.iter().any(|line| {
                        line.raw.start <= start && switched_end <= line.raw.end
                    });
                    if !contains {
                        continue;
                    }
                    let (run, _) =
                        super::item::find_editor_target_run(switched_item);
                    if let Some(run) = run
                        && run.selection.members.iter().any(|member| {
                            member.start == start && member.end == switched_end
                        })
                    {
                        return true;
                    }
                }
            }
            // Type markers are transparent to the toggle: mask them so a
            // neighboring `#<type>` never blocks an otherwise plain marker.
            // Masking preserves byte lengths, so every span still indexes
            // the original draft.
            let masked = super::type_marker::masked_line(&RawLine {
                text: line_text,
                start: line_base,
                end: line_end,
            });
            let raw_line = RawLine {
                text: &masked,
                start: line_base,
                end: line_end,
            };
            let tokens = tokenize_line_with_spans(&raw_line);
            let parse =
                parse_editor_line(tokens, line_text, line_base, leading, false);
            let Some(marker) = parse.marker.as_ref() else {
                return false;
            };
            if parse.marker_text.as_deref() != Some(stripped) {
                return false;
            }
            let mode_matches = match (separator, sealed) {
                ('^', false) => marker.mode == EditorMode::Task,
                (':', false) => marker.mode == EditorMode::PomodoroTask,
                // A sealed candidate selects the matching project-note
                // mode with no needs.
                ('^', true) => marker.mode == EditorMode::ProjectNote,
                (':', true) => marker.mode == EditorMode::PomodoroProjectNote,
                ('+', false) => matches!(
                    marker.mode,
                    EditorMode::SubBullet | EditorMode::TaskToggle
                ),
                _ => false,
            };
            if !mode_matches
                || !marker.needs.is_empty()
                || marker.section.is_some()
                || marker.pomodoro_start.is_some()
                || marker.pomodoro_close.is_some()
                || marker.route.is_none()
                || marker.block_id.is_none()
            {
                return false;
            }
            let Some(first) = marker.spans.first() else {
                return false;
            };
            let Some(last) = marker.spans.last() else {
                return false;
            };
            return first.start == start && last.end == end;
        }
    }
    false
}

pub(super) fn unchanged_rewrite(
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
pub(super) struct DeclarationOccurrence<'a> {
    pub(super) token: Token<'a>,
    pub(super) owner_item: Option<usize>,
}

/// Select the bare `@@` Rule A1 absorbs into: the one containing or ending
/// at `cursor` when a cursor is given, otherwise the last one in source
/// order. Returns an index into `occurrences`.
pub(super) fn select_bare_declaration(
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

/// The destination-shaped token after a standalone `>` on one parent
/// line, for notices on lines the transfer scan refuses (a trailing `@@`
/// is `transfer_extra_text`, so the scan carries no destination span).
/// Returns `None` when the arrow is glued, escaped, or missing.
fn transfer_destination_token_text<'a>(
    parent: &'a RawLine<'a>,
) -> Option<&'a str> {
    let mut tokens = tokenize_with_spans(parent.text).into_iter();
    loop {
        let token = tokens.next()?;
        if token.text == ">" {
            let next = tokens.next()?;
            return next.text.starts_with('@').then_some(next.text);
        }
    }
}

/// Refuse a bare `@@` typed on a `>` transfer's own line: the transfer
/// destination cannot become a declaration payload. Transfer items surface
/// no declarations, so the normal selection cannot claim these tokens;
/// this mirrors its claiming rule instead — the `@@` containing the
/// cursor, else the last bare `@@` in the draft — and reports the same
/// copy as every other non-absorbable marker.
fn transfer_bare_at_at_notice(
    draft: &CaptureDraft<'_>,
    item_outcomes: &[EditorItemOutcome<'_>],
    occurrences: &[DeclarationOccurrence<'_>],
    raw_text: &str,
    cursor: Option<usize>,
) -> Option<DraftRewrite> {
    use super::transfer::{scan_transfer_line, TransferScan};
    let mut bare: Vec<(usize, usize, usize)> = Vec::new();
    for (item_index, item) in draft.items.iter().enumerate() {
        let Some(parent) = item.lines.first() else {
            continue;
        };
        // Any claimed scan owns the line: a complete transfer, a partial
        // right-hand side, or a teaching error (a trailing `@@` is
        // `transfer_extra_text`). Escaped and unclaimed lines keep the
        // ordinary declaration paths.
        if !matches!(
            scan_transfer_line(&parent.raw, item.source, item.lines.len() > 1),
            TransferScan::Valid { .. }
                | TransferScan::Incomplete { .. }
                | TransferScan::Invalid { .. }
        ) {
            continue;
        }
        for token in tokenize_with_spans(parent.raw.text) {
            if token.text == "@@" {
                bare.push((
                    item_index,
                    parent.raw.start + token.start,
                    parent.raw.start + token.end,
                ));
            }
        }
    }
    let (item_index, _, _) = match cursor {
        Some(position) => bare
            .iter()
            .find(|(_, start, end)| *start <= position && position <= *end)
            .copied()?,
        None => {
            let last = bare.last().copied()?;
            let last_occurrence = occurrences
                .iter()
                .filter(|occurrence| occurrence.token.text == "@@")
                .map(|occurrence| occurrence.token.start)
                .next_back();
            if last_occurrence.is_some_and(|start| start > last.1) {
                return None;
            }
            last
        }
    };
    let marker = item_outcomes[item_index]
        .item
        .spans
        .iter()
        .find(|span| span.kind == SpanKind::TransferDestination)
        .and_then(|span| raw_text.get(span.start..span.end))
        .or_else(|| {
            transfer_destination_token_text(
                &draft.items[item_index].lines.first()?.raw,
            )
        })
        .unwrap_or("the transfer destination");
    Some(unchanged_rewrite(
        raw_text,
        cursor,
        vec![format!(
            "@@ cannot take a capture transfer: leave {marker} on this item, or delete it"
        )],
    ))
}

pub(super) enum LocalMarkerAbsorbability {
    Absorbable(String),
    NonAbsorbable,
}

/// Only a plain task route can become a shared `@@` declaration. Markers that
/// select an existing task, section, Pomodoro, or project note stay local.
pub(super) fn classify_local_marker(
    marker: &LocalDestinationMarker,
) -> LocalMarkerAbsorbability {
    match marker.mode {
        EditorMode::Task if marker.block_id.is_none() => {
            LocalMarkerAbsorbability::Absorbable(
                marker.route.clone().unwrap_or_default(),
            )
        }
        EditorMode::Task
        | EditorMode::SubBullet
        | EditorMode::Bullet
        | EditorMode::PomodoroTask
        | EditorMode::PomodoroNote
        | EditorMode::ProjectNote
        | EditorMode::PomodoroProjectNote
        | EditorMode::PomodoroAdjust
        | EditorMode::PomodoroShift
        | EditorMode::PomodoroSplit
        | EditorMode::PomodoroMerge
        | EditorMode::PomodoroLink
        | EditorMode::PomodoroClose
        | EditorMode::PomodoroPast
        | EditorMode::PomodoroStart
        | EditorMode::TaskDependency
        | EditorMode::TaskComplete
        | EditorMode::TaskMove
        | EditorMode::TaskMerge
        | EditorMode::Ref
        | EditorMode::NotePull
        | EditorMode::TaskToggle => LocalMarkerAbsorbability::NonAbsorbable,
        EditorMode::Incomplete => {
            unreachable!("complete_local_destination_marker filters these out")
        }
    }
}

/// Rule A5: explain why `@@` cannot take this item's single local marker.
pub(super) fn non_absorbable_marker_notice(
    marker: &LocalDestinationMarker,
    item: &EditorItemParse,
) -> String {
    let route = marker.route.as_deref().unwrap_or("route");
    match marker.mode {
        EditorMode::SubBullet => format!(
            "@@ cannot take an existing task destination: leave {} on this item, or delete it and declare @@{route}",
            marker.text
        ),
        EditorMode::Bullet => format!(
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
        EditorMode::ProjectNote | EditorMode::PomodoroProjectNote => format!(
            "@@ cannot take a project note: leave {} on this item, or delete it",
            marker.text
        ),
        EditorMode::TaskToggle => format!(
            "@@ cannot take an existing task destination: leave {} on this item, or delete it and declare @@{route}",
            marker.text,
        ),
        EditorMode::PomodoroAdjust => format!(
            "@@ cannot take a Pomodoro adjustment: leave {} on this item, or delete it",
            marker.text
        ),
        EditorMode::PomodoroShift => format!(
            "@@ cannot take a Pomodoro shift: leave {} on this item, or delete it",
            marker.text
        ),
        EditorMode::PomodoroSplit => format!(
            "@@ cannot take a Pomodoro split: leave {} on this item, or delete it",
            marker.text
        ),
        EditorMode::PomodoroMerge => format!(
            "@@ cannot take a Pomodoro merge: leave {} on this item, or delete it",
            marker.text
        ),
        EditorMode::PomodoroLink => format!(
            "@@ cannot take a Pomodoro link: leave {} on this item, or delete it",
            marker.text
        ),
        EditorMode::PomodoroClose => format!(
            "@@ cannot take a Pomodoro close: leave {} on this item, or delete it",
            marker.text
        ),
        EditorMode::PomodoroStart => format!(
            "@@ cannot take a Pomodoro start: leave {} on this item, or delete it",
            marker.text
        ),
        EditorMode::PomodoroPast => format!(
            "@@ cannot take a past Pomodoro: leave {} on this item, or delete it",
            marker.text
        ),
        // A dependency-only marker names an existing task: absorbing it
        // into `@@` would change which task the prerequisites attach to.
        EditorMode::TaskDependency => format!(
            "@@ cannot take a task relationship owner ({}): leave {} on this item, or delete it",
            {
                let mut candidates: Vec<(usize, &str)> = Vec::new();
                candidates.extend(
                    item.dependencies
                        .first()
                        .map(|entry| (entry.start, "&")),
                );
                candidates.extend(
                    item.blocked_tasks
                        .first()
                        .map(|entry| (entry.start, "*")),
                );
                candidates.extend(
                    item.relations.first().map(|entry| (entry.start, "~")),
                );
                candidates.sort_by_key(|(start, _)| *start);
                candidates
                    .first()
                    .map(|(_, sigil)| *sigil)
                    .unwrap_or("&")
            },
            marker.text,
        ),
        // A completion token names an existing task: absorbing it into
        // `@@` would change which task gets completed.
        EditorMode::TaskComplete => format!(
            "@@ cannot take a task completion: leave {} on this item, or delete it",
            marker.text
        ),
        EditorMode::TaskMove | EditorMode::TaskMerge => format!(
            "@@ cannot take a capture transfer: leave {} on this item, or delete it",
            marker.text
        ),
        EditorMode::Ref => format!(
            "@@ cannot take a reference link: leave {} on this item, or delete it",
            marker.text
        ),
        EditorMode::NotePull => format!(
            "@@ cannot take a note pull: leave {} on this item, or delete it",
            marker.text
        ),
        EditorMode::Incomplete => {
            unreachable!("complete_local_destination_marker filters these out")
        }
    }
}

pub(super) fn absorb_local_marker_summary(
    marker_text: &str,
    payload: &str,
) -> String {
    format!("Moved {marker_text} into @@{payload}")
}

pub(super) fn absorb_declaration_summary(payload: &str) -> String {
    format!("Moved the @@{payload} declaration here")
}

/// Build every edit Rule A1's absorption needs: the bare `@@` becomes
/// `@@<payload>`, every other declaration token in the draft is deleted, and
/// -- for `AbsorbLocalMarker` only -- `extra_deletion` (the local marker's
/// own span, which is not itself a declaration token) is deleted too.
#[allow(clippy::too_many_arguments)]
pub(super) fn finish_absorption(
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

pub(super) fn apply_text_edits(raw_text: &str, edits: &[TextEdit]) -> String {
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
pub(super) fn mapped_cursor_after(
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
pub(super) struct DeletionLineContext<'a> {
    pub(super) physical: RawLine<'a>,
    pub(super) content_start: usize,
    pub(super) content_end: usize,
}

pub(super) fn deletion_line_context<'a>(
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
pub(super) fn deletion_edits_for_token(
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
pub(super) fn whole_line_deletion_span(
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
