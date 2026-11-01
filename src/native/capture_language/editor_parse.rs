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
use crate::native::url_routing::{classify_token, RoutingEntry};

/// Remap one physical line's own tokenizer output into the original
/// multi-line text's byte offsets, so every span an editor receives always
/// indexes the raw text the user is looking at.
pub(crate) fn tokenize_line_with_spans<'a>(
    line: &RawLine<'a>,
) -> Vec<Token<'a>> {
    let tokens = tokenize_with_spans(line.text)
        .into_iter()
        .map(|token| Token {
            text: token.text,
            start: token.start + line.start,
            end: token.end + line.start,
        })
        .collect();
    // A spaced `#"…"` literal merges into the leading marker token so the
    // name lexes whole in execution, the editor, and completion alike.
    super::pomodoro_literal::merge_leading_literal_token(line, tokens)
}

fn is_valid_drawer_section(section: &str) -> bool {
    if section.is_empty() {
        return true;
    }
    if section.ends_with('!')
        && !section.starts_with('"')
        && !section.starts_with('“')
    {
        return false;
    }
    if section.starts_with('"') || section.starts_with('“') {
        use super::pomodoro_literal::{
            lex_exact_name_after_hash, opens_exact_name,
        };
        if !opens_exact_name(section) {
            return false;
        }
        let Some(lex) = lex_exact_name_after_hash(section, section.len())
        else {
            return false;
        };
        if !lex.complete {
            return true;
        }
        let Some(canonical) =
            crate::native::capture_drawers::normalize_drawer_literal(
                &lex.decoded,
            )
        else {
            return false;
        };
        let _ = canonical;
        return section.len() == lex.literal_len;
    }
    crate::native::capture_drawers::is_drawer_slug(section)
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
    // Dependency modifiers interleave with destination,
    // schedule/priority, and `%` directives in either order, so they leave
    // the token stream before terminal-marker extraction and marker
    // selection see it. The remaining tokens resolve exactly like a draft
    // without them.
    let (stripped, dependencies) =
        extract_line_dependencies(tokens, line_text, line_base, leading);
    tokens = stripped;
    let (markers, marker_spans) = extract_terminal_markers(&mut tokens);
    let mut terminal_spans: Vec<Span> = marker_spans
        .into_iter()
        .map(|(kind, start, end)| Span {
            start,
            end,
            kind,
            project_note: None,
            title: None,
            key: None,
        })
        .collect();
    terminal_spans.extend(dependencies.spans.iter().cloned());

    let mut diagnostics = Vec::new();
    if let Some(diagnostic) = retired_priority_marker_diagnostic(&markers) {
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
    // A consumed `\&`/`\*`/`\$` escape leaves the visible marker in task
    // text. Priority unescaping only applies in the terminal region.
    let body = body_tokens
        .iter()
        .map(|token| {
            let text = unescape_dependency_text(
                token.text,
                token.start,
                &dependencies.escapes,
            );
            unescape_priority_text(
                &text,
                Some((token.start, token.end)),
                &markers.priority_escapes,
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
    pub(super) type_marker: bool,
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
                SpanKind::Directive => &mut self.clip,
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

    /// Track a type marker token: the first one claims the item, and every
    /// later one is a duplicate with the execution teaching copy.
    pub(super) fn absorb_type_marker(
        &mut self,
        range: (usize, usize),
        diagnostics: &mut Vec<Diagnostic>,
    ) {
        if self.type_marker {
            diagnostics.push(duplicate_capture_marker_diagnostic(
                duplicate_marker_error("type marker (#<type>)"),
                range,
            ));
        }
        self.type_marker = true;
    }
}

pub(super) fn terminal_marker_label(kind: SpanKind) -> &'static str {
    match kind {
        SpanKind::Schedule => "schedule marker (s:<N>)",
        SpanKind::Priority => "priority marker ($<N>)",
        SpanKind::Directive => "directive (%c)",
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
/// Unlike [`parse_capture_text_with_options`] this never fails: an
/// incomplete interactive marker (`@`, `@#`, `@route#`, `@:`, `@route:`,
/// `@route:id#`, `@route:#name`, `@^`, `@route^`, `@+`, `@route+`, `@@`,
/// `@@route`, and their legacy `@!` aliases) is a valid editing state,
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
    parse_for_editor_with(raw_text, &EditorParseOptions::routing_off())
}

pub(crate) fn parse_for_editor_with(
    raw_text: &str,
    options: &EditorParseOptions<'_>,
) -> EditorParse {
    let draft = split_capture_draft(raw_text);
    let mut global_spans = Vec::new();
    let mut global_diagnostics = Vec::new();
    let options = EditorParseOptions {
        has_global_destination: options.has_global_destination
            || !draft.declarations.is_empty(),
        ..*options
    };
    let mut item_outcomes = draft
        .items
        .iter()
        .map(|item| parse_editor_item_with(item, &options))
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
    // A plain `@@route` upgrades default (routeless) Refs to the global
    // parent; an explicit local `@route` keeps local precedence.
    if let Some(global_ref) = &global_destination {
        for outcome in &mut item_outcomes {
            if outcome.item.mode == EditorMode::Ref
                && outcome.item.route.is_none()
            {
                outcome.item.route = global_ref.route.clone();
            }
        }
    }
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
            // Whole-item session operators, `x[<N>][!<M>]` closes
            // (including dangling-separator editing states), `=`/`=<X>`
            // starts, `=<X>#` incomplete named starts (which carry
            // the partial `pomodoro_start` spec), `:` task-link queries,
            // and `!` completion queries are their own mode: a
            // `@@` declaration routes ordinary items in the same draft
            // but never turns an operator, close, start, or query into a task
            // or changes its destination.
            if item.mode == EditorMode::PomodoroAdjust
                || item.mode == EditorMode::PomodoroShift
                || item.mode == EditorMode::PomodoroClose
                || item.mode == EditorMode::PomodoroStart
                || item.mode == EditorMode::Ref
                || item.mode == EditorMode::TaskComplete
                || item.needs == [Need::TaskLink]
                || item.needs == [Need::TaskGoto]
                || item.needs == [Need::TaskComplete]
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
            block_id: None,
            needs: global_destination
                .as_ref()
                .map(|global| global.needs.clone())
                .unwrap_or_default(),
            ref_listen: false,
            pomodoro_start: None,
            pomodoro_adjust: None,
            pomodoro_shift: None,
            pomodoro_close: None,
            dependencies: Vec::new(),
            blocked_tasks: Vec::new(),
            relations: Vec::new(),
            dependency_target: None,
            task_complete: None,
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
    let ref_listen = first.ref_listen;
    let pomodoro_start = first.pomodoro_start.clone();
    let pomodoro_adjust = first.pomodoro_adjust.clone();
    let pomodoro_shift = first.pomodoro_shift.clone();
    let pomodoro_close = first.pomodoro_close.clone();
    let dependencies = first.dependencies.clone();
    let blocked_tasks = first.blocked_tasks.clone();
    let relations = first.relations.clone();
    let dependency_target = first.dependency_target.clone();
    let task_complete = first.task_complete.clone();
    let sub_bullets = first.sub_bullets.clone();
    let mut spans = global_spans;
    spans.extend(items.iter().flat_map(|item| item.spans.iter().cloned()));
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
        ref_listen,
        pomodoro_start,
        pomodoro_adjust,
        pomodoro_shift,
        pomodoro_close,
        dependencies,
        blocked_tasks,
        relations,
        dependency_target,
        task_complete,
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
    let had_blocked_task_need = item.needs.contains(&Need::BlockedTask);
    let had_target_need = item.needs.contains(&Need::DependencyTarget);
    item.mode = global.mode;
    item.route = global.route.clone();
    item.section = None;
    item.block_id = None;
    item.needs = global.needs.clone();
    if item.dependencies.is_empty()
        && item.blocked_tasks.is_empty()
        && !had_task_dependency_need
        && !had_blocked_task_need
        && !had_target_need
    {
        return;
    }
    if had_task_dependency_need && !item.needs.contains(&Need::TaskDependency) {
        item.needs.push(Need::TaskDependency);
    }
    if had_blocked_task_need && !item.needs.contains(&Need::BlockedTask) {
        item.needs.push(Need::BlockedTask);
    }
    if let Some(target) = item.dependency_target.as_mut() {
        // A bare `@@route` routes a new dependent without naming a parent.
        if target.kind == DependencyTargetKind::NewTask
            && target.route.is_none()
        {
            target.route = global.route.clone();
            target.inherited = global.route.is_some();
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
    if !(marker.starts_with('@') || marker.starts_with('^'))
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
        EditorMode::TaskComplete => "task-complete",
        EditorMode::Ref => "reference",
        EditorMode::PomodoroLink | EditorMode::PomodoroTask => {
            "Pomodoro-ledger-link"
        }
        EditorMode::TaskToggle
        | EditorMode::SubBullet
        | EditorMode::TaskDependency => "task-toggle",
        EditorMode::Task | EditorMode::Incomplete => "task",
    }
}

fn dependency_unsupported_target_diagnostic(
    mode: EditorMode,
    range: Option<(usize, usize)>,
    sigil: u8,
) -> Diagnostic {
    Diagnostic {
        severity: Severity::Error,
        code: "dependency_unsupported_target",
        message: format!(
            "task relationships need a task owner: '{sigil}note:block-id' cannot \
             attach to a {} capture (add task text or '@note+task-id' for \
             the dependent)",
            dependency_owner_mode_word(mode),
            sigil = sigil as char,
        ),
        range,
    }
}

fn invalid_dependency_target_diagnostic(
    marker_text: &str,
    range: Option<(usize, usize)>,
    sigil: u8,
) -> Diagnostic {
    Diagnostic {
        severity: Severity::Error,
        code: "invalid_dependency_target",
        message: format!(
            "relationship-only '{marker_text}' must be bare: it mixes a \
             Pomodoro action with '{sigil}note:block-id' (capture them as \
             separate blank-line items)",
            sigil = sigil as char
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
    let found = scan_item_dependencies(item);
    if found.is_empty() {
        return outcome;
    }
    outcome.item.dependencies = found.dependency_entries();
    outcome.item.blocked_tasks = found.blocked_task_entries();
    outcome.item.relations = found.relation_entries();
    for invalid in found.invalid.iter().chain(found.blocked_invalid.iter()) {
        outcome.item.diagnostics.push(Diagnostic {
            severity: Severity::Error,
            code: "invalid_dependency",
            message: invalid.message.clone(),
            range: Some((invalid.start, invalid.end)),
        });
    }
    for invalid in found.relation_invalid.iter() {
        outcome.item.diagnostics.push(Diagnostic {
            severity: Severity::Error,
            code: "invalid_relation",
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
            found.sigil(),
        ));
    if !found.partials.is_empty()
        && !outcome.item.needs.contains(&Need::TaskDependency)
    {
        outcome.item.needs.push(Need::TaskDependency);
    }
    if !found.blocked_partials.is_empty()
        && !outcome.item.needs.contains(&Need::BlockedTask)
    {
        outcome.item.needs.push(Need::BlockedTask);
    }
    if !found.relation_target_partials.is_empty()
        && !outcome.item.needs.contains(&Need::RelationTarget)
    {
        outcome.item.needs.push(Need::RelationTarget);
    }
    if !found.relation_label_partials.is_empty()
        && !outcome.item.needs.contains(&Need::RelationLabel)
    {
        outcome.item.needs.push(Need::RelationLabel);
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
            ref_listen: false,
            pomodoro_start: None,
            pomodoro_adjust: None,
            pomodoro_shift: None,
            pomodoro_close: None,
            dependencies: Vec::new(),
            blocked_tasks: Vec::new(),
            relations: Vec::new(),
            dependency_target: None,
            task_complete: None,
            spans: vec![Span {
                start: token.start,
                end: token.end,
                kind: SpanKind::InteractivePlaceholder,
                project_note: None,
                title: None,
                key: None,
            }],
            diagnostics: Vec::new(),
            sub_bullets: Vec::new(),
            has_local_destination: true,
            local_destination_markers: Vec::new(),
        },
        declarations: Vec::new(),
    })
}

/// A whole-draft `/query` Go To picker search: a single-line,
/// single-token item holding the whole draft reports `incomplete` needing
/// `task_goto` with one `interactive_placeholder` span over the whole token
/// (sigil included) and no diagnostics. A `@@` declaration never applies
/// to it, exactly like the `:` picker family.
pub(super) fn parse_editor_task_goto_item<'a>(
    item: &CaptureItem<'a>,
) -> Option<EditorItemOutcome<'a>> {
    let token = task_goto_query_token(item)?;
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
            needs: vec![Need::TaskGoto],
            ref_listen: false,
            pomodoro_start: None,
            pomodoro_adjust: None,
            pomodoro_shift: None,
            pomodoro_close: None,
            dependencies: Vec::new(),
            blocked_tasks: Vec::new(),
            relations: Vec::new(),
            dependency_target: None,
            task_complete: None,
            spans: vec![Span {
                start: token.start,
                end: token.end,
                kind: SpanKind::InteractivePlaceholder,
                project_note: None,
                title: None,
                key: None,
            }],
            diagnostics: Vec::new(),
            sub_bullets: Vec::new(),
            has_local_destination: true,
            local_destination_markers: Vec::new(),
        },
        declarations: Vec::new(),
    })
}

/// A whole-item `!` token: a still-typing query reports `incomplete`
/// needing `task_complete` with one `interactive_placeholder` span over
/// the whole token (sigil included); a complete token reports
/// `task_complete` with sigil/note/block-ID spans and the per-item
/// `task_complete` object; a claimed invalid token reports `task_complete`
/// with an `invalid_task_complete` diagnostic, mirroring how a padded
/// `^route:id` keeps `pomodoro_link` with its diagnostic. A `@@`
/// declaration never applies to it, exactly like the `:` picker family.
pub(super) fn parse_editor_task_complete_item<'a>(
    item: &CaptureItem<'a>,
) -> Option<EditorItemOutcome<'a>> {
    let claim = claim_bang_item(item)?;
    let outcome = |parsed: EditorItemParse| EditorItemOutcome {
        item: parsed,
        declarations: Vec::new(),
    };
    let base_item = |mode: EditorMode,
                     needs: Vec<Need>,
                     block_id: Option<String>,
                     task_complete: Option<TaskCompleteEntry>,
                     spans: Vec<Span>,
                     diagnostics: Vec<Diagnostic>| {
        outcome(EditorItemParse {
            index: item.index,
            start: item.start,
            end: item.end,
            line_start: item.line_start,
            line_end: item.line_end,
            body: String::new(),
            mode,
            route: None,
            section: None,
            block_id,
            needs,
            ref_listen: false,
            pomodoro_start: None,
            pomodoro_adjust: None,
            pomodoro_shift: None,
            pomodoro_close: None,
            dependencies: Vec::new(),
            blocked_tasks: Vec::new(),
            relations: Vec::new(),
            dependency_target: None,
            task_complete,
            spans,
            diagnostics,
            sub_bullets: Vec::new(),
            has_local_destination: true,
            local_destination_markers: Vec::new(),
        })
    };
    match claim {
        BangClaim::Query { raw: _, start, end } => Some(base_item(
            EditorMode::Incomplete,
            vec![Need::TaskComplete],
            None,
            None,
            vec![Span {
                start,
                end,
                kind: SpanKind::InteractivePlaceholder,
                project_note: None,
                title: None,
                key: None,
            }],
            Vec::new(),
        )),
        BangClaim::Complete(dependency) => {
            let entry = TaskCompleteEntry {
                raw: dependency.raw.clone(),
                note: dependency.note.clone(),
                block_id: dependency.block_id.clone(),
                quoted: dependency.quoted,
                start: dependency.start,
                end: dependency.end,
            };
            Some(base_item(
                EditorMode::TaskComplete,
                Vec::new(),
                Some(dependency.block_id.clone()),
                Some(entry),
                vec![
                    Span {
                        start: dependency.start,
                        end: dependency.start + 1,
                        kind: SpanKind::TaskCompleteSigil,
                        project_note: None,
                        title: None,
                        key: None,
                    },
                    Span {
                        start: dependency.start + 1,
                        end: dependency.block_start - 1,
                        kind: SpanKind::TaskCompleteNote,
                        project_note: None,
                        title: None,
                        key: None,
                    },
                    Span {
                        start: dependency.block_start,
                        end: dependency.block_end,
                        kind: SpanKind::TaskCompleteBlockId,
                        project_note: None,
                        title: None,
                        key: None,
                    },
                ],
                Vec::new(),
            ))
        }
        BangClaim::Invalid(invalid) => Some(base_item(
            EditorMode::TaskComplete,
            Vec::new(),
            None,
            None,
            Vec::new(),
            vec![Diagnostic {
                severity: Severity::Error,
                code: "invalid_task_complete",
                message: invalid.message.clone(),
                range: Some((invalid.start, invalid.end)),
            }],
        )),
    }
}

