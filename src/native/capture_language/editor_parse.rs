//! Core editor line, global, and item parsing.

use super::dependencies::*;
use super::draft::*;
use super::editor_classify::*;
use super::editor_model::*;
use super::editor_pomodoro::*;
use super::line::*;
use super::markers::*;
use super::model::*;
use super::project_tasks::*;
use super::tokens::*;

/// Remap one physical line's own tokenizer output into the original
/// multi-line text's byte offsets, so every span an editor receives always
/// indexes the raw text the user is looking at.
pub(crate) fn tokenize_line_with_spans<'a>(
    line: &RawLine<'a>,
) -> Vec<Token<'a>> {
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
pub(super) struct LineEditorParse<'a> {
    pub(super) body: String,
    /// The remaining body tokens with original-draft byte offsets, in
    /// source order. The task-ID post-pass reads the last entry to span
    /// the ` :id` / ` ^id` token exactly.
    pub(super) body_tokens: Vec<Token<'a>>,
    pub(super) marker: Option<MarkerParse>,
    pub(super) marker_text: Option<String>,
    pub(super) declarations: Vec<Token<'a>>,
    pub(super) terminal_spans: Vec<Span>,
    pub(super) diagnostics: Vec<Diagnostic>,
    pub(super) has_destination_marker: bool,
    /// Dependency modifiers stripped from this line's marker runs.
    pub(super) dependencies: LineDependencies,
}

/// Resolve one line's already offset-tagged tokens exactly like
/// `parse_for_editor` resolved its single line before this module became
/// line-aware. `leading` allows a first-token route to win and must only be
/// set for the parent line.
pub(super) fn parse_editor_line<'a>(
    mut tokens: Vec<Token<'a>>,
    line_text: &str,
    line_base: usize,
    leading: bool,
) -> LineEditorParse<'a> {
    let declarations = take_global_declarations(&mut tokens);
    // Dependency modifiers interleave with destination and
    // schedule/priority/clipboard markers in either order, so they leave
    // the token stream before terminal-marker extraction and marker
    // selection see it. The remaining tokens resolve exactly like a draft
    // without them.
    let (stripped, dependencies) =
        extract_line_dependencies(tokens, line_text, line_base, leading, true);
    tokens = stripped;
    let (_, marker_spans) = extract_terminal_markers(&mut tokens, true);
    let mut terminal_spans: Vec<Span> = marker_spans
        .into_iter()
        .map(|(kind, start, end)| Span { start, end, kind })
        .collect();
    terminal_spans.extend(dependencies.spans.iter().copied());

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
    let body_tokens: Vec<Token<'_>> = tokens
        .iter()
        .enumerate()
        .filter(|(index, _)| Some(*index) != marker_index)
        .map(|(_, token)| *token)
        .collect();
    // A consumed `\&` escape leaves its visible `&...` in task text.
    let body = body_tokens
        .iter()
        .map(|token| {
            unescape_dependency_text(
                token.text,
                token.start,
                &dependencies.escapes,
            )
        })
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
        body_tokens,
        marker,
        marker_text,
        declarations,
        terminal_spans,
        diagnostics,
        has_destination_marker,
        dependencies,
    }
}

/// Track which item-wide marker slots earlier lines already resolved, so
/// a later line that resolves the same slot becomes a diagnostic instead of
/// silently overriding or being silently dropped.
#[derive(Default)]
pub(super) struct SeenMarkers {
    pub(super) schedule: bool,
    pub(super) priority: bool,
    pub(super) clip: bool,
    pub(super) route: bool,
}

