//! Draft absorption and text edits.

use super::draft::*;
use super::editor_model::*;
use super::editor_parse::*;
use super::model::*;

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

pub(super) enum LocalMarkerAbsorbability {
    Absorbable(String),
    NonAbsorbable,
}

/// Classify Rule A1's ordered payload source 1: `mode`/`block_id`/`section`
/// close over the eight local destination marker forms the Vocabulary section
/// defines, so this match is exhaustive over real (non-incomplete) markers.
/// A project-note marker is never absorbable: a `@@` declaration that
/// created a note would try to create the same note once per item.
pub(super) fn classify_local_marker(
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
        | EditorMode::PomodoroNote
        | EditorMode::ProjectNote
        | EditorMode::PomodoroProjectNote
        | EditorMode::PomodoroAdjust
        | EditorMode::PomodoroShift
        | EditorMode::PomodoroLink
        | EditorMode::PomodoroClose
        | EditorMode::PomodoroStart
        | EditorMode::TaskDependency
        | EditorMode::TaskComplete
        | EditorMode::Ref
        | EditorMode::TaskToggle => LocalMarkerAbsorbability::NonAbsorbable,
        EditorMode::Incomplete => {
            unreachable!("complete_local_destination_marker filters these out")
        }
    }
}

/// Rule A5: explain why `@@` cannot take this item's single local marker.
pub(super) fn non_absorbable_marker_notice(
    marker: &LocalDestinationMarker,
) -> String {
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
        EditorMode::ProjectNote | EditorMode::PomodoroProjectNote => format!(
            "@@ cannot take a project note: leave {} on this item, or delete it",
            marker.text
        ),
        EditorMode::TaskToggle => format!(
            "@@ cannot take a task toggle: leave {} on this item, or delete it",
            marker.text
        ),
        EditorMode::PomodoroAdjust => format!(
            "@@ cannot take a Pomodoro adjustment: leave {} on this item, or delete it",
            marker.text
        ),
        EditorMode::PomodoroShift => format!(
            "@@ cannot take a Pomodoro shift: leave {} on this item, or delete it",
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
        // A dependency-only marker names an existing task: absorbing it
        // into `@@` would change which task the prerequisites attach to.
        EditorMode::TaskDependency => format!(
            "@@ cannot take a task dependency owner: leave {} on this item, or delete it",
            marker.text
        ),
        // A completion token names an existing task: absorbing it into
        // `@@` would change which task gets completed.
        EditorMode::TaskComplete => format!(
            "@@ cannot take a task completion: leave {} on this item, or delete it",
            marker.text
        ),
        EditorMode::Ref => format!(
            "@@ cannot take a reference link: leave {} on this item, or delete it",
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