/// A terminal-plus task selector is unresolved editor input. Like the
/// leading-colon picker, it is never previewed or submitted as an ordinary
/// task; the explicit selection writes a scoped `@route+block-id` marker.
/// The exact lone parent-line `+` remains the existing Pomodoro adjustment.
/// Beside `&`/`*` the outcome keeps those modifiers and does not diagnose
/// them as an unsupported operator target.
pub(super) fn parse_editor_parent_task_item<'a>(
    item: &CaptureItem<'a>,
) -> Option<EditorItemOutcome<'a>> {
    let selector = parent_task_selector_tokens(item)
        .into_iter()
        .find(|selector| !selector.dual_use_adjustment)?;
    let token = selector.token;
    let found = scan_item_dependencies(item);
    let mut needs = vec![Need::TaskParent];
    if !found.partials.is_empty() && !needs.contains(&Need::TaskDependency) {
        needs.push(Need::TaskDependency);
    }
    if !found.blocked_partials.is_empty() && !needs.contains(&Need::BlockedTask)
    {
        needs.push(Need::BlockedTask);
    }
    if !found.relation_target_partials.is_empty()
        && !needs.contains(&Need::RelationTarget)
    {
        needs.push(Need::RelationTarget);
    }
    if !found.relation_label_partials.is_empty()
        && !needs.contains(&Need::RelationLabel)
    {
        needs.push(Need::RelationLabel);
    }
    let mut diagnostics = Vec::new();
    for invalid in found.invalid.iter().chain(found.blocked_invalid.iter()) {
        diagnostics.push(Diagnostic {
            severity: Severity::Error,
            code: "invalid_dependency",
            message: invalid.message.clone(),
            range: Some((invalid.start, invalid.end)),
        });
    }
    for invalid in found.relation_invalid.iter() {
        diagnostics.push(Diagnostic {
            severity: Severity::Error,
            code: "invalid_relation",
            message: invalid.message.clone(),
            range: Some((invalid.start, invalid.end)),
        });
    }
    let parent = item.lines.first()?;
    let parent_tokens = tokenize_line_with_spans(&parent.raw);
    let (_, parent_deps) = extract_line_dependencies(
        parent_tokens,
        parent.raw.text,
        parent.raw.start,
        true,
    );
    let mut spans = parent_deps.spans;
    spans.push(Span {
        start: token.start,
        end: token.end,
        kind: SpanKind::InteractivePlaceholder,
        project_note: None,
        title: None,
        key: None,
    });
    spans.sort_by_key(|span| (span.start, span.end));
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
            needs,
            ref_listen: false,
            pomodoro_start: None,
            pomodoro_adjust: None,
            pomodoro_shift: None,
            pomodoro_close: None,
            dependencies: found.dependency_entries(),
            blocked_tasks: found.blocked_task_entries(),
            relations: found.relation_entries(),
            dependency_target: None,
            task_complete: None,
            spans,
            diagnostics,
            sub_bullets: Vec::new(),
            has_local_destination: true,
            local_destination_markers: Vec::new(),
        },
        declarations: Vec::new(),
    })
}