impl SeenMarkers {
    pub(super) fn absorb_terminal_spans(
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

    pub(super) fn absorb_route(
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

pub(super) fn terminal_marker_label(kind: SpanKind) -> &'static str {
    match kind {
        SpanKind::Schedule => "schedule marker (s:<N>)",
        SpanKind::Priority => "priority marker (p:<N>)",
        SpanKind::Clipboard => "clipboard marker (%)",
        _ => "capture marker",
    }
}

pub(super) fn duplicate_capture_marker_diagnostic(
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
/// `@route:id#`, `@route:#name`, `@^`, `@route^`, `@+`, `@route+`, `@@`,
/// `@@route+`, and their legacy `@!` aliases) is a valid editing state,
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
            // Whole-item session operators, `=x[<N>][!<M>]` closes
            // (including dangling-separator editing states), `=`/`=<X>`
            // starts, `=<X>#` incomplete named starts (which carry
            // the partial `pomodoro_start` spec), and `:` task-link
            // queries are their own mode: a
            // `@@` declaration routes ordinary items in the same draft
            // but never turns an operator, close, start, or query into a task
            // or changes its destination.
            if item.mode == EditorMode::PomodoroAdjust
                || item.mode == EditorMode::PomodoroShift
                || item.mode == EditorMode::PomodoroClose
                || item.mode == EditorMode::PomodoroStart
                || item.needs == [Need::TaskLink]
                || item.pomodoro_close.is_some()
                || item.pomodoro_start.is_some()
            {
                continue;
            }
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
            pomodoro_start: None,
            pomodoro_adjust: None,
            pomodoro_shift: None,
            pomodoro_close: None,
            dependencies: Vec::new(),
            dependency_target: None,
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
    let pomodoro_start = first.pomodoro_start.clone();
    let pomodoro_adjust = first.pomodoro_adjust.clone();
    let pomodoro_shift = first.pomodoro_shift.clone();
    let pomodoro_close = first.pomodoro_close.clone();
    let dependencies = first.dependencies.clone();
    let dependency_target = first.dependency_target.clone();
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
        pomodoro_start,
        pomodoro_adjust,
        pomodoro_shift,
        pomodoro_close,
        dependencies,
        dependency_target,
        spans,
        diagnostics,
        sub_bullets,
        items,
        global_destination,
    }
}

pub(super) fn parse_editor_global_declarations(
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

pub(super) fn inherit_editor_global_destination(
    item: &mut EditorItemParse,
    global: &EditorGlobalDestination,
) {
    if item.has_local_destination {
        return;
    }
    // Dependency needs survive inheritance: the modifiers are item-local
    // even when their dependent comes from the declaration.
    let had_task_dependency_need = item.needs.contains(&Need::TaskDependency);
    let had_target_need = item.needs.contains(&Need::DependencyTarget);
    item.mode = global.mode;
    item.route = global.route.clone();
    item.section = None;
    item.block_id = global.block_id.clone();
    item.needs = global.needs.clone();
    if item.dependencies.is_empty()
        && !had_task_dependency_need
        && !had_target_need
    {
        return;
    }
    if had_task_dependency_need && !item.needs.contains(&Need::TaskDependency) {
        item.needs.push(Need::TaskDependency);
    }
    if let Some(block_id) = global.block_id.clone() {
        // `@@route+id` selects the inherited existing parent: the item's
        // modifiers apply to it.
        item.dependency_target = Some(DependencyTarget {
            kind: DependencyTargetKind::ExistingTask,
            route: global.route.clone(),
            block_id: Some(block_id),
            inherited: true,
        });
        item.needs.retain(|need| *need != Need::DependencyTarget);
        if item.body.is_empty() {
            item.mode = EditorMode::TaskDependency;
        }
    } else if let Some(target) = item.dependency_target.as_mut() {
        // A bare `@@route` routes a new dependent without naming a parent.
        if target.kind == DependencyTargetKind::NewTask
            && target.route.is_none()
        {
            target.route = global.route.clone();
        }
    } else if had_target_need {
        // Still ownerless: a bare route never names the dependent.
        if item.body.is_empty() {
            item.mode = EditorMode::Incomplete;
        }
        if !item.needs.contains(&Need::DependencyTarget) {
            item.needs.push(Need::DependencyTarget);
        }
    }
}

pub(super) struct EditorItemOutcome<'a> {
    pub(super) item: EditorItemParse,
    pub(super) declarations: Vec<GlobalDeclarationToken<'a>>,
}

/// Re-kind a resolved sub-bullet marker's spans to the task-toggle span
/// kinds, so the editor can color the marker distinctly the instant an item
/// becomes (or is one keystroke from becoming) a toggle.
pub(super) fn rekind_sub_bullet_spans(spans: &mut [Span]) {
    for span in spans {
        span.kind = match span.kind {
            SpanKind::SubBulletRoute => SpanKind::TaskToggleRoute,
            SpanKind::SubBulletBlockId => SpanKind::TaskToggleBlockId,
            SpanKind::SubBulletSection => SpanKind::TaskTogglePomodoroName,
            other => other,
        };
    }
}

/// Build an `existing_task` dependency target when the item resolved an
/// explicit existing-task owner. `None` (no target) when the route or
/// block ID is still unknown.
fn existing_dependency_target(
    route: &Option<String>,
    block_id: &Option<String>,
) -> Option<DependencyTarget> {
    match (route, block_id) {
        (Some(route), Some(block_id)) => Some(DependencyTarget {
            kind: DependencyTargetKind::ExistingTask,
            route: Some(route.clone()),
            block_id: Some(block_id.clone()),
            inherited: false,
        }),
        _ => None,
    }
}

/// Anticipate the narrow colon alias while its `@route:` owner is still
/// being typed: a known route with no block ID yet, on a bare colon-link
/// marker, will resolve to an existing task once the ID lands.
fn pending_colon_dependency_target(
    route: &Option<String>,
    marker_text: Option<&str>,
) -> Option<DependencyTarget> {
    let marker = marker_text?;
    if !marker.starts_with('@')
        || !marker.contains(':')
        || marker.contains('+')
        || marker.contains('#')
        || marker.contains('=')
        || marker.contains('!')
    {
        return None;
    }
    Some(DependencyTarget {
        kind: DependencyTargetKind::ExistingTask,
        route: route.clone(),
        block_id: None,
        inherited: false,
    })
}

fn dependency_owner_mode_word(mode: EditorMode) -> &'static str {
    match mode {
        EditorMode::Bullet => "section-bullet",
        EditorMode::ProjectNote | EditorMode::PomodoroProjectNote => {
            "project-note"
        }
        EditorMode::PomodoroNote => "Pomodoro-note",
        EditorMode::PomodoroAdjust | EditorMode::PomodoroShift => {
            "Pomodoro session-operator"
        }
        EditorMode::PomodoroClose => "Pomodoro-close",
        EditorMode::PomodoroStart => "Pomodoro-start",
        _ => "Pomodoro-ledger-link",
    }
}

fn dependency_unsupported_target_diagnostic(
    mode: EditorMode,
    range: Option<(usize, usize)>,
) -> Diagnostic {
    Diagnostic {
        severity: Severity::Error,
        code: "dependency_unsupported_target",
        message: format!(
            "task dependencies need a task owner: '&note:block-id' cannot \
             attach to a {} capture (add task text or '@note+task-id' for \
             the dependent)",
            dependency_owner_mode_word(mode)
        ),
        range,
    }
}

fn invalid_dependency_target_diagnostic(
    marker_text: &str,
    range: Option<(usize, usize)>,
) -> Diagnostic {
    Diagnostic {
        severity: Severity::Error,
        code: "invalid_dependency_target",
        message: format!(
            "dependency-only '{marker_text}' must be bare: it mixes a \
             Pomodoro action with '&note:block-id' (capture them as \
             separate blank-line items)"
        ),
        range,
    }
}