/// Resolve a plain `@route` token for the editor ref path.
fn editor_plain_ref_route(text: &str) -> Option<String> {
    let parsed = parse_route_token(text)?;
    let route = parsed.route?;
    if !matches!(parsed.kind, CaptureKind::Task) {
        return None;
    }
    if text.to_ascii_lowercase() != format!("@{route}") {
        return None;
    }
    if !is_route_token(&route) {
        return None;
    }
    Some(route)
}

#[allow(clippy::too_many_lines)]
/// A whole-item bare URL admitted by the routing policy: mode `ref`
/// with one `ref_url` span over the URL token, `<>` included, plus an
/// ordinary `route` span when exactly one plain `@route` token leads or
/// trails it. When `listen` is true (the `%l` directive was typed) the
/// claim produces a listen reference; otherwise a plain reference.
/// Anything else (extra text, child lines, markers, `@@`) falls through
/// to the generic path, exactly like execution. A bare URL remains
/// submittable and never acquires a required `needs` entry merely because
/// a client can offer a parent picker.
fn parse_editor_ref_item<'a>(
    item: &CaptureItem<'a>,
    options: &EditorParseOptions<'_>,
    listen: bool,
) -> Option<EditorItemOutcome<'a>> {
    let policy = options.url_routing?;
    if item.lines.len() != 1 {
        return None;
    }
    if !scan_item_dependencies(item).is_empty() {
        return None;
    }
    let parent = item.lines.first()?.raw;
    let tokens = tokenize_line_with_spans(&parent);
    if tokens.iter().any(|token| token.text.starts_with("@@")) {
        return None;
    }
    if tokens.len() == 1 {
        let token = tokens[0];
        let intent = classify_token(token.text)?;
        if !policy.admits(&intent, RoutingEntry::Capture) {
            return None;
        }
        return Some(EditorItemOutcome {
            item: EditorItemParse {
                index: item.index,
                start: item.start,
                end: item.end,
                line_start: item.line_start,
                line_end: item.line_end,
                body: token.text.to_string(),
                mode: EditorMode::Ref,
                route: None,
                section: None,
                block_id: None,
                needs: Vec::new(),
                ref_listen: listen,
                pomodoro_start: None,
                pomodoro_adjust: None,
                pomodoro_shift: None,
                pomodoro_close: None,
                dependencies: Vec::new(),
                blocked_tasks: Vec::new(),
                relations: Vec::new(),
                dependency_target: None,
                task_complete: None,
                spans: vec![Span {
                    start: token.start,
                    end: token.end,
                    kind: SpanKind::RefUrl,
                    project_note: None,
                    title: None,
                    key: None,
                }],
                diagnostics: Vec::new(),
                sub_bullets: Vec::new(),
                has_local_destination: false,
                local_destination_markers: Vec::new(),
            },
            declarations: Vec::new(),
        });
    }
    if tokens.len() == 2 {
        let first_intent = classify_token(tokens[0].text);
        let second_intent = classify_token(tokens[1].text);
        let (url_token, route_token) =
            if first_intent.is_some() && second_intent.is_none() {
                (tokens[0], tokens[1])
            } else if first_intent.is_none() && second_intent.is_some() {
                (tokens[1], tokens[0])
            } else {
                return None;
            };
        let intent = classify_token(url_token.text)?;
        if !policy.admits(&intent, RoutingEntry::Capture) {
            return None;
        }
        let route = editor_plain_ref_route(route_token.text)?;
        return Some(EditorItemOutcome {
            item: EditorItemParse {
                index: item.index,
                start: item.start,
                end: item.end,
                line_start: item.line_start,
                line_end: item.line_end,
                body: url_token.text.to_string(),
                mode: EditorMode::Ref,
                route: Some(route.clone()),
                section: None,
                block_id: None,
                needs: Vec::new(),
                ref_listen: listen,
                pomodoro_start: None,
                pomodoro_adjust: None,
                pomodoro_shift: None,
                pomodoro_close: None,
                dependencies: Vec::new(),
                blocked_tasks: Vec::new(),
                relations: Vec::new(),
                dependency_target: None,
                task_complete: None,
                spans: vec![
                    Span {
                        start: url_token.start,
                        end: url_token.end,
                        kind: SpanKind::RefUrl,
                        project_note: None,
                        title: None,
                        key: None,
                    },
                    Span {
                        start: route_token.start,
                        end: route_token.end,
                        kind: SpanKind::Route,
                        project_note: None,
                        title: None,
                        key: None,
                    },
                ],
                diagnostics: Vec::new(),
                sub_bullets: Vec::new(),
                has_local_destination: true,
                local_destination_markers: vec![LocalDestinationMarker {
                    start: route_token.start,
                    end: route_token.end,
                    text: route_token.text.to_string(),
                    mode: EditorMode::Ref,
                    route: Some(route),
                    block_id: None,
                    section: None,
                }],
            },
            declarations: Vec::new(),
        });
    }
    None
}

pub(super) fn parse_editor_item<'a>(
    item: &CaptureItem<'a>,
) -> EditorItemOutcome<'a> {
    parse_editor_item_with(item, &EditorParseOptions::routing_off())
}

pub(super) fn parse_editor_item_with<'a>(
    item: &CaptureItem<'a>,
    options: &EditorParseOptions<'_>,
) -> EditorItemOutcome<'a> {
    let occurrences = super::type_marker::collect_item(item);
    let type_masked_source =
        super::type_marker::masked_source(item, &occurrences);
    let type_masked_item =
        super::type_marker::masked_item(item, &type_masked_source);
    let directive_occurrences =
        super::directive::collect_item(&type_masked_item);
    let resolved = super::directive::resolve(&directive_occurrences).ok();
    let has_clip = resolved
        .as_ref()
        .and_then(|directives| directives.clip.clone())
        .is_some();
    let has_note = resolved.as_ref().is_some_and(|directives| directives.note);
    let has_listen = resolved
        .as_ref()
        .is_some_and(|directives| directives.listen);
    if occurrences.is_empty() && directive_occurrences.is_empty() {
        return parse_editor_item_inner(item, options, false, false);
    }
    // Type markers and % directives are transparent to every existing
    // claim: run the established parse on the offset-preserving masked
    // view (marker bytes become spaces, so every range still indexes the
    // original draft), then re-slice borrowed declarations back to the
    // original text and attach each marker's spans, needs, and diagnostics.
    let masked_source = super::directive::masked_source(
        &type_masked_item,
        &directive_occurrences,
    );
    let skips = super::type_marker::marker_only_child_lines(item, &occurrences);
    let mut masked_item =
        super::directive::masked_item(&type_masked_item, &masked_source);
    masked_item
        .lines
        .retain(|line| !skips.contains(&(line.raw.start, line.raw.end)));
    let inner =
        parse_editor_item_inner(&masked_item, options, has_clip, has_listen);
    let declarations =
        remap_type_marker_declarations(inner.declarations, item.source);
    let mut outcome = EditorItemOutcome {
        item: inner.item,
        declarations,
    };
    if has_note {
        apply_note_directive(&mut outcome.item);
    }
    attach_type_marker_metadata(&mut outcome.item, &occurrences);
    attach_directive_metadata(&mut outcome.item, &directive_occurrences);
    if has_listen {
        apply_listen_directive(&mut outcome.item);
    }
    outcome
}

fn apply_note_directive(parsed: &mut EditorItemParse) {
    if parsed.mode == EditorMode::PomodoroNote {
        return;
    }
    if parsed.route.is_some() || parsed.has_local_destination {
        parsed.diagnostics.push(Diagnostic {
            severity: Severity::Error,
            code: "pomodoro_note_conflict",
            message: pomodoro_note_route_conflict_error(),
            range: None,
        });
        return;
    }
    parsed.mode = EditorMode::PomodoroNote;
    parsed.route = None;
    parsed.section = None;
    parsed.block_id = None;
    parsed.has_local_destination = false;
    parsed.local_destination_markers.clear();
}

fn apply_listen_directive(parsed: &mut EditorItemParse) {
    if parsed.mode == EditorMode::Ref && parsed.ref_listen {
        return;
    }
    // `%l` only applies to bare-URL reference items; anything else gets
    // the directive error instead of silently capturing as a task.
    if parsed
        .diagnostics
        .iter()
        .any(|diagnostic| diagnostic.code == "directive_not_applicable")
    {
        return;
    }
    let range = parsed
        .spans
        .iter()
        .find(|span| span.kind == SpanKind::Directive)
        .map(|span| (span.start, span.end));
    parsed.diagnostics.push(Diagnostic {
        severity: Severity::Error,
        code: "directive_not_applicable",
        message: super::directive::directive_not_applicable_error(),
        range,
    });
}

/// Re-slice borrowed global declarations back to the original draft text.
/// Masking preserves byte offsets, so only the borrowed slices change.
fn remap_type_marker_declarations<'a>(
    declarations: Vec<GlobalDeclarationToken<'_>>,
    source: &'a str,
) -> Vec<GlobalDeclarationToken<'a>> {
    declarations
        .into_iter()
        .filter_map(|declaration| {
            Some(GlobalDeclarationToken {
                token: Token {
                    text: source
                        .get(declaration.token.start..declaration.token.end)?,
                    start: declaration.token.start,
                    end: declaration.token.end,
                },
                line_number: declaration.line_number,
            })
        })
        .collect()
}

/// Attach one item's collected type marker to its masked editor parse:
/// a whole-token `task_type` span for every marker token, then the same
/// precedence execution uses — duplicates, then incomplete/invalid shapes,
/// then the subject judgment. Query, dependency, and other incomplete modes
/// keep their own needs: only the span is added there.
fn attach_type_marker_metadata(
    parsed: &mut EditorItemParse,
    occurrences: &[super::type_marker::TypeMarkerOccurrence],
) {
    use super::type_marker::{
        invalid_marker_error, needs_task_error,
        type_marker_not_applicable_error, TypeMarkerOccurrence,
    };
    let markers = occurrences
        .iter()
        .filter(|occurrence| occurrence.is_marker())
        .collect::<Vec<_>>();
    if markers.is_empty() {
        return;
    }
    let mut seen = SeenMarkers::default();
    for marker in &markers {
        let span = marker.span();
        seen.absorb_type_marker(
            (span.start, span.end),
            &mut parsed.diagnostics,
        );
        parsed.spans.push(Span {
            start: span.start,
            end: span.end,
            kind: SpanKind::TaskType,
            project_note: None,
            title: None,
            key: None,
        });
    }
    // Like execution, duplicates refuse before any subject judgment.
    if markers.len() > 1 {
        parsed.spans.sort_by_key(|span| (span.start, span.end));
        return;
    }
    match markers[0] {
        TypeMarkerOccurrence::Complete(marker) => {
            // A link-close form refuses as a close gesture, not as an
            // existing link.
            if parsed.mode == EditorMode::PomodoroLink
                && parsed.pomodoro_close.is_some()
            {
                parsed.diagnostics.push(Diagnostic {
                    severity: Severity::Error,
                    code: "type_marker_not_applicable",
                    message: type_marker_not_applicable_error(
                        &marker.name,
                        super::type_marker::TypeMarkerSubject::Close,
                    ),
                    range: Some((marker.span.start, marker.span.end)),
                });
                parsed.spans.sort_by_key(|span| (span.start, span.end));
                return;
            }
            // A relationship-only SubBullet owner accepts the marker.
            let relationship_sub_bullet = parsed.mode == EditorMode::SubBullet
                && parsed.body.trim().is_empty()
                && parsed.sub_bullets.is_empty()
                && matches!(
                    parsed.dependency_target.as_ref(),
                    Some(target) if target.kind
                        == DependencyTargetKind::ExistingTask
                );
            // A form with no body never parses masked: execution fails it
            // with missing text, so the marker needs a task instead of
            // refusing a capture that does not exist yet.
            let needs_task_instead = parsed.body.trim().is_empty()
                && parsed.pomodoro_close.is_none()
                && !relationship_sub_bullet
                && matches!(
                    parsed.mode,
                    EditorMode::Task
                        | EditorMode::PomodoroTask
                        | EditorMode::Bullet
                        | EditorMode::SubBullet
                        | EditorMode::ProjectNote
                        | EditorMode::PomodoroProjectNote
                );
            if needs_task_instead {
                parsed.diagnostics.push(Diagnostic {
                    severity: Severity::Error,
                    code: "type_marker_needs_task",
                    message: needs_task_error(Some(&marker.name)),
                    range: Some((marker.span.start, marker.span.end)),
                });
                if !parsed.needs.contains(&Need::Task) {
                    parsed.needs.push(Need::Task);
                }
            } else if !relationship_sub_bullet
                && let Some(subject) = type_marker_subject_for_editor(
                    parsed.mode,
                    parsed.pomodoro_close.is_some(),
                )
            {
                parsed.diagnostics.push(Diagnostic {
                    severity: Severity::Error,
                    code: "type_marker_not_applicable",
                    message: type_marker_not_applicable_error(
                        &marker.name,
                        subject,
                    ),
                    range: Some((marker.span.start, marker.span.end)),
                });
            }
        }
        TypeMarkerOccurrence::Incomplete { .. } => {
            if !parsed.needs.contains(&Need::TaskType) {
                parsed.needs.push(Need::TaskType);
            }
            parsed.mode = EditorMode::Incomplete;
        }
        TypeMarkerOccurrence::Invalid { raw, .. } => {
            let span = markers[0].span();
            parsed.diagnostics.push(Diagnostic {
                severity: Severity::Error,
                code: "invalid_type_marker",
                message: invalid_marker_error(raw),
                range: Some((span.start, span.end)),
            });
        }
        TypeMarkerOccurrence::Escaped { .. } => {}
    }
    parsed.spans.sort_by_key(|span| (span.start, span.end));
}