/// Attach a dependency rejection to an operator item (Pomodoro close,
/// start, adjustment, or shift) that claimed the draft before the generic
/// marker pass could: the modifiers are listed, malformed ones diagnosed,
/// and the combination refused with a targeted diagnostic.
pub(super) fn attach_operator_dependency_rejection<'a>(
    mut outcome: EditorItemOutcome<'a>,
    item: &CaptureItem<'a>,
) -> EditorItemOutcome<'a> {
    let found = scan_item_dependencies(item, true);
    if found.is_empty() {
        return outcome;
    }
    outcome.item.dependencies = found.dependency_entries();
    for invalid in &found.invalid {
        outcome.item.diagnostics.push(Diagnostic {
            severity: Severity::Error,
            code: "invalid_dependency",
            message: invalid.message.clone(),
            range: Some((invalid.start, invalid.end)),
        });
    }
    outcome
        .item
        .diagnostics
        .push(dependency_unsupported_target_diagnostic(
            outcome.item.mode,
            None,
        ));
    if found.has_partial()
        && !outcome.item.needs.contains(&Need::TaskDependency)
    {
        outcome.item.needs.push(Need::TaskDependency);
    }
    outcome
}

/// A `:` task-link picker query: a single-line, single-token item starting
/// with `:`. It is never executable, so the editor reports `incomplete`
/// needing `task_link` with one `interactive_placeholder` span over the
/// whole token (sigil included) and no diagnostics. A `@@` declaration
/// never applies to it, exactly like the `^` picker family.
pub(super) fn parse_editor_task_link_item<'a>(
    item: &CaptureItem<'a>,
) -> Option<EditorItemOutcome<'a>> {
    let token = task_link_query_token(item)?;
    Some(EditorItemOutcome {
        item: EditorItemParse {
            index: item.index,
            start: item.start,
            end: item.end,
            line_start: item.line_start,
            line_end: item.line_end,
            body: String::new(),
            mode: EditorMode::Incomplete,
            route: None,
            section: None,
            block_id: None,
            needs: vec![Need::TaskLink],
            pomodoro_start: None,
            pomodoro_adjust: None,
            pomodoro_shift: None,
            pomodoro_close: None,
            dependencies: Vec::new(),
            dependency_target: None,
            spans: vec![Span {
                start: token.start,
                end: token.end,
                kind: SpanKind::InteractivePlaceholder,
            }],
            diagnostics: Vec::new(),
            sub_bullets: Vec::new(),
            has_local_destination: true,
            local_destination_markers: Vec::new(),
        },
        declarations: Vec::new(),
    })
}

/// A terminal-plus task selector is unresolved editor input. Like the
/// leading-colon picker, it is never previewed or submitted as an ordinary
/// task; the explicit selection writes a scoped `@route+block-id` marker.
/// The exact lone parent-line `+` remains the existing Pomodoro adjustment.
pub(super) fn parse_editor_parent_task_item<'a>(
    item: &CaptureItem<'a>,
) -> Option<EditorItemOutcome<'a>> {
    let selector = parent_task_selector_tokens(item)
        .into_iter()
        .find(|selector| !selector.dual_use_adjustment)?;
    let token = selector.token;
    Some(EditorItemOutcome {
        item: EditorItemParse {
            index: item.index,
            start: item.start,
            end: item.end,
            line_start: item.line_start,
            line_end: item.line_end,
            body: String::new(),
            mode: EditorMode::Incomplete,
            route: None,
            section: None,
            block_id: None,
            needs: vec![Need::TaskParent],
            pomodoro_start: None,
            pomodoro_adjust: None,
            pomodoro_shift: None,
            pomodoro_close: None,
            dependencies: Vec::new(),
            dependency_target: None,
            spans: vec![Span {
                start: token.start,
                end: token.end,
                kind: SpanKind::InteractivePlaceholder,
            }],
            diagnostics: Vec::new(),
            sub_bullets: Vec::new(),
            has_local_destination: true,
            local_destination_markers: Vec::new(),
        },
        declarations: Vec::new(),
    })
}