fn attach_drawer_bullet_metadata(
    item: &CaptureItem<'_>,
    mode: &mut EditorMode,
    parent_body: &str,
    glued_section: Option<&str>,
    needs: &mut Vec<Need>,
    spans: &mut Vec<Span>,
    diagnostics: &mut Vec<Diagnostic>,
    sub_bullets: &mut [AuthoredSubBullet],
) {
    use super::drawer_bullet::{
        editor_drawer_bullet_position_error, first_level_groups,
        is_all_caps_drawer_title, is_under_task_mode, scan_leading,
        DrawerBulletHit,
    };

    let under_task = is_under_task_mode(*mode);
    let mut authored_lines: Vec<(AuthoredDepth, &str, usize)> = Vec::new();
    for line in item.lines.iter().skip(1) {
        let AuthoredLineClass::Item(authored) =
            classify_authored_line(line.raw)
        else {
            continue;
        };
        authored_lines.push((
            authored.depth,
            authored.body,
            authored.body_start,
        ));
    }

    let mut index = 0usize;
    while index < authored_lines.len() {
        if authored_lines[index].0 != AuthoredDepth::First {
            index += 1;
            continue;
        }
        let start = index;
        index += 1;
        while index < authored_lines.len()
            && authored_lines[index].0 == AuthoredDepth::Nested
        {
            index += 1;
        }
        let nested = index - start > 1;
        let (_, body_text, body_start) = authored_lines[start];
        match scan_leading(body_text, body_start) {
            Some(DrawerBulletHit::Complete {
                span,
                title,
                key,
                leftover,
                ..
            }) => {
                spans.push(Span {
                    start: span.start,
                    end: span.end,
                    kind: SpanKind::Drawer,
                    project_note: None,
                    title: Some(title),
                    key: Some(key),
                });
                if !leftover.is_empty() {
                    continue;
                }
                if !under_task {
                    diagnostics.push(Diagnostic {
                        severity: Severity::Error,
                        code: "drawer_bullet_position",
                        message: editor_drawer_bullet_position_error(
                            *mode,
                            parent_body,
                            glued_section,
                        ),
                        range: Some((span.start, span.end)),
                    });
                    continue;
                }
                if !nested {
                    if !needs.contains(&Need::DrawerNotes) {
                        needs.push(Need::DrawerNotes);
                    }
                    *mode = EditorMode::Incomplete;
                }
            }
            Some(DrawerBulletHit::Incomplete { span }) => {
                spans.push(Span {
                    start: span.start,
                    end: span.end,
                    kind: SpanKind::InteractivePlaceholder,
                    project_note: None,
                    title: None,
                    key: None,
                });
                if !needs.contains(&Need::Drawer) {
                    needs.push(Need::Drawer);
                }
                *mode = EditorMode::Incomplete;
            }
            Some(DrawerBulletHit::Invalid { .. })
            | Some(DrawerBulletHit::Escaped { .. })
            | None => {
                if under_task && nested && is_all_caps_drawer_title(body_text) {
                    let title = body_text.trim().to_string();
                    let key =
                        crate::native::capture_drawers::drawer_key(&title);
                    let leading =
                        body_text.len() - body_text.trim_start().len();
                    let trailing = body_text.len() - body_text.trim_end().len();
                    let start = body_start + leading;
                    let end = body_start + body_text.len() - trailing;
                    spans.push(Span {
                        start,
                        end,
                        kind: SpanKind::DrawerBullet,
                        project_note: None,
                        title: Some(title),
                        key: Some(key),
                    });
                }
            }
        }
    }

    if !under_task {
        return;
    }
    for (start, end) in first_level_groups(sub_bullets) {
        if sub_bullets[start].drawer.is_some() {
            continue;
        }
        if end - start > 1 && is_all_caps_drawer_title(&sub_bullets[start].body)
        {
            let title = sub_bullets[start].body.trim().to_string();
            sub_bullets[start].drawer = Some(DrawerSelector {
                text: title,
                exact: true,
            });
        }
    }
}

fn attach_directive_metadata(
    parsed: &mut EditorItemParse,
    occurrences: &[super::directive::DirectiveOccurrence],
) {
    let markers = occurrences
        .iter()
        .filter(|occurrence| occurrence.is_marker())
        .collect::<Vec<_>>();
    if markers.is_empty() {
        return;
    }
    for marker in &markers {
        let span = marker.span();
        parsed.spans.push(Span {
            start: span.start,
            end: span.end,
            kind: SpanKind::Directive,
            project_note: None,
            title: None,
            key: None,
        });
    }
    match super::directive::resolve(occurrences) {
        Ok(directives) => {
            if directives.clip.is_some() {
                attach_clip_incompatible_kind(parsed);
            }
        }
        Err((message, code, span)) => {
            if code == "duplicate_capture_marker" {
                parsed.diagnostics.push(duplicate_capture_marker_diagnostic(
                    message,
                    (span.start, span.end),
                ));
            } else if code == "incomplete_directive" {
                if !parsed.needs.contains(&Need::Directive) {
                    parsed.needs.push(Need::Directive);
                }
                parsed.mode = EditorMode::Incomplete;
            } else {
                parsed.diagnostics.push(Diagnostic {
                    severity: Severity::Error,
                    code,
                    message,
                    range: Some((span.start, span.end)),
                });
            }
        }
    }
    parsed.spans.sort_by_key(|span| (span.start, span.end));
}

fn attach_clip_incompatible_kind(parsed: &mut EditorItemParse) {
    if parsed.diagnostics.iter().any(|diagnostic| {
        matches!(
            diagnostic.code,
            "invalid_pomodoro_link"
                | "unsupported_explicit_toggle"
                | "relationship_needs_text"
        )
    }) {
        return;
    }
    let (code, message) = match parsed.mode {
        EditorMode::PomodoroLink => (
            "invalid_pomodoro_link",
            POMODORO_LINK_CLIP_CONFLICT_ERROR.to_string(),
        ),
        EditorMode::PomodoroAdjust => (
            "invalid_pomodoro_adjust",
            "Pomodoro adjustment `+N`/`-N` cannot be combined with %c; capture the adjustment alone"
                .to_string(),
        ),
        EditorMode::PomodoroShift => (
            "invalid_pomodoro_shift",
            "Pomodoro shift `++N`/`--N` cannot be combined with %c; capture the shift alone"
                .to_string(),
        ),
        EditorMode::PomodoroClose => (
            "invalid_pomodoro_close",
            "Pomodoro close `x` cannot be combined with %c; capture the close alone"
                .to_string(),
        ),
        EditorMode::PomodoroStart => (
            "invalid_pomodoro_start",
            "Pomodoro start `=<X>` cannot be combined with %c; capture the start alone"
                .to_string(),
        ),
        EditorMode::TaskComplete => (
            "invalid_task_complete",
            "task complete cannot be combined with %c".to_string(),
        ),
        EditorMode::ProjectNote | EditorMode::PomodoroProjectNote => (
            "invalid_project_note",
            "project-note capture cannot be combined with %c".to_string(),
        ),
        _ => return,
    };
    let range = parsed
        .spans
        .iter()
        .find_map(|span| {
            matches!(
                span.kind,
                SpanKind::ActiveTaskRoute | SpanKind::PomodoroRoute
            )
            .then_some(span.start)
        })
        .and_then(|start| {
            parsed
                .spans
                .iter()
                .rev()
                .find(|span| {
                    matches!(
                        span.kind,
                        SpanKind::ActiveTaskBlockId
                            | SpanKind::PomodoroBlockId
                            | SpanKind::ActiveTaskRoute
                            | SpanKind::PomodoroRoute
                    )
                })
                .map(|span| (start, span.end))
        })
        .or_else(|| {
            parsed.spans.iter().find_map(|span| {
                (span.kind == SpanKind::Directive)
                    .then_some((span.start, span.end))
            })
        });
    parsed.diagnostics.push(Diagnostic {
        severity: Severity::Error,
        code,
        message,
        range,
    });
}

/// Map an editor mode to the shared type-marker subject judgment. New-task
/// modes are judged by their body; existing-target link, toggle, and
/// dependency modes accept the marker; incomplete, dependency, and picker
/// modes keep their own needs and get only the marker span.
fn type_marker_subject_for_editor(
    mode: EditorMode,
    has_close_gesture: bool,
) -> Option<super::type_marker::TypeMarkerSubject> {
    if has_close_gesture
        && matches!(mode, EditorMode::PomodoroTask | EditorMode::PomodoroLink)
    {
        return Some(super::type_marker::TypeMarkerSubject::Close);
    }
    match mode {
        EditorMode::Task
        | EditorMode::PomodoroTask
        | EditorMode::PomodoroLink
        | EditorMode::TaskToggle => None,
        EditorMode::Bullet => {
            Some(super::type_marker::TypeMarkerSubject::Bullet)
        }
        EditorMode::SubBullet => None,
        EditorMode::ProjectNote | EditorMode::PomodoroProjectNote => {
            Some(super::type_marker::TypeMarkerSubject::ProjectNote)
        }
        EditorMode::Ref => {
            Some(super::type_marker::TypeMarkerSubject::Reference)
        }
        EditorMode::PomodoroNote => {
            Some(super::type_marker::TypeMarkerSubject::PomodoroNote)
        }
        EditorMode::TaskComplete => {
            Some(super::type_marker::TypeMarkerSubject::TaskComplete)
        }
        EditorMode::PomodoroAdjust => {
            Some(super::type_marker::TypeMarkerSubject::Adjust)
        }
        EditorMode::PomodoroShift => {
            Some(super::type_marker::TypeMarkerSubject::Shift)
        }
        EditorMode::PomodoroClose => {
            Some(super::type_marker::TypeMarkerSubject::Close)
        }
        EditorMode::PomodoroStart => {
            Some(super::type_marker::TypeMarkerSubject::Start)
        }
        _ => None,
    }
}