pub(super) fn parse_editor_item<'a>(
    item: &CaptureItem<'a>,
) -> EditorItemOutcome<'a> {
    if let Some(task_link) = parse_editor_task_link_item(item) {
        return task_link;
    }
    if let Some(close) = parse_editor_close_item(item) {
        return attach_operator_dependency_rejection(close, item);
    }
    if let Some(parent_task) = parse_editor_parent_task_item(item) {
        return attach_operator_dependency_rejection(parent_task, item);
    }
    if let Some(adjustment) = parse_editor_adjust_item(item) {
        return attach_operator_dependency_rejection(adjustment, item);
    }
    let mut spans: Vec<Span> = Vec::new();
    let mut diagnostics: Vec<Diagnostic> = Vec::new();
    let mut seen = SeenMarkers::default();
    let mut declarations = Vec::new();
    let mut local_destination_marker = None;
    let mut local_destination_markers = Vec::new();

    let parent_item_line = item.lines.first().expect("nonempty item");
    let parent_line = parent_item_line.raw;
    let parent_tokens = tokenize_line_with_spans(&parent_line);
    let parent_parse = parse_editor_line(
        parent_tokens,
        parent_line.text,
        parent_line.start,
        true,
    );
    declarations.extend(global_declarations_from_tokens(
        parent_parse.declarations,
        parent_item_line.line_number,
    ));
    seen.absorb_terminal_spans(&parent_parse.terminal_spans, &mut diagnostics);
    spans.extend(parent_parse.terminal_spans);
    diagnostics.extend(parent_parse.diagnostics);

    // Every `&note:block-id` modifier on this item, across its parent and
    // authored-child lines. Ownership (`dependency_target`), incomplete
    // needs, and mode rejections resolve once the item's marker is known.
    let mut item_dependencies = ItemDependencySet::default();
    item_dependencies.absorb(&parent_parse.dependencies);

    let caret = classify_caret_item(item, parent_line);

    let mut body = parent_parse.body;
    let mut has_local_destination = parent_parse.has_destination_marker;
    if local_destination_marker.is_none() {
        local_destination_marker = parent_parse.marker_text.clone();
    }
    let mut own_local_destination_marker_index = None;
    if let Some(marker) = complete_local_destination_marker(
        parent_parse.marker_text.as_deref(),
        parent_parse.marker.as_ref(),
    ) {
        local_destination_markers.push(marker);
        own_local_destination_marker_index =
            Some(local_destination_markers.len() - 1);
    }
    // Set only when the parent's marker is a complete sub-bullet marker whose
    // trailing name only passed the wider Pomodoro charset -- i.e. it needs a
    // strict re-check once we know whether this item is a task toggle.
    let mut sub_bullet_relaxed_section_range = None;
    let (
        mut mode,
        mut route,
        mut section,
        mut block_id,
        mut needs,
        mut pomodoro_start,
        mut pomodoro_close,
    ) = match &parent_parse.marker {
        Some(marker) => {
            spans.extend(marker.spans.clone());
            seen.absorb_route(None, &mut diagnostics);
            if marker.mode == EditorMode::SubBullet
                && marker
                    .section
                    .as_deref()
                    .is_some_and(|section| !is_selector_component(section))
            {
                let start = marker.spans.first().map(|span| span.start);
                let end = marker.spans.last().map(|span| span.end);
                if let (Some(start), Some(end)) = (start, end) {
                    sub_bullet_relaxed_section_range = Some((start, end));
                }
            }
            (
                marker.mode,
                marker.route.clone(),
                marker.section.clone(),
                marker.block_id.clone(),
                marker.needs.clone(),
                marker.pomodoro_start.clone(),
                marker.pomodoro_close.clone(),
            )
        }
        None => (EditorMode::Task, None, None, None, Vec::new(), None, None),
    };

    let mut sub_bullets = Vec::new();
    let mut child_task_tokens: Vec<Option<Token<'_>>> = Vec::new();
    let mut child_task_lines: Vec<usize> = Vec::new();
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
        let child_parse = parse_editor_line(
            child_tokens,
            child_line.text,
            child_line.start,
            false,
        );
        item_dependencies.absorb(&child_parse.dependencies);
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
                pomodoro_start = marker.pomodoro_start.clone();
                pomodoro_close = marker.pomodoro_close.clone();
            }
        }

        if child_parse.body.is_empty() {
            // A child holding only dependency modifiers contributes no
            // empty bullet: its prerequisites belong to the item's
            // dependent. Any other emptied child stays an error.
            if child_parse.dependencies.stripped_tokens == 0 {
                diagnostics.push(Diagnostic {
                    severity: Severity::Error,
                    code: "empty_child_after_markers",
                    message: empty_child_after_markers_error(line_number),
                    range: Some((raw.start, raw.end)),
                });
            }
        } else {
            child_task_tokens.push(child_parse.body_tokens.last().copied());
            child_task_lines.push(line_number);
            let child_body = child_parse.body;
            sub_bullets.push(AuthoredSubBullet {
                body: child_body,
                depth: authored.depth,
                task_id: None,
            });
            if authored.depth == AuthoredDepth::First {
                has_first_level_owner = true;
            }
        }
    }

    // A bare `@route+block-id[#name]` marker with an empty body, no authored
    // children, and no other item-wide marker is a task toggle instead of a
    // sub-bullet capture; a trailing bare `#` on the same shape is one
    // keystroke away from one. Neither can arise from a child line's marker
    // (a child line always has authored body text), so only the parent's
    // marker is ever relevant here. See `resolve_sub_bullet_kind` for the
    // mirrored decision in the execution grammar.
    let toggle_eligible = body.is_empty()
        && sub_bullets.is_empty()
        && !seen.schedule
        && !seen.priority
        && !seen.clip;
    let has_explicit_toggle = spans
        .iter()
        .any(|span| span.kind == SpanKind::TaskToggleExplicitToggle);
    if has_explicit_toggle && !toggle_eligible {
        let range = spans.iter().find_map(|span| {
            (span.kind == SpanKind::TaskToggleExplicitToggle)
                .then_some((span.start, span.end))
        });
        diagnostics.push(Diagnostic {
            severity: Severity::Error,
            code: "unsupported_explicit_toggle",
            message: EXPLICIT_TOGGLE_ONLY_MARKER_ERROR.to_string(),
            range,
        });
    }
    if toggle_eligible && mode == EditorMode::SubBullet {
        mode = EditorMode::TaskToggle;
        rekind_sub_bullet_spans(&mut spans);
        if let Some(index) = own_local_destination_marker_index {
            local_destination_markers[index].mode = EditorMode::TaskToggle;
        }
    } else if toggle_eligible
        && mode == EditorMode::Incomplete
        && needs == [Need::TaskSection]
    {
        needs = vec![Need::PomodoroName];
        rekind_sub_bullet_spans(&mut spans);
    } else if let Some(range) = sub_bullet_relaxed_section_range {
        mode = EditorMode::Task;
        route = None;
        section = None;
        block_id = None;
        needs = Vec::new();
        spans.retain(|span| {
            !matches!(
                span.kind,
                SpanKind::SubBulletRoute
                    | SpanKind::SubBulletBlockId
                    | SpanKind::SubBulletSection
            )
        });
        diagnostics.push(Diagnostic {
            severity: Severity::Error,
            code: "invalid_sub_bullet_section",
            message: SUB_BULLET_SECTION_ERROR.to_string(),
            range: Some(range),
        });
        local_destination_marker = None;
        if let Some(index) = own_local_destination_marker_index {
            local_destination_markers.remove(index);
        }
    }

    // A solo `@route:block-id…` item links an existing task instead of
    // creating one, so `capture-parse` reports `pomodoro_link` exactly like
    // `bob capture` executes it. Anything else on the item (authored
    // children, clipboard, schedule, or priority markers) is an
    // `invalid_pomodoro_link` conflict instead of a new task. A close
    // suffix's schedule/priority conflicts report `invalid_pomodoro_close`
    // below with the conflicting marker's range instead.
    if mode == EditorMode::PomodoroTask
        && body.is_empty()
        && parent_parse
            .marker
            .as_ref()
            .is_some_and(|marker| marker.mode == EditorMode::PomodoroTask)
    {
        mode = EditorMode::PomodoroLink;
        if let Some(index) = own_local_destination_marker_index {
            local_destination_markers[index].mode = EditorMode::PomodoroLink;
        }
        let range = parent_parse.marker.as_ref().and_then(|marker| {
            let start = marker.spans.first().map(|span| span.start)?;
            let end = marker.spans.last().map(|span| span.end)?;
            Some((start, end))
        });
        let close_defers_schedule =
            pomodoro_close.is_some() && (seen.schedule || seen.priority);
        let conflict = if !sub_bullets.is_empty() {
            Some(POMODORO_LINK_CHILD_CONFLICT_ERROR)
        } else if seen.schedule && !close_defers_schedule {
            Some(POMODORO_LINK_SCHEDULE_CONFLICT_ERROR)
        } else if seen.priority && !close_defers_schedule {
            Some(POMODORO_LINK_PRIORITY_CONFLICT_ERROR)
        } else if seen.clip {
            Some(POMODORO_LINK_CLIP_CONFLICT_ERROR)
        } else {
            None
        };
        if let Some(message) = conflict {
            diagnostics.push(Diagnostic {
                severity: Severity::Error,
                code: "invalid_pomodoro_link",
                message: message.to_string(),
                range,
            });
        }
    }

    // A `=x` close suffix cannot combine with `s:<N>`, `p:<N>`, or `%`:
    // report `invalid_pomodoro_close` on the conflicting marker itself,
    // mirroring `bob capture`'s strict execution errors. Applies to both
    // solo links and body-bearing task captures.
    if pomodoro_close.is_some()
        && (mode == EditorMode::PomodoroTask
            || mode == EditorMode::PomodoroLink)
    {
        if seen.schedule
            && let Some(span) =
                spans.iter().find(|span| span.kind == SpanKind::Schedule)
        {
            diagnostics.push(Diagnostic {
                severity: Severity::Error,
                code: "invalid_pomodoro_close",
                message: POMODORO_CLOSE_SCHEDULE_CONFLICT_ERROR.to_string(),
                range: Some((span.start, span.end)),
            });
        }
        if seen.priority
            && let Some(span) =
                spans.iter().find(|span| span.kind == SpanKind::Priority)
        {
            diagnostics.push(Diagnostic {
                severity: Severity::Error,
                code: "invalid_pomodoro_close",
                message: POMODORO_CLOSE_PRIORITY_CONFLICT_ERROR.to_string(),
                range: Some((span.start, span.end)),
            });
        }
        if seen.clip
            && let Some(span) =
                spans.iter().find(|span| span.kind == SpanKind::Clipboard)
        {
            diagnostics.push(Diagnostic {
                severity: Severity::Error,
                code: "invalid_pomodoro_close",
                message: POMODORO_LINK_CLIP_CONFLICT_ERROR.to_string(),
                range: Some((span.start, span.end)),
            });
        }
    }

    // Project task IDs (` :id` / ` ^id`) are a post-pass over the parsed
    // lines, mirroring execution's evaluation order through the shared
    // `ProjectTaskPass`: the parent line, then each child in source order,
    // then the unused-`#pomodoro` rule. Accepted IDs are stripped from the
    // sub-bullet bodies and recorded on them; violations become
    // diagnostics. A lone sigil ending a first-level bullet is an
    // unfinished ID: `incomplete` needing `block_id` with a placeholder
    // span over the sigil and no diagnostic. A project note with at least
    // one accepted ` :` task upgrades to `pomodoro_project_note`.
    if mode == EditorMode::ProjectNote {
        let mut pass = ProjectTaskPass::new();
        let mut has_pending_colon_link = false;
        if let Some(message) = pass.check_parent(&body) {
            diagnostics.push(Diagnostic {
                severity: Severity::Error,
                code: "misplaced_project_task_id",
                message,
                range: parent_parse
                    .body_tokens
                    .last()
                    .map(|token| (token.start, token.end)),
            });
        }
        for (index, sub_bullet) in sub_bullets.iter_mut().enumerate() {
            let token = child_task_tokens[index];
            let line_number = child_task_lines[index];
            match pass.check_child(
                &sub_bullet.body,
                sub_bullet.depth,
                line_number,
            ) {
                ChildTaskOutcome::Ignore => {}
                ChildTaskOutcome::Unfinished { sigil } => {
                    mode = EditorMode::Incomplete;
                    if sigil == ':' {
                        has_pending_colon_link = true;
                    }
                    if !needs.contains(&Need::BlockId) {
                        needs.push(Need::BlockId);
                    }
                    if let Some(token) = token {
                        spans.push(Span {
                            start: token.start,
                            end: token.end,
                            kind: SpanKind::InteractivePlaceholder,
                        });
                    }
                }
                ChildTaskOutcome::Accept {
                    sigil,
                    id,
                    stripped,
                } => {
                    let link = sigil == ':';
                    sub_bullet.body = stripped;
                    sub_bullet.task_id =
                        Some(ProjectTaskId { block_id: id, link });
                    if let Some(token) = token {
                        if link {
                            spans.push(Span {
                                start: token.start,
                                end: token.start + 1,
                                kind: SpanKind::ProjectTaskLinkMarker,
                            });
                        }
                        spans.push(Span {
                            start: token.start + 1,
                            end: token.end,
                            kind: SpanKind::ProjectTaskBlockId,
                        });
                    }
                }
                ChildTaskOutcome::Error { code, message } => {
                    diagnostics.push(Diagnostic {
                        severity: Severity::Error,
                        code,
                        message,
                        range: token.map(|token| (token.start, token.end)),
                    });
                }
            }
        }
        // A `#pomodoro` name picks the Pomodoro that ` :<id>` Task Links
        // go under. The diagnostic covers the `#name` component. An
        // unfinished ` :` ID counts as a pending link (a lone `^` does
        // not, per Rule 7).
        if section.is_some() && !pass.has_link && !has_pending_colon_link {
            let range = spans
                .iter()
                .find(|span| span.kind == SpanKind::PomodoroName)
                .map(|span| (span.start.saturating_sub(1), span.end));
            diagnostics.push(Diagnostic {
                severity: Severity::Error,
                code: "unused_project_note_pomodoro",
                message: unused_project_note_pomodoro_error(
                    section.as_deref().unwrap_or_default(),
                ),
                range,
            });
        }
        if pass.has_link && mode == EditorMode::ProjectNote {
            mode = EditorMode::PomodoroProjectNote;
        }
    }

    // A leading `^` token claims its item (see `classify_caret_item`):
    // complete shapes report `pomodoro_link` with the `active_task_*`
    // spans, partial shapes report `incomplete`, and near misses report
    // `pomodoro_link` with an `invalid_pomodoro_link` diagnostic. The `^`
    // token replaces any ordinary marker the generic pass resolved, and a
    // `@@` declaration never applies to the item.
    let has_caret_item = caret.is_some();
    if let Some(caret) = caret {
        local_destination_markers.clear();
        local_destination_marker = None;
        has_local_destination = true;
        let token_start = caret.token.start;
        let token_end = caret.token.end;
        match caret.kind {
            CaretItemKind::Complete {
                route: caret_route,
                block_id: caret_block,
                pomodoro_name: caret_name,
                start: caret_start,
                close: caret_close,
                link_end,
                name_range,
                start_offset,
                close_in_progress,
                close_park,
                close_complete,
                close_drop,
                conflict,
            } => {
                mode = EditorMode::PomodoroLink;
                body = if caret.solo_parent {
                    String::new()
                } else {
                    body
                };
                let route_end = token_start + 1 + caret_route.len();
                route = Some(caret_route);
                section = caret_name;
                block_id = Some(caret_block);
                needs = Vec::new();
                pomodoro_start = caret_start;
                pomodoro_close = caret_close;
                spans.push(Span {
                    start: token_start,
                    end: route_end,
                    kind: SpanKind::ActiveTaskRoute,
                });
                spans.push(Span {
                    start: route_end + 1,
                    end: token_start + link_end,
                    kind: SpanKind::ActiveTaskBlockId,
                });
                if let Some((name_start, name_end)) = name_range
                    && name_end > name_start
                {
                    spans.push(Span {
                        start: token_start + name_start,
                        end: token_start + name_end,
                        kind: SpanKind::PomodoroName,
                    });
                }
                if let Some(offset) = start_offset {
                    if pomodoro_close.is_some() {
                        spans.push(Span {
                            start: token_start + offset,
                            end: token_start + offset + 2,
                            kind: SpanKind::PomodoroClose,
                        });
                        if let Some((start, end)) = close_in_progress {
                            spans.push(Span {
                                start: token_start + start,
                                end: token_start + end,
                                kind: SpanKind::PomodoroCloseInProgress,
                            });
                        }
                        if let Some((start, end)) = close_park {
                            spans.push(Span {
                                start: token_start + start,
                                end: token_start + end,
                                kind: SpanKind::PomodoroClosePark,
                            });
                        }
                        if let Some((start, end)) = close_complete {
                            spans.push(Span {
                                start: token_start + start,
                                end: token_start + end,
                                kind: SpanKind::PomodoroCloseComplete,
                            });
                        }
                        if let Some((start, end)) = close_drop {
                            spans.push(Span {
                                start: token_start + start,
                                end: token_start + end,
                                kind: SpanKind::PomodoroCloseDrop,
                            });
                        }
                    } else {
                        spans.push(Span {
                            start: token_start + offset,
                            end: token_end,
                            kind: SpanKind::PomodoroStart,
                        });
                    }
                }
                local_destination_markers.push(LocalDestinationMarker {
                    start: token_start,
                    end: token_end,
                    text: caret.token.text.to_string(),
                    mode: EditorMode::PomodoroLink,
                    route: route.clone(),
                    block_id: block_id.clone(),
                    section: section.clone(),
                });
                local_destination_marker = local_destination_markers
                    .last()
                    .map(|marker| marker.text.clone());
                if let Some(message) = conflict {
                    diagnostics.push(Diagnostic {
                        severity: Severity::Error,
                        code: "invalid_pomodoro_link",
                        message,
                        range: Some((token_start, token_end)),
                    });
                }
            }
            CaretItemKind::CloseIncomplete {
                route: caret_route,
                block_id: caret_block,
                close: caret_close,
                link_end,
                suffix_offset,
                close_in_progress,
                close_park,
                close_complete,
                close_drop,
                separator_range,
                conflict,
            } => {
                // A dangling separator is an editing state: mode
                // `incomplete` needing `pomodoro_close_task`, the partial
                // spec, the spans typed so far, and one
                // `interactive_placeholder` span over the separator. An item
                // conflict (extra text, child lines) wins instead and
                // reports like a complete link conflict.
                mode = EditorMode::PomodoroLink;
                body = if caret.solo_parent {
                    String::new()
                } else {
                    body
                };
                let route_end = token_start + 1 + caret_route.len();
                route = Some(caret_route);
                section = None;
                block_id = Some(caret_block);
                pomodoro_start = None;
                pomodoro_close = Some(caret_close);
                spans.push(Span {
                    start: token_start,
                    end: route_end,
                    kind: SpanKind::ActiveTaskRoute,
                });
                spans.push(Span {
                    start: route_end + 1,
                    end: token_start + link_end,
                    kind: SpanKind::ActiveTaskBlockId,
                });
                spans.push(Span {
                    start: token_start + suffix_offset,
                    end: token_start + suffix_offset + 2,
                    kind: SpanKind::PomodoroClose,
                });
                if let Some((start, end)) = close_in_progress {
                    spans.push(Span {
                        start: token_start + start,
                        end: token_start + end,
                        kind: SpanKind::PomodoroCloseInProgress,
                    });
                }
                if let Some((start, end)) = close_park {
                    spans.push(Span {
                        start: token_start + start,
                        end: token_start + end,
                        kind: SpanKind::PomodoroClosePark,
                    });
                }
                if let Some((start, end)) = close_complete {
                    spans.push(Span {
                        start: token_start + start,
                        end: token_start + end,
                        kind: SpanKind::PomodoroCloseComplete,
                    });
                }
                if let Some((start, end)) = close_drop {
                    spans.push(Span {
                        start: token_start + start,
                        end: token_start + end,
                        kind: SpanKind::PomodoroCloseDrop,
                    });
                }
                spans.push(Span {
                    start: token_start + separator_range.0,
                    end: token_start + separator_range.1,
                    kind: SpanKind::InteractivePlaceholder,
                });
                local_destination_markers.push(LocalDestinationMarker {
                    start: token_start,
                    end: token_end,
                    text: caret.token.text.to_string(),
                    mode: EditorMode::PomodoroLink,
                    route: route.clone(),
                    block_id: block_id.clone(),
                    section: section.clone(),
                });
                local_destination_marker = local_destination_markers
                    .last()
                    .map(|marker| marker.text.clone());
                if let Some(message) = conflict {
                    diagnostics.push(Diagnostic {
                        severity: Severity::Error,
                        code: "invalid_pomodoro_link",
                        message,
                        range: Some((token_start, token_end)),
                    });
                } else {
                    mode = EditorMode::Incomplete;
                    needs = vec![Need::PomodoroCloseTask];
                }
            }
            CaretItemKind::CloseInvalid {
                message,
                range,
                conflict,
            } => {
                // A malformed selection reports `pomodoro_link` with a
                // precise `invalid_pomodoro_close` diagnostic and no spec.
                // An item conflict (extra text, child lines) wins instead,
                // exactly like execution.
                mode = EditorMode::PomodoroLink;
                if caret.solo_parent {
                    body = String::new();
                }
                if let Some(conflict) = conflict {
                    diagnostics.push(Diagnostic {
                        severity: Severity::Error,
                        code: "invalid_pomodoro_link",
                        message: conflict,
                        range: Some((token_start, token_end)),
                    });
                } else {
                    diagnostics.push(Diagnostic {
                        severity: Severity::Error,
                        code: "invalid_pomodoro_close",
                        message,
                        range: Some((
                            token_start + range.0,
                            token_start + range.1,
                        )),
                    });
                }
            }
            CaretItemKind::Partial {
                route: partial_route,
            } => {
                mode = EditorMode::Incomplete;
                body = String::new();
                route = partial_route;
                section = None;
                block_id = None;
                needs = vec![Need::ActiveTask];
                pomodoro_start = None;
                pomodoro_close = None;
                spans.push(Span {
                    start: token_start,
                    end: token_end,
                    kind: SpanKind::InteractivePlaceholder,
                });
            }
            CaretItemKind::NamePartial {
                route: partial_route,
                block_id: partial_block,
            } => {
                mode = EditorMode::Incomplete;
                body = String::new();
                route = Some(partial_route);
                section = None;
                block_id = Some(partial_block);
                needs = vec![Need::PomodoroName];
                pomodoro_start = None;
                pomodoro_close = None;
                let route_end =
                    token_start + 1 + route.as_deref().unwrap_or("").len();
                spans.push(Span {
                    start: token_start,
                    end: route_end,
                    kind: SpanKind::ActiveTaskRoute,
                });
                spans.push(Span {
                    start: route_end + 1,
                    end: token_end - 1,
                    kind: SpanKind::ActiveTaskBlockId,
                });
                spans.push(Span {
                    start: token_end - 1,
                    end: token_end,
                    kind: SpanKind::InteractivePlaceholder,
                });
            }
            CaretItemKind::Invalid { message } => {
                mode = EditorMode::PomodoroLink;
                if caret.solo_parent {
                    body = String::new();
                }
                // Close-suffix conflicts keep the `pomodoro_link` mode but
                // report `invalid_pomodoro_close`, mirroring the `@` path:
                // the `#name` component is the precise range for `#name=x`.
                let (code, range) = if message.starts_with("`=x` always closes")
                {
                    let text = caret.token.text;
                    let hash = text.find('#').unwrap_or(text.len());
                    let eq = text.find('=').unwrap_or(text.len());
                    (
                        "invalid_pomodoro_close",
                        Some((token_start + hash, token_start + eq)),
                    )
                } else if message == POMODORO_CLOSE_PROJECT_NOTE_ERROR {
                    ("invalid_pomodoro_close", Some((token_start, token_end)))
                } else {
                    ("invalid_pomodoro_link", Some((token_start, token_end)))
                };
                diagnostics.push(Diagnostic {
                    severity: Severity::Error,
                    code,
                    message,
                    range,
                });
            }
        }
    }

    // Dependency ownership (`dependency_target`), incomplete needs, and
    // mode-specific rejections. An explicit `@route+id` selects the
    // existing parent even with child-bullet text; a bare solo
    // `@route:id` alongside modifiers is the narrow colon alias for the
    // same; anything else with a dependent resolves to a new task (or an
    // ownerless incomplete), while section bullets, note constructions,
    // ledger links, and session operators reject the modifiers.
    let mut dependency_target: Option<DependencyTarget> = None;
    if !item_dependencies.is_empty() {
        for invalid in &item_dependencies.invalid {
            diagnostics.push(Diagnostic {
                severity: Severity::Error,
                code: "invalid_dependency",
                message: invalid.message.clone(),
                range: Some((invalid.start, invalid.end)),
            });
        }
        let parent_marker = parent_parse.marker.as_ref();
        let aliased = mode == EditorMode::PomodoroLink
            && !has_caret_item
            && parent_marker.is_some_and(|marker| {
                marker.mode == EditorMode::PomodoroTask
                    && marker.section.is_none()
                    && marker.pomodoro_start.is_none()
                    && marker.pomodoro_close.is_none()
            })
            && body.is_empty()
            && sub_bullets.is_empty();
        if has_caret_item {
            diagnostics
                .push(dependency_unsupported_target_diagnostic(mode, None));
        } else if has_explicit_toggle {
            diagnostics.push(invalid_dependency_target_diagnostic(
                local_destination_marker.as_deref().unwrap_or("@route+id"),
                spans.iter().find_map(|span| {
                    (span.kind == SpanKind::TaskToggleExplicitToggle)
                        .then_some((span.start, span.end))
                }),
            ));
        } else if mode == EditorMode::TaskToggle
            && parent_marker.is_some_and(|marker| {
                matches!(marker.mode, EditorMode::SubBullet)
            })
            && let Some(target) = existing_dependency_target(&route, &block_id)
        {
            mode = EditorMode::TaskDependency;
            if let Some(index) = own_local_destination_marker_index {
                local_destination_markers[index].mode =
                    EditorMode::TaskDependency;
            }
            dependency_target = Some(target);
        } else if aliased
            && let Some(target) = existing_dependency_target(&route, &block_id)
        {
            mode = EditorMode::TaskDependency;
            if let Some(index) = own_local_destination_marker_index {
                local_destination_markers[index].mode =
                    EditorMode::TaskDependency;
            }
            dependency_target = Some(target);
        } else if mode == EditorMode::PomodoroLink {
            diagnostics.push(invalid_dependency_target_diagnostic(
                local_destination_marker.as_deref().unwrap_or("@route:id"),
                local_destination_markers
                    .first()
                    .map(|marker| (marker.start, marker.end)),
            ));
        } else if mode == EditorMode::SubBullet
            && let Some(target) = existing_dependency_target(&route, &block_id)
        {
            dependency_target = Some(target);
        } else if mode == EditorMode::Incomplete
            && body.is_empty()
            && block_id.is_none()
            && let Some(pending) = pending_colon_dependency_target(
                &route,
                parent_parse.marker_text.as_deref(),
            )
        {
            dependency_target = Some(pending);
        } else if matches!(mode, EditorMode::Task | EditorMode::PomodoroTask) {
            if body.is_empty() && !has_local_destination {
                mode = EditorMode::Incomplete;
            } else {
                dependency_target = Some(DependencyTarget {
                    kind: DependencyTargetKind::NewTask,
                    route: route.clone(),
                    block_id: block_id.clone(),
                    inherited: false,
                });
            }
        } else if mode != EditorMode::Incomplete {
            diagnostics
                .push(dependency_unsupported_target_diagnostic(mode, None));
        }
        if item_dependencies.has_partial()
            && !needs.contains(&Need::TaskDependency)
        {
            needs.push(Need::TaskDependency);
        }
        if dependency_target.is_none()
            && !matches!(
                mode,
                EditorMode::Task
                    | EditorMode::PomodoroTask
                    | EditorMode::SubBullet
                    | EditorMode::TaskDependency
            )
            && !needs.contains(&Need::DependencyTarget)
        {
            needs.push(Need::DependencyTarget);
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
            pomodoro_start,
            pomodoro_adjust: None,
            pomodoro_shift: None,
            pomodoro_close,
            dependencies: item_dependencies.dependency_entries(),
            dependency_target,
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
/// (still being typed, e.g. a bare `@`) is not one of the eight local
/// destination marker forms `rewrite_draft` reasons about, so it is filtered
/// out here rather than at every call site.
pub(super) fn complete_local_destination_marker(
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