pub(super) fn parse_editor_item_inner<'a>(
    item: &CaptureItem<'a>,
    options: &EditorParseOptions<'_>,
    has_clip: bool,
    has_listen: bool,
) -> EditorItemOutcome<'a> {
    if let Some(task_goto) = parse_editor_task_goto_item(item) {
        return task_goto;
    }
    if let Some(task_link) = parse_editor_task_link_item(item) {
        return task_link;
    }
    if let Some(task_complete) = parse_editor_task_complete_item(item) {
        return task_complete;
    }
    if let Some(close) = parse_editor_close_item(item) {
        return attach_operator_dependency_rejection(close, item);
    }
    if let Some(parent_task) = parse_editor_parent_task_item(item) {
        return parent_task;
    }
    if let Some(adjustment) = parse_editor_adjust_item(item) {
        return attach_operator_dependency_rejection(adjustment, item);
    }
    if !has_clip {
        if let Some(reference) =
            parse_editor_ref_item(item, options, has_listen)
        {
            return reference;
        }
        if has_listen {
            // `%l` on a non-reference shape falls through to the generic
            // task path; `apply_listen_directive` reports it.
        }
    }
    let mut spans: Vec<Span> = Vec::new();
    let mut diagnostics: Vec<Diagnostic> = Vec::new();
    let mut seen = SeenMarkers::default();
    seen.clip = has_clip;
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

    let caret = classify_caret_item(item, parent_line, has_clip);

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
                && marker.section.as_deref().is_some_and(|section| {
                    !section.is_empty() && !is_valid_drawer_section(section)
                })
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

        let drawer_hit = (authored.depth == AuthoredDepth::First)
            .then(|| {
                super::drawer_bullet::scan_leading(
                    authored.body,
                    authored.body_start,
                )
            })
            .flatten();
        if let Some(super::drawer_bullet::DrawerBulletHit::Complete {
            leftover,
            ..
        }) = drawer_hit.as_ref()
            && !leftover.is_empty()
        {
            diagnostics.push(Diagnostic {
                severity: Severity::Error,
                code: "drawer_bullet_text",
                message: super::drawer_bullet::drawer_bullet_text_error(),
                range: Some((raw.start, raw.end)),
            });
        }
        if let Some(super::drawer_bullet::DrawerBulletHit::Invalid {
            span,
            raw: token,
        }) = drawer_hit.as_ref()
        {
            let message = if token.starts_with('"')
                || token.starts_with('“')
                || !token.starts_with('#')
            {
                super::drawer_bullet::invalid_quoted_drawer_error(token)
            } else {
                INVALID_DRAWER_ERROR.to_string()
            };
            diagnostics.push(Diagnostic {
                severity: Severity::Error,
                code: "invalid_drawer",
                message,
                range: Some((span.start, span.end)),
            });
        }

        if child_parse.body.is_empty() && drawer_hit.is_none() {
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
        } else if matches!(
            drawer_hit,
            Some(super::drawer_bullet::DrawerBulletHit::Incomplete { .. })
                | Some(super::drawer_bullet::DrawerBulletHit::Invalid { .. })
        ) {
            if authored.depth == AuthoredDepth::First {
                has_first_level_owner = true;
            }
        } else {
            child_task_tokens.push(child_parse.body_tokens.last().copied());
            child_task_lines.push(line_number);
            let (child_body, drawer) = match drawer_hit {
                Some(super::drawer_bullet::DrawerBulletHit::Escaped {
                    ..
                }) => (
                    super::drawer_bullet::unescape_escaped_body(
                        &child_parse.body,
                    ),
                    None,
                ),
                Some(super::drawer_bullet::DrawerBulletHit::Complete {
                    selector,
                    title,
                    ..
                }) => (title, Some(selector)),
                _ => (child_parse.body, None),
            };
            if child_body.is_empty() && drawer.is_none() {
                if child_parse.dependencies.stripped_tokens == 0 {
                    diagnostics.push(Diagnostic {
                        severity: Severity::Error,
                        code: "empty_child_after_markers",
                        message: empty_child_after_markers_error(line_number),
                        range: Some((raw.start, raw.end)),
                    });
                }
            } else {
                sub_bullets.push(AuthoredSubBullet {
                    body: child_body,
                    depth: authored.depth,
                    task_id: None,
                    drawer,
                });
                if authored.depth == AuthoredDepth::First {
                    has_first_level_owner = true;
                }
            }
        }
    }

    // A marker-only `@route+block-id` with no body, authored children, or
    // other item-wide markers is Ensure Next. A glued `#drawer` suffix
    // always stays a drawer capture and never toggles.
    let has_drawer = section.is_some();
    let toggle_eligible = body.is_empty()
        && sub_bullets.is_empty()
        && !seen.schedule
        && !seen.priority
        && !seen.clip
        && !has_drawer;
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
    } else if mode == EditorMode::SubBullet
        && has_drawer
        && body.trim().is_empty()
        && sub_bullet_relaxed_section_range.is_none()
    {
        // A drawer capture with no parent note text is incomplete even
        // with authored children.
        if !needs.contains(&Need::DrawerText) {
            needs.push(Need::DrawerText);
        }
        if mode != EditorMode::Incomplete {
            // Keep SubBullet mode but mark incomplete via needs; the
            // editor shows the drawer menu and the missing-text hint.
            // Diagnostics stay empty so the menu can open.
        }
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
                    | SpanKind::Drawer
            )
        });
        diagnostics.push(Diagnostic {
            severity: Severity::Error,
            code: "invalid_drawer",
            message: INVALID_DRAWER_ERROR.to_string(),
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
        let conflict = if item_dependencies.has_entries() {
            // Relationship extras use `relationship_needs_text`.
            None
        } else if !sub_bullets.is_empty() {
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

    // A `=x` close suffix cannot combine with `s:<N>`, `$<N>`, or `%c`:
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
                spans.iter().find(|span| span.kind == SpanKind::Directive)
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
                            project_note: None,
                            title: None,
                            key: None,
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
                                project_note: None,
                                title: None,
                                key: None,
                            });
                        }
                        spans.push(Span {
                            start: token.start + 1,
                            end: token.end,
                            kind: SpanKind::ProjectTaskBlockId,
                            project_note: None,
                            title: None,
                            key: None,
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
                bare_name: _,
                start_ranges,
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
                    project_note: None,
                    title: None,
                    key: None,
                });
                spans.push(Span {
                    start: route_end + 1,
                    end: token_start + link_end,
                    kind: SpanKind::ActiveTaskBlockId,
                    project_note: None,
                    title: None,
                    key: None,
                });
                if let Some((name_start, name_end)) = name_range
                    && name_end > name_start
                {
                    spans.push(Span {
                        start: token_start + name_start,
                        end: token_start + name_end,
                        kind: SpanKind::PomodoroName,
                        project_note: None,
                        title: None,
                        key: None,
                    });
                }
                if let Some(offset) = start_offset {
                    if pomodoro_close.is_some() {
                        spans.push(Span {
                            start: token_start + offset,
                            end: token_start + offset + 2,
                            kind: SpanKind::PomodoroClose,
                            project_note: None,
                            title: None,
                            key: None,
                        });
                        if let Some((start, end)) = close_in_progress {
                            spans.push(Span {
                                start: token_start + start,
                                end: token_start + end,
                                kind: SpanKind::PomodoroCloseInProgress,
                                project_note: None,
                                title: None,
                                key: None,
                            });
                        }
                        if let Some((start, end)) = close_park {
                            spans.push(Span {
                                start: token_start + start,
                                end: token_start + end,
                                kind: SpanKind::PomodoroClosePark,
                                project_note: None,
                                title: None,
                                key: None,
                            });
                        }
                        if let Some((start, end)) = close_complete {
                            spans.push(Span {
                                start: token_start + start,
                                end: token_start + end,
                                kind: SpanKind::PomodoroCloseComplete,
                                project_note: None,
                                title: None,
                                key: None,
                            });
                        }
                        if let Some((start, end)) = close_drop {
                            spans.push(Span {
                                start: token_start + start,
                                end: token_start + end,
                                kind: SpanKind::PomodoroCloseDrop,
                                project_note: None,
                                title: None,
                                key: None,
                            });
                        }
                    } else {
                        for (start, end) in start_ranges {
                            if end > start {
                                spans.push(Span {
                                    start: token_start + start,
                                    end: token_start + end,
                                    kind: SpanKind::PomodoroStart,
                                    project_note: None,
                                    title: None,
                                    key: None,
                                });
                            }
                        }
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
                    project_note: None,
                    title: None,
                    key: None,
                });
                spans.push(Span {
                    start: route_end + 1,
                    end: token_start + link_end,
                    kind: SpanKind::ActiveTaskBlockId,
                    project_note: None,
                    title: None,
                    key: None,
                });
                spans.push(Span {
                    start: token_start + suffix_offset,
                    end: token_start + suffix_offset + 2,
                    kind: SpanKind::PomodoroClose,
                    project_note: None,
                    title: None,
                    key: None,
                });
                if let Some((start, end)) = close_in_progress {
                    spans.push(Span {
                        start: token_start + start,
                        end: token_start + end,
                        kind: SpanKind::PomodoroCloseInProgress,
                        project_note: None,
                        title: None,
                        key: None,
                    });
                }
                if let Some((start, end)) = close_park {
                    spans.push(Span {
                        start: token_start + start,
                        end: token_start + end,
                        kind: SpanKind::PomodoroClosePark,
                        project_note: None,
                        title: None,
                        key: None,
                    });
                }
                if let Some((start, end)) = close_complete {
                    spans.push(Span {
                        start: token_start + start,
                        end: token_start + end,
                        kind: SpanKind::PomodoroCloseComplete,
                        project_note: None,
                        title: None,
                        key: None,
                    });
                }
                if let Some((start, end)) = close_drop {
                    spans.push(Span {
                        start: token_start + start,
                        end: token_start + end,
                        kind: SpanKind::PomodoroCloseDrop,
                        project_note: None,
                        title: None,
                        key: None,
                    });
                }
                spans.push(Span {
                    start: token_start + separator_range.0,
                    end: token_start + separator_range.1,
                    kind: SpanKind::InteractivePlaceholder,
                    project_note: None,
                    title: None,
                    key: None,
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
                local_destination_markers.push(LocalDestinationMarker {
                    start: token_start,
                    end: token_end,
                    text: caret.token.text.to_string(),
                    mode: EditorMode::Incomplete,
                    route: route.clone(),
                    block_id: None,
                    section: None,
                });
                local_destination_marker = local_destination_markers
                    .last()
                    .map(|marker| marker.text.clone());
                spans.push(Span {
                    start: token_start,
                    end: token_end,
                    kind: SpanKind::InteractivePlaceholder,
                    project_note: None,
                    title: None,
                    key: None,
                });
            }
            CaretItemKind::NamePartial {
                route: partial_route,
                block_id: partial_block,
                link_end,
                name_range,
                start_offset: _,
                start_ranges,
                start: partial_start,
            } => {
                mode = EditorMode::Incomplete;
                body = String::new();
                route = Some(partial_route);
                section = None;
                block_id = Some(partial_block);
                needs = vec![Need::PomodoroName];
                pomodoro_start = partial_start;
                pomodoro_close = None;
                let route_end =
                    token_start + 1 + route.as_deref().unwrap_or("").len();
                spans.push(Span {
                    start: token_start,
                    end: route_end,
                    kind: SpanKind::ActiveTaskRoute,
                    project_note: None,
                    title: None,
                    key: None,
                });
                spans.push(Span {
                    start: route_end + 1,
                    end: token_start + link_end,
                    kind: SpanKind::ActiveTaskBlockId,
                    project_note: None,
                    title: None,
                    key: None,
                });
                for (start, end) in start_ranges {
                    if end > start {
                        spans.push(Span {
                            start: token_start + start,
                            end: token_start + end,
                            kind: SpanKind::PomodoroStart,
                            project_note: None,
                            title: None,
                            key: None,
                        });
                    }
                }
                spans.push(Span {
                    start: token_start + name_range.0 - 1,
                    end: token_start + name_range.0,
                    kind: SpanKind::InteractivePlaceholder,
                    project_note: None,
                    title: None,
                    key: None,
                });
            }
            CaretItemKind::TimingIncomplete {
                route: timing_route,
                block_id: timing_block,
                pomodoro_name: timing_name,
                link_end: timing_link_end,
                name_range: timing_name_range,
                start_offset: _,
                start_ranges: timing_ranges,
                separator_range,
            } => {
                mode = EditorMode::Incomplete;
                body = String::new();
                route = Some(timing_route);
                section = Some(timing_name);
                block_id = Some(timing_block);
                needs = vec![Need::PomodoroStartTiming];
                pomodoro_start = None;
                pomodoro_close = None;
                let route_end =
                    token_start + 1 + route.as_deref().unwrap_or("").len();
                spans.push(Span {
                    start: token_start,
                    end: route_end,
                    kind: SpanKind::ActiveTaskRoute,
                    project_note: None,
                    title: None,
                    key: None,
                });
                spans.push(Span {
                    start: route_end + 1,
                    end: token_start + timing_link_end,
                    kind: SpanKind::ActiveTaskBlockId,
                    project_note: None,
                    title: None,
                    key: None,
                });
                for (start, end) in timing_ranges {
                    if end > start {
                        spans.push(Span {
                            start: token_start + start,
                            end: token_start + end,
                            kind: SpanKind::PomodoroStart,
                            project_note: None,
                            title: None,
                            key: None,
                        });
                    }
                }
                spans.push(Span {
                    start: token_start + timing_name_range.0,
                    end: token_start + timing_name_range.1,
                    kind: SpanKind::PomodoroName,
                    project_note: None,
                    title: None,
                    key: None,
                });
                spans.push(Span {
                    start: token_start + separator_range.0,
                    end: token_start + separator_range.1,
                    kind: SpanKind::InteractivePlaceholder,
                    project_note: None,
                    title: None,
                    key: None,
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
        for invalid in item_dependencies
            .invalid
            .iter()
            .chain(item_dependencies.blocked_invalid.iter())
        {
            diagnostics.push(Diagnostic {
                severity: Severity::Error,
                code: "invalid_dependency",
                message: invalid.message.clone(),
                range: Some((invalid.start, invalid.end)),
            });
        }
        for invalid in item_dependencies.relation_invalid.iter() {
            diagnostics.push(Diagnostic {
                severity: Severity::Error,
                code: "invalid_relation",
                message: invalid.message.clone(),
                range: Some((invalid.start, invalid.end)),
            });
        }
        let parent_marker = parent_parse.marker.as_ref();
        let caret_aliased = has_caret_item
            && body.is_empty()
            && section.is_none()
            && pomodoro_start.is_none()
            && pomodoro_close.is_none()
            && matches!(
                mode,
                EditorMode::PomodoroLink | EditorMode::Incomplete
            );
        let aliased = (mode == EditorMode::PomodoroLink
            && !has_caret_item
            && parent_marker.is_some_and(|marker| {
                marker.mode == EditorMode::PomodoroTask
                    && marker.section.is_none()
                    && marker.pomodoro_start.is_none()
                    && marker.pomodoro_close.is_none()
            })
            && body.is_empty())
            || (caret_aliased && mode == EditorMode::PomodoroLink);
        if has_explicit_toggle {
            diagnostics.push(invalid_dependency_target_diagnostic(
                local_destination_marker.as_deref().unwrap_or("@route+id"),
                spans.iter().find_map(|span| {
                    (span.kind == SpanKind::TaskToggleExplicitToggle)
                        .then_some((span.start, span.end))
                }),
                item_dependencies.sigil(),
            ));
        } else if mode == EditorMode::TaskToggle
            && parent_marker.is_some_and(|marker| {
                matches!(marker.mode, EditorMode::SubBullet)
            })
        {
            let owner =
                local_destination_marker.as_deref().unwrap_or("@route+id");
            if owner_marker_is_suffixed(owner) {
                diagnostics.push(invalid_dependency_target_diagnostic(
                    owner,
                    local_destination_markers
                        .first()
                        .map(|marker| (marker.start, marker.end)),
                    item_dependencies.sigil(),
                ));
            } else if let Some(target) =
                existing_dependency_target(&route, &block_id)
            {
                mode = EditorMode::TaskDependency;
                if let Some(index) = own_local_destination_marker_index {
                    local_destination_markers[index].mode =
                        EditorMode::TaskDependency;
                }
                dependency_target = Some(target);
            }
        } else if aliased
            && let Some(target) = existing_dependency_target(&route, &block_id)
        {
            mode = EditorMode::TaskDependency;
            if let Some(index) = own_local_destination_marker_index {
                local_destination_markers[index].mode =
                    EditorMode::TaskDependency;
            } else if let Some(index) = local_destination_markers
                .iter()
                .position(|marker| marker.text.starts_with('^'))
            {
                local_destination_markers[index].mode =
                    EditorMode::TaskDependency;
            }
            dependency_target = Some(target);
        } else if mode == EditorMode::PomodoroLink {
            if !diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code == "invalid_pomodoro_link")
            {
                diagnostics.push(invalid_dependency_target_diagnostic(
                    local_destination_marker.as_deref().unwrap_or("@route:id"),
                    local_destination_markers
                        .first()
                        .map(|marker| (marker.start, marker.end)),
                    item_dependencies.sigil(),
                ));
            }
        } else if mode == EditorMode::SubBullet
            && let Some(target) = existing_dependency_target(&route, &block_id)
        {
            dependency_target = Some(target);
        } else if mode == EditorMode::Incomplete
            && body.is_empty()
            && block_id.is_none()
            && let Some(pending) = pending_colon_dependency_target(
                &route,
                local_destination_marker
                    .as_deref()
                    .or(parent_parse.marker_text.as_deref()),
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
            diagnostics.push(dependency_unsupported_target_diagnostic(
                mode,
                None,
                item_dependencies.sigil(),
            ));
        }
        if matches!(
            dependency_target.as_ref().map(|target| &target.kind),
            Some(DependencyTargetKind::ExistingTask)
        ) && body.is_empty()
            && (seen.schedule
                || seen.priority
                || seen.clip
                || !sub_bullets.is_empty())
        {
            let owner =
                local_destination_marker.as_deref().unwrap_or("@route+id");
            let (offending, range) = if let Some(span) = spans
                .iter()
                .filter(|span| {
                    matches!(
                        span.kind,
                        SpanKind::Schedule
                            | SpanKind::Priority
                            | SpanKind::Directive
                    )
                })
                .min_by_key(|span| span.start)
            {
                (
                    item.source.get(span.start..span.end).unwrap_or("$2"),
                    Some((span.start, span.end)),
                )
            } else if seen.clip {
                // `%c` is masked before this pass, so the directive span is
                // attached later. Name the extra from the seeded clip flag.
                ("%c", None)
            } else {
                let child_range = item.lines.iter().skip(1).find_map(|line| {
                    matches!(
                        classify_authored_line(line.raw),
                        AuthoredLineClass::Item(_)
                    )
                    .then_some((line.raw.start, line.raw.end))
                });
                ("child bullets", child_range)
            };
            diagnostics.push(Diagnostic {
                severity: Severity::Error,
                code: "relationship_needs_text",
                message: relationship_needs_text_error(
                    offending,
                    owner,
                    &relation_modifier_raws(&item_dependencies),
                ),
                range,
            });
        }
        if !item_dependencies.partials.is_empty()
            && !needs.contains(&Need::TaskDependency)
        {
            needs.push(Need::TaskDependency);
        }
        if !item_dependencies.blocked_partials.is_empty()
            && !needs.contains(&Need::BlockedTask)
        {
            needs.push(Need::BlockedTask);
        }
        if !item_dependencies.relation_target_partials.is_empty()
            && !needs.contains(&Need::RelationTarget)
        {
            needs.push(Need::RelationTarget);
        }
        if !item_dependencies.relation_label_partials.is_empty()
            && !needs.contains(&Need::RelationLabel)
        {
            needs.push(Need::RelationLabel);
        }
        if pomodoro_close.is_some()
            && !item_dependencies.blocked_entries.is_empty()
        {
            diagnostics.push(Diagnostic {
                severity: Severity::Error,
                code: "invalid_dependency_target",
                message: close_blocked_task_error(
                    local_destination_marker
                        .as_deref()
                        .unwrap_or("@route:id=x"),
                ),
                range: item_dependencies
                    .blocked_entries
                    .first()
                    .map(|entry| (entry.start, entry.end)),
            });
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

    attach_drawer_bullet_metadata(
        item,
        &mut mode,
        &body,
        section.as_deref(),
        &mut needs,
        &mut spans,
        &mut diagnostics,
        &mut sub_bullets,
    );

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
            ref_listen: false,
            pomodoro_start,
            pomodoro_adjust: None,
            pomodoro_shift: None,
            pomodoro_close,
            dependencies: item_dependencies.dependency_entries(),
            blocked_tasks: item_dependencies.blocked_task_entries(),
            relations: item_dependencies.relation_entries(),
            dependency_target,
            task_complete: None,
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
