//! Core editor line, global, and item parsing.

use super::dependencies::*;
use super::draft::*;
use super::editor_classify::*;
use super::editor_model::*;
use super::editor_pomodoro::*;
use super::line::*;
use super::markers::*;
use super::model::*;
use super::move_bullet::MoveBulletHit;
use super::move_bullet::MoveBulletInvalid;
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
    /// A trailing-run task ID stripped before marker extraction, so the
    /// spans above never cover it. `None` on lines that take no ID.
    pub(super) stripped_task_id: Option<StrippedTaskId<'a>>,
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
    strip_task_id: bool,
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
    // Task-bullet IDs (` :id` / ` ^id`) may sit anywhere in the trailing
    // marker run, so the ID leaves before marker extraction and
    // schedule/priority markers behind it still resolve.
    let stripped_task_id = if strip_task_id {
        strip_trailing_task_id(&mut tokens)
    } else {
        None
    };
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
        Some((_, TokenParse::Marker(mut marker))) => {
            if let Some(diagnostic) = marker.diagnostic.take() {
                diagnostics.push(diagnostic);
            }
            Some(marker)
        }
        Some((_, TokenParse::Invalid(diagnostic))) => {
            diagnostics.push(diagnostic);
            None
        }
        None => None,
    };

    LineEditorParse {
        body,
        body_tokens,
        stripped_task_id,
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
        rewrite: None,
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
    // Template rows are never split by headers, and a header in the
    // template body is refused outright.
    let template_draft =
        super::template::analyze_for_capture(raw_text, options.record_policy)
            .directive
            .is_some();
    let draft = if template_draft {
        split_capture_draft_no_lists(raw_text)
    } else {
        split_capture_draft(raw_text)
    };
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
    if template_draft {
        for (raw_item, outcome) in
            draft.items.iter().zip(item_outcomes.iter_mut())
        {
            let Some(first) = raw_item.lines.first() else {
                continue;
            };
            let Some(detected) = detect_list_header(&first.raw) else {
                continue;
            };
            let list = ListContext {
                kind: detected.kind,
                route: detected.route,
                section: detected.section,
                header_line: first.line_number,
                position: 0,
                count: 0,
                exact: detected.exact,
                header_start: first.raw.start,
                header_end: first.raw.end,
                ownerless: false,
            };
            outcome.item = list_refusal_item(
                raw_item,
                &list,
                Some(("template_list", super::draft::template_list_error())),
                Some((first.raw.start, first.raw.end)),
            )
            .item;
        }
    }
    // A second `- ^^` anywhere in the draft names the first line, across
    // items. Per-item duplicates already diagnosed above.
    {
        let mut first_choice_line: Option<usize> = None;
        for outcome in &mut item_outcomes {
            // Collect second-choice ranges first to avoid borrowing the
            // outcome while pushing its diagnostic.
            let mut second_ranges: Vec<(usize, usize)> = Vec::new();
            for sub_bullet in &outcome.item.sub_bullets {
                let Some(choice) = sub_bullet.move_choice.as_ref() else {
                    continue;
                };
                let line = raw_text[..choice.range.0.min(raw_text.len())]
                    .matches('\n')
                    .count()
                    + 1;
                if first_choice_line.is_some() {
                    second_ranges.push(choice.range);
                } else {
                    first_choice_line = Some(line);
                }
            }
            for range in second_ranges {
                let first = first_choice_line.unwrap_or(1);
                outcome.item.diagnostics.push(Diagnostic {
                    severity: Severity::Error,
                    code: "move_choice_duplicate",
                    message: super::move_bullet::move_choice_duplicate_error(
                        first,
                    ),
                    range: Some(range),
                    rewrite: None,
                });
            }
        }
    }
    attach_list_duplicate_id_diagnostics(&mut item_outcomes);
    let record_intents = draft
        .items
        .iter()
        .zip(item_outcomes.iter())
        .filter_map(|(raw_item, outcome)| {
            (outcome.item.mode == EditorMode::RecordNote)
                .then(|| {
                    super::record::classify_record_item(
                        raw_item,
                        options.record_policy,
                    )
                })
                .flatten()
                .and_then(|claim| match claim {
                    super::record::RecordClaim::Note(intent) => {
                        Some((outcome.item.index, intent))
                    }
                    super::record::RecordClaim::Refused { .. } => None,
                })
        })
        .collect::<Vec<_>>();
    let transfers = draft
        .items
        .iter()
        .zip(item_outcomes.iter())
        .filter_map(|(raw_item, outcome)| {
            matches!(
                outcome.item.mode,
                EditorMode::TaskMove | EditorMode::TaskMerge
            )
            .then(|| editor_transfer_item(raw_item))
            .flatten()
        })
        .collect::<Vec<_>>();
    let merge_ins = item_outcomes
        .iter()
        .filter_map(|outcome| {
            outcome.item.merge_in.clone().map(|merge_in| {
                super::editor_model::EditorMergeInItem {
                    index: outcome.item.index,
                    merge_in,
                }
            })
        })
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
            rewrite: None,
        });
    }

    let mut items = item_outcomes
        .into_iter()
        .map(|outcome| outcome.item)
        .collect::<Vec<_>>();
    // Past stacks: maximal runs of consecutive valid Past items. Invalid
    // or unfinished items break runs and never fabricate durations. Pin
    // diagnostics become `invalid_pomodoro_past` diagnostics on the
    // offending item; every other item's parse is unchanged.
    {
        let members: Vec<super::past::PastStackMember<'_>> = items
            .iter()
            .map(|it| {
                if it.mode == EditorMode::PomodoroPast
                    && let Some(spec) = it.pomodoro_past.as_ref()
                {
                    return super::past::PastStackMember {
                        blocks: Some(spec.blocks),
                        edge: spec.segment.as_ref().map(|s| s.edge),
                        segment_raw: spec
                            .segment
                            .as_ref()
                            .map(|s| s.raw.as_str()),
                        raw_token: spec.raw.as_str(),
                    };
                }
                super::past::PastStackMember {
                    blocks: None,
                    edge: None,
                    segment_raw: None,
                    raw_token: "",
                }
            })
            .collect();
        let (stacks, diagnostics) = super::past::compute_past_stacks(&members);
        for (it, st) in items.iter_mut().zip(stacks) {
            if let Some(spec) = it.pomodoro_past.as_mut() {
                spec.stack = st;
            }
        }
        for (idx, message) in diagnostics {
            if let Some(item) = items.get_mut(idx) {
                let range = Some((item.start, item.end));
                item.diagnostics.push(Diagnostic {
                    severity: Severity::Error,
                    code: "invalid_pomodoro_past",
                    message,
                    range,
                    rewrite: None,
                });
                item.pomodoro_past = None;
            }
        }
    }
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
                || item.mode == EditorMode::TaskMove
                || item.mode == EditorMode::TaskMerge
                || item.mode == EditorMode::PomodoroShift
                || item.mode == EditorMode::PomodoroSplit
                || item.mode == EditorMode::PomodoroMerge
                || item.mode == EditorMode::PomodoroClose
                || item.mode == EditorMode::PomodoroPast
                || item.mode == EditorMode::PomodoroStart
                || item.mode == EditorMode::Ref
                || item.mode == EditorMode::TaskComplete
                || item.needs == [Need::TaskLink]
                || item.needs == [Need::TaskGoto]
                || item.needs == [Need::TaskComplete]
                || item.pomodoro_close.is_some()
                || item.pomodoro_past.is_some()
                || item.pomodoro_start.is_some()
                || item.spans.iter().any(|span| {
                    matches!(
                        span.kind,
                        SpanKind::TransferSource
                            | SpanKind::TransferArrow
                            | SpanKind::TransferDestination
                    )
                })
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
            pomodoro_split: None,
            pomodoro_merge: None,
            pomodoro_close: None,
            pomodoro_past: None,
            dependencies: Vec::new(),
            blocked_tasks: Vec::new(),
            relations: Vec::new(),
            dependency_target: None,
            task_complete: None,
            spans,
            diagnostics: global_diagnostics,
            sub_bullets: Vec::new(),
            items,
            record_intents,
            global_destination,
            targets: Vec::new(),
            transfers,
            merge_ins,
            merge_in: None,
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
    let pomodoro_split = first.pomodoro_split.clone();
    let pomodoro_merge = first.pomodoro_merge.clone();
    let pomodoro_close = first.pomodoro_close.clone();
    let pomodoro_past = first.pomodoro_past.clone();
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

    let targets = items
        .first()
        .map(|item| item.targets.clone())
        .unwrap_or_default();
    let merge_in = items.first().and_then(|item| item.merge_in.clone());
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
        pomodoro_split,
        pomodoro_merge,
        pomodoro_close,
        pomodoro_past,
        dependencies,
        blocked_tasks,
        relations,
        dependency_target,
        task_complete,
        spans,
        diagnostics,
        sub_bullets,
        items,
        record_intents,
        global_destination,
        targets,
        transfers,
        merge_ins,
        merge_in,
    }
}

fn editor_transfer_item(item: &CaptureItem<'_>) -> Option<EditorTransferItem> {
    let parent = item.lines.first()?.raw;
    let super::transfer::TransferScan::Valid {
        sources,
        arrow,
        destination,
    } = super::transfer::scan_transfer_line(
        &parent,
        item.source,
        item.lines.len() > 1,
    )
    else {
        return None;
    };
    Some(EditorTransferItem {
        index: item.index,
        transfer: EditorTransfer {
            sources: sources
                .into_iter()
                .map(|source| EditorTransferSource {
                    start: source.start,
                    end: source.end,
                    marker: source.marker,
                    route: source.route,
                    block_id: source.block_id,
                })
                .collect(),
            arrow: EditorTransferArrow {
                start: arrow.start,
                end: arrow.end,
            },
            destination: EditorTransferDestination {
                start: destination.start,
                end: destination.end,
                kind: editor_transfer_destination_kind(destination.kind),
                route: destination.route,
                block_id: destination.block_id,
                drawer: destination.drawer,
                section: destination.section,
            },
        },
    })
}

fn editor_transfer_destination_kind(
    kind: super::transfer::TransferDestinationKind,
) -> EditorTransferDestinationKind {
    match kind {
        super::transfer::TransferDestinationKind::Note => {
            EditorTransferDestinationKind::Note
        }
        super::transfer::TransferDestinationKind::Task => {
            EditorTransferDestinationKind::Task
        }
        super::transfer::TransferDestinationKind::Drawer => {
            EditorTransferDestinationKind::Drawer
        }
        super::transfer::TransferDestinationKind::Section => {
            EditorTransferDestinationKind::Section
        }
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
                rewrite: None,
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
    // Only a new task under a plain `@@route` keeps its drawer position,
    // now filed at the global route.
    item.drawer_bullet_target =
        match (item.drawer_bullet_target.take(), global.mode) {
            (
                Some(super::drawer_bullet::DrawerBulletTarget::NewTask {
                    text,
                    ..
                }),
                EditorMode::Task,
            ) => Some(super::drawer_bullet::DrawerBulletTarget::NewTask {
                route: global.route.clone(),
                text,
            }),
            _ => None,
        };
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
        EditorMode::PomodoroAdjust
        | EditorMode::PomodoroShift
        | EditorMode::PomodoroSplit
        | EditorMode::PomodoroMerge => "Pomodoro session-operator",
        EditorMode::PomodoroClose => "Pomodoro-close",
        EditorMode::PomodoroPast => "Pomodoro-past",
        EditorMode::PomodoroStart => "Pomodoro-start",
        EditorMode::TaskComplete => "task-complete",
        EditorMode::RecordNote => "record-note",
        EditorMode::Ref => "reference",
        EditorMode::PomodoroLink | EditorMode::PomodoroTask => {
            "Pomodoro-ledger-link"
        }
        EditorMode::TaskToggle
        | EditorMode::SubBullet
        | EditorMode::TaskDependency => "task-toggle",
        EditorMode::TaskMove | EditorMode::TaskMerge => "task transfer",
        EditorMode::Task | EditorMode::Incomplete | EditorMode::NotePull => {
            "task"
        }
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
        rewrite: None,
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
        rewrite: None,
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
            rewrite: None,
        });
    }
    for invalid in found.relation_invalid.iter() {
        outcome.item.diagnostics.push(Diagnostic {
            severity: Severity::Error,
            code: "invalid_relation",
            message: invalid.message.clone(),
            range: Some((invalid.start, invalid.end)),
            rewrite: None,
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
            pomodoro_split: None,
            pomodoro_merge: None,
            pomodoro_close: None,
            pomodoro_past: None,
            dependencies: Vec::new(),
            blocked_tasks: Vec::new(),
            relations: Vec::new(),
            dependency_target: None,
            task_complete: None,
            list: None,
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
            drawer_bullet_target: None,
            targets: Vec::new(),
            merge_in: None,
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
            pomodoro_split: None,
            pomodoro_merge: None,
            pomodoro_close: None,
            pomodoro_past: None,
            dependencies: Vec::new(),
            blocked_tasks: Vec::new(),
            relations: Vec::new(),
            dependency_target: None,
            task_complete: None,
            list: None,
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
            drawer_bullet_target: None,
            targets: Vec::new(),
            merge_in: None,
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
            pomodoro_split: None,
            pomodoro_merge: None,
            pomodoro_close: None,
            pomodoro_past: None,
            dependencies: Vec::new(),
            blocked_tasks: Vec::new(),
            relations: Vec::new(),
            dependency_target: None,
            task_complete,
            list: None,
            spans,
            diagnostics,
            sub_bullets: Vec::new(),
            has_local_destination: true,
            local_destination_markers: Vec::new(),
            drawer_bullet_target: None,
            targets: Vec::new(),
            merge_in: None,
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
                rewrite: None,
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
            rewrite: None,
        });
    }
    for invalid in found.relation_invalid.iter() {
        diagnostics.push(Diagnostic {
            severity: Severity::Error,
            code: "invalid_relation",
            message: invalid.message.clone(),
            range: Some((invalid.start, invalid.end)),
            rewrite: None,
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
            pomodoro_split: None,
            pomodoro_merge: None,
            pomodoro_close: None,
            pomodoro_past: None,
            dependencies: found.dependency_entries(),
            blocked_tasks: found.blocked_task_entries(),
            relations: found.relation_entries(),
            dependency_target: None,
            task_complete: None,
            list: None,
            spans,
            diagnostics,
            sub_bullets: Vec::new(),
            has_local_destination: true,
            local_destination_markers: Vec::new(),
            drawer_bullet_target: None,
            targets: Vec::new(),
            merge_in: None,
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
                pomodoro_split: None,
                pomodoro_merge: None,
                pomodoro_close: None,
                pomodoro_past: None,
                dependencies: Vec::new(),
                blocked_tasks: Vec::new(),
                relations: Vec::new(),
                dependency_target: None,
                task_complete: None,
                list: None,
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
                drawer_bullet_target: None,
                targets: Vec::new(),
                merge_in: None,
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
                pomodoro_split: None,
                pomodoro_merge: None,
                pomodoro_close: None,
                pomodoro_past: None,
                dependencies: Vec::new(),
                blocked_tasks: Vec::new(),
                relations: Vec::new(),
                dependency_target: None,
                task_complete: None,
                list: None,
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
                drawer_bullet_target: None,
                targets: Vec::new(),
                merge_in: None,
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

/// Build the editor outcome for a claimed record item: mode
/// `record_note` with the claim's needs, spans, and diagnostics.
/// Notes stay out of `sub_bullets` so task-only editor tooling
/// ignores them, exactly like the strict path.
fn record_editor_outcome<'a>(
    item: &CaptureItem<'a>,
    view: super::record::RecordEditorView,
) -> EditorItemOutcome<'a> {
    EditorItemOutcome {
        item: EditorItemParse {
            index: item.index,
            start: item.start,
            end: item.end,
            line_start: item.line_start,
            line_end: item.line_end,
            body: view.body,
            mode: EditorMode::RecordNote,
            route: view.route,
            section: None,
            block_id: None,
            needs: view.needs,
            ref_listen: false,
            pomodoro_start: None,
            pomodoro_adjust: None,
            pomodoro_shift: None,
            pomodoro_split: None,
            pomodoro_merge: None,
            pomodoro_close: None,
            pomodoro_past: None,
            dependencies: Vec::new(),
            blocked_tasks: Vec::new(),
            relations: Vec::new(),
            dependency_target: None,
            task_complete: None,
            list: item.list.clone(),
            spans: view.spans,
            diagnostics: view.diagnostics,
            sub_bullets: Vec::new(),
            has_local_destination: true,
            local_destination_markers: Vec::new(),
            drawer_bullet_target: None,
            targets: Vec::new(),
            merge_in: None,
        },
        declarations: Vec::new(),
    }
}

pub(super) fn parse_editor_item_with<'a>(
    item: &CaptureItem<'a>,
    options: &EditorParseOptions<'_>,
) -> EditorItemOutcome<'a> {
    // Record creation claims before transfers: a record field value may
    // hold `>`/`<` prose that must stay opaque.
    if let Some(view) = super::record::claim_editor_record(item, options) {
        return record_editor_outcome(item, view);
    }
    let transfer = item.lines.first().map(|parent| {
        super::transfer::scan_transfer_line(
            &parent.raw,
            item.source,
            item.lines.len() > 1,
        )
    });
    if let Some(scan) = transfer {
        match scan {
            super::transfer::TransferScan::Unclaimed => {}
            super::transfer::TransferScan::Escaped {
                backslash_span, ..
            } => {
                let mut escaped = item.source.to_string();
                escaped.replace_range(backslash_span.0..backslash_span.1, " ");
                let lines = item
                    .lines
                    .iter()
                    .map(|line| ItemLine {
                        raw: RawLine {
                            text: escaped
                                .get(line.raw.start..line.raw.end)
                                .unwrap_or(""),
                            start: line.raw.start,
                            end: line.raw.end,
                        },
                        line_number: line.line_number,
                    })
                    .collect();
                let view = CaptureItem {
                    source: escaped.as_str(),
                    index: item.index,
                    start: item.start,
                    end: item.end,
                    line_start: item.line_start,
                    line_end: item.line_end,
                    lines,
                    list: item.list.clone(),
                };
                let outcome =
                    parse_editor_item_without_transfer(&view, options);
                return EditorItemOutcome {
                    item: outcome.item,
                    declarations: remap_type_marker_declarations(
                        outcome.declarations,
                        item.source,
                    ),
                };
            }
            claimed => return parse_editor_transfer_item(item, claimed),
        }
    }
    if let Some(outcome) = parse_editor_merge_claim(item, options) {
        return outcome;
    }
    parse_editor_item_without_transfer(item, options)
}

fn masked_editor_source(source: &str, from: usize, to: usize) -> String {
    let mut bytes = source.as_bytes().to_vec();
    let end = to.min(bytes.len());
    let start = from.min(end);
    for byte in bytes.iter_mut().skip(start).take(end - start) {
        *byte = b' ';
    }
    String::from_utf8(bytes).unwrap_or_else(|_| source.to_string())
}

fn editor_item_view<'a>(
    item: &CaptureItem<'a>,
    source: &'a str,
) -> CaptureItem<'a> {
    let lines = item
        .lines
        .iter()
        .map(|line| ItemLine {
            raw: RawLine {
                text: source.get(line.raw.start..line.raw.end).unwrap_or(""),
                start: line.raw.start,
                end: line.raw.end,
            },
            line_number: line.line_number,
        })
        .collect();
    CaptureItem {
        source,
        index: item.index,
        start: item.start,
        end: item.end,
        line_start: item.line_start,
        line_end: item.line_end,
        lines,
        list: item.list.clone(),
    }
}

fn parse_editor_merge_claim<'a>(
    item: &CaptureItem<'a>,
    options: &EditorParseOptions<'_>,
) -> Option<EditorItemOutcome<'a>> {
    let parent = item.lines.first()?.raw;
    let scan = super::merge_in::scan_merge_in_line(
        &parent,
        item.source,
        item.lines.len() > 1,
    );
    match scan {
        super::merge_in::MergeInScan::Unclaimed => None,
        super::merge_in::MergeInScan::Escaped {
            backslash_span,
            arrow_span,
        } => {
            let mut masked_bytes = item.source.as_bytes().to_vec();
            let from = backslash_span.0.min(masked_bytes.len());
            for byte in masked_bytes.iter_mut().skip(from) {
                *byte = b' ';
            }
            let masked_text = String::from_utf8(masked_bytes)
                .unwrap_or_else(|_| item.source.to_string());
            let view = editor_item_view(item, &masked_text);
            let outcome = parse_editor_item_without_transfer(&view, options);
            // Restore literal tail (`< ...` without backslash) into body.
            let tail_literal = item
                .source
                .get(arrow_span.0..parent.end)
                .unwrap_or("")
                .trim_start()
                .to_string();
            let mut item_parse = outcome.item;
            if !tail_literal.is_empty() {
                if item_parse.body.trim().is_empty() {
                    item_parse.body = tail_literal;
                } else {
                    item_parse.body = format!(
                        "{} {}",
                        item_parse.body.trim_end(),
                        tail_literal
                    );
                }
            }
            item_parse.merge_in = None;
            Some(EditorItemOutcome {
                item: item_parse,
                declarations: remap_type_marker_declarations(
                    outcome.declarations,
                    item.source,
                ),
            })
        }
        super::merge_in::MergeInScan::Incomplete {
            sources,
            arrow,
            span,
            partial,
            ..
        } => {
            let (mut item_parse, remapped_declarations) = {
                let masked =
                    masked_editor_source(item.source, arrow.start, parent.end);
                let view = editor_item_view(item, &masked);
                let outcome =
                    parse_editor_item_without_transfer(&view, options);
                let remapped = remap_type_marker_declarations(
                    outcome.declarations,
                    item.source,
                );
                (outcome.item, remapped)
            };
            // Keep complete sources, add pending need + placeholder.
            let mut spans = item_parse.spans.clone();
            spans.push(Span {
                start: arrow.start,
                end: arrow.end,
                kind: SpanKind::MergeInArrow,
                project_note: None,
                title: None,
                key: None,
            });
            for source in &sources {
                spans.push(Span {
                    start: source.start,
                    end: source.end,
                    kind: SpanKind::MergeInSource,
                    project_note: None,
                    title: None,
                    key: None,
                });
            }
            if let Some((partial_start, partial_end)) = partial {
                spans.push(Span {
                    start: partial_start,
                    end: partial_end,
                    kind: SpanKind::InteractivePlaceholder,
                    project_note: None,
                    title: None,
                    key: None,
                });
            } else {
                spans.push(Span {
                    start: arrow.end,
                    end: arrow.end,
                    kind: SpanKind::InteractivePlaceholder,
                    project_note: None,
                    title: None,
                    key: None,
                });
            }
            spans.sort_by_key(|span| (span.start, span.end));
            item_parse.spans = spans;
            if !item_parse.needs.contains(&Need::MergeInSource) {
                item_parse.needs.push(Need::MergeInSource);
            }
            let _ = span;
            let merge_in = EditorMergeIn {
                arrow: EditorMergeInArrow {
                    start: arrow.start,
                    end: arrow.end,
                },
                sources: sources
                    .into_iter()
                    .map(|source| EditorMergeInSource {
                        start: source.start,
                        end: source.end,
                        marker: source.marker,
                        route: source.route,
                        block_id: source.block_id,
                    })
                    .collect(),
            };
            item_parse.merge_in = Some(merge_in);
            Some(EditorItemOutcome {
                item: item_parse,
                declarations: remapped_declarations,
            })
        }
        super::merge_in::MergeInScan::Invalid {
            code,
            message,
            span,
            rewrite,
        } => {
            let diagnostic = Diagnostic {
                severity: Severity::Error,
                code,
                message,
                range: Some(span),
                rewrite: rewrite.map(|rewrite| DiagnosticRewrite {
                    start: rewrite.start,
                    end: rewrite.end,
                    replacement: rewrite.replacement,
                    label: rewrite.label,
                }),
            };
            let mut item_parse = empty_editor_item(
                item,
                EditorMode::Incomplete,
                None,
                None,
                None,
                Vec::new(),
                vec![Span {
                    start: span.0,
                    end: span.1,
                    kind: SpanKind::MergeInArrow,
                    project_note: None,
                    title: None,
                    key: None,
                }],
                vec![diagnostic],
            );
            item_parse.merge_in = None;
            Some(EditorItemOutcome {
                item: item_parse,
                declarations: Vec::new(),
            })
        }
        super::merge_in::MergeInScan::Valid { sources, arrow } => Some(
            parse_editor_merge_valid(item, options, parent, sources, arrow),
        ),
    }
}

fn parse_editor_merge_valid<'a>(
    item: &CaptureItem<'a>,
    options: &EditorParseOptions<'_>,
    parent: RawLine<'_>,
    sources: Vec<super::merge_in::MergeInSource>,
    arrow: super::merge_in::MergeInArrow,
) -> EditorItemOutcome<'a> {
    // An empty left side names no new task.
    let left_prefix_len = arrow.start.saturating_sub(parent.start);
    if parent
        .text
        .get(..left_prefix_len)
        .unwrap_or("")
        .trim()
        .is_empty()
    {
        let (message, rewrite) = editor_needs_new_task_message(
            &parent,
            &sources,
            EditorMergeNeeds::Empty,
        );
        let diagnostic = Diagnostic {
            severity: Severity::Error,
            code: super::merge_in::MERGE_IN_NEEDS_NEW_TASK_CODE,
            message,
            range: Some((arrow.start, arrow.end)),
            rewrite,
        };
        let mut item_parse = empty_editor_item(
            item,
            EditorMode::Incomplete,
            None,
            None,
            None,
            Vec::new(),
            Vec::new(),
            vec![diagnostic],
        );
        item_parse.merge_in = None;
        return EditorItemOutcome {
            item: item_parse,
            declarations: Vec::new(),
        };
    }
    // Scope the masked view so the local borrow ends before we return.
    // Remap declarations to the original source inside the block.
    let (mut left_parse, left_declarations) = {
        let masked = masked_editor_source(item.source, arrow.start, parent.end);
        let view = editor_item_view(item, &masked);
        let outcome = parse_editor_item_without_transfer(&view, options);
        let remapped =
            remap_type_marker_declarations(outcome.declarations, item.source);
        (outcome.item, remapped)
    };
    // Lexical `&`/`*`/`~`/`~~` source matches refuse first.
    if let Some(message) =
        editor_merge_source_also_marked_message(&left_parse, &sources)
    {
        let diagnostic = Diagnostic {
            severity: Severity::Error,
            code: super::merge_in::MERGE_IN_SOURCE_ALSO_MARKED_CODE,
            message,
            range: Some((
                sources
                    .first()
                    .map(|source| source.start)
                    .unwrap_or(arrow.start),
                sources.last().map(|source| source.end).unwrap_or(arrow.end),
            )),
            rewrite: None,
        };
        let mut item_parse = empty_editor_item(
            item,
            EditorMode::Incomplete,
            None,
            None,
            None,
            Vec::new(),
            Vec::new(),
            vec![diagnostic],
        );
        item_parse.merge_in = None;
        return EditorItemOutcome {
            item: item_parse,
            declarations: Vec::new(),
        };
    }
    match classify_editor_merge_left(&left_parse, &parent) {
        EditorMergeNeeds::Allowed => {
            left_parse.spans.push(Span {
                start: arrow.start,
                end: arrow.end,
                kind: SpanKind::MergeInArrow,
                project_note: None,
                title: None,
                key: None,
            });
            for source in &sources {
                left_parse.spans.push(Span {
                    start: source.start,
                    end: source.end,
                    kind: SpanKind::MergeInSource,
                    project_note: None,
                    title: None,
                    key: None,
                });
            }
            left_parse.spans.sort_by_key(|span| (span.start, span.end));
            let merge_in = EditorMergeIn {
                arrow: EditorMergeInArrow {
                    start: arrow.start,
                    end: arrow.end,
                },
                sources: sources
                    .into_iter()
                    .map(|source| EditorMergeInSource {
                        start: source.start,
                        end: source.end,
                        marker: source.marker,
                        route: source.route,
                        block_id: source.block_id,
                    })
                    .collect(),
            };
            left_parse.merge_in = Some(merge_in);
            EditorItemOutcome {
                item: left_parse,
                declarations: left_declarations,
            }
        }
        needs => {
            let (message, rewrite) =
                editor_needs_new_task_message(&parent, &sources, needs);
            let diagnostic = Diagnostic {
                severity: Severity::Error,
                code: super::merge_in::MERGE_IN_NEEDS_NEW_TASK_CODE,
                message,
                range: Some((arrow.start, arrow.end)),
                rewrite,
            };
            let mut item_parse = empty_editor_item(
                item,
                EditorMode::Incomplete,
                None,
                None,
                None,
                Vec::new(),
                Vec::new(),
                vec![diagnostic],
            );
            item_parse.merge_in = None;
            EditorItemOutcome {
                item: item_parse,
                declarations: Vec::new(),
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EditorMergeNeeds {
    Allowed,
    Empty,
    StartClose,
    Project,
    Existing,
    NoTask,
    TargetRun,
}

fn classify_editor_merge_left(
    parsed: &EditorItemParse,
    parent: &RawLine<'_>,
) -> EditorMergeNeeds {
    let left_trimmed = {
        let mut left_end = parent.text.len();
        for (index, _) in parent.text.char_indices() {
            if parent.text[index..].starts_with('<') {
                let before_ok = index == 0
                    || parent.text[..index]
                        .chars()
                        .next_back()
                        .is_some_and(|character| character.is_whitespace());
                if before_ok {
                    left_end = index;
                    break;
                }
            }
        }
        parent.text[..left_end].trim()
    };
    let is_empty = parsed.body.trim().is_empty()
        && parsed.route.is_none()
        && parsed.block_id.is_none()
        && parsed.section.is_none()
        && parsed.sub_bullets.is_empty()
        && parsed.dependencies.is_empty()
        && parsed.blocked_tasks.is_empty()
        && parsed.relations.is_empty();
    if is_empty && left_trimmed.is_empty() {
        return EditorMergeNeeds::Empty;
    }
    match parsed.mode {
        EditorMode::Task | EditorMode::SubBullet => EditorMergeNeeds::Allowed,
        EditorMode::PomodoroTask => {
            if parsed.pomodoro_start.is_none()
                && parsed.pomodoro_close.is_none()
            {
                EditorMergeNeeds::Allowed
            } else {
                EditorMergeNeeds::StartClose
            }
        }
        EditorMode::ProjectNote | EditorMode::PomodoroProjectNote => {
            EditorMergeNeeds::Project
        }
        EditorMode::TaskDependency
        | EditorMode::TaskToggle
        | EditorMode::TaskComplete => EditorMergeNeeds::Existing,
        EditorMode::TaskMove | EditorMode::TaskMerge => {
            EditorMergeNeeds::Existing
        }
        EditorMode::Incomplete => {
            // A target-run fan-out on the left is an existing-task shape;
            // other incompletes are empty/no-task based on body.
            if !parsed.targets.is_empty() {
                EditorMergeNeeds::TargetRun
            } else if parsed.body.trim().is_empty() {
                EditorMergeNeeds::Empty
            } else {
                EditorMergeNeeds::NoTask
            }
        }
        _ => EditorMergeNeeds::NoTask,
    }
}

fn editor_needs_new_task_message(
    parent: &RawLine<'_>,
    sources: &[super::merge_in::MergeInSource],
    needs: EditorMergeNeeds,
) -> (String, Option<DiagnosticRewrite>) {
    const PREFIX: &str = "< merges into the task this line creates";
    match needs {
        EditorMergeNeeds::Empty => (
            format!("{PREFIX}; name the new task before <"),
            None,
        ),
        EditorMergeNeeds::StartClose => (
            format!(
                "{PREFIX}; the new task must stay open to take in other tasks; start or close it in a later item"
            ),
            None,
        ),
        EditorMergeNeeds::Project => (
            format!(
                "{PREFIX}; a new project keeps tasks as tasks: list them as - ^id bullets to move them in"
            ),
            None,
        ),
        EditorMergeNeeds::Existing | EditorMergeNeeds::TargetRun => {
            let left = {
                let mut left_end = parent.text.len();
                for (index, _) in parent.text.char_indices() {
                    if parent.text[index..].starts_with('<') {
                        let before_ok = index == 0
                            || parent.text[..index]
                                .chars()
                                .next_back()
                                .is_some_and(|character| {
                                    character.is_whitespace()
                                });
                        if before_ok {
                            left_end = index;
                            break;
                        }
                    }
                }
                parent.text[..left_end].trim().to_string()
            };
            if super::transfer::is_single_destination_token(&left) {
                let sources_text = sources
                    .iter()
                    .map(|source| source.marker.as_str())
                    .collect::<Vec<_>>()
                    .join(" ");
                let replacement = format!("{sources_text} > {left}");
                // Preserve leading indentation.
                let leading_len =
                    parent.text.len() - parent.text.trim_start().len();
                let leading =
                    parent.text.get(..leading_len).unwrap_or("");
                let mut full_replacement = replacement;
                if !full_replacement.starts_with(leading)
                    && !leading.is_empty()
                {
                    full_replacement =
                        format!("{leading}{}", full_replacement.trim_start());
                }
                (
                    format!(
                        "{PREFIX}; {left} already exists. Write {sources_text} > {left}"
                    ),
                    Some(DiagnosticRewrite {
                        start: parent.start,
                        end: parent.end,
                        replacement: full_replacement,
                        label: "Use >".to_string(),
                    }),
                )
            } else {
                (
                    format!("{PREFIX}; this line names an existing task"),
                    None,
                )
            }
        }
        EditorMergeNeeds::NoTask => (
            format!("{PREFIX}; this line creates no task"),
            None,
        ),
        EditorMergeNeeds::Allowed => unreachable!(),
    }
}

fn editor_merge_source_also_marked_message(
    parsed: &EditorItemParse,
    sources: &[super::merge_in::MergeInSource],
) -> Option<String> {
    for source in sources {
        let route = source.route.to_ascii_lowercase();
        for dependency in &parsed.dependencies {
            if dependency.note.to_ascii_lowercase() == route
                && dependency.block_id == source.block_id
            {
                return Some(
                    "that source merges into the new task; it can't also be its prerequisite/dependent/relation"
                        .to_string(),
                );
            }
        }
        for blocked in &parsed.blocked_tasks {
            if blocked.note.to_ascii_lowercase() == route
                && blocked.block_id == source.block_id
            {
                return Some(
                    "that source merges into the new task; it can't also be its prerequisite/dependent/relation"
                        .to_string(),
                );
            }
        }
        for relation in &parsed.relations {
            if relation.note.to_ascii_lowercase() == route
                && relation.block_id == source.block_id
            {
                return Some(
                    "that source merges into the new task; it can't also be its prerequisite/dependent/relation"
                        .to_string(),
                );
            }
        }
    }
    None
}

fn parse_editor_item_without_transfer<'a>(
    item: &CaptureItem<'a>,
    options: &EditorParseOptions<'_>,
) -> EditorItemOutcome<'a> {
    if let Some(outcome) = try_parse_editor_target_run(item, options) {
        return outcome;
    }
    parse_editor_item_singleton_with(item, options)
}

fn parse_editor_transfer_item<'a>(
    item: &CaptureItem<'a>,
    scan: super::transfer::TransferScan,
) -> EditorItemOutcome<'a> {
    if let Some(list) = item.list.as_ref() {
        let parent = item.lines.first().map(|line| line.raw);
        let marker = parent.map(|line| line.text.trim()).unwrap_or_default();
        let header = list
            .section
            .as_deref()
            .map(|section| format!("@{}#{section}", list.route))
            .unwrap_or_else(|| format!("@{}", list.route));
        let suggestion =
            format!("give it its own item (a blank line, then `{marker}`)");
        let message = super::project_tasks::list_bullet_marker_error(
            &header,
            &format!("{}.md", list.route),
            marker,
            &suggestion,
        );
        return list_refusal_item(
            item,
            list,
            Some(("list_bullet_marker", message)),
            parent.map(|line| (line.start, line.end)),
        );
    }

    use super::transfer::TransferScan;
    let mut spans = Vec::new();
    let mut diagnostics = Vec::new();
    let (mode, route, section, block_id, needs, transfer) = match scan {
        TransferScan::Incomplete {
            sources,
            arrow,
            need,
            span,
            message,
        } => {
            spans.extend(sources.iter().map(transfer_source_span));
            spans.push(transfer_span(
                arrow.start,
                arrow.end,
                SpanKind::TransferArrow,
            ));
            let destination_range = if span == (arrow.start, arrow.end) {
                (arrow.end, arrow.end)
            } else {
                span
            };
            if need != super::transfer::TransferNeed::Task {
                spans.push(transfer_span(
                    destination_range.0,
                    destination_range.1,
                    SpanKind::TransferDestination,
                ));
            }
            add_incomplete_transfer_selector_span(
                &mut spans,
                item.source,
                destination_range,
                need,
            );
            diagnostics.push(Diagnostic {
                severity: Severity::Error,
                code: "incomplete_transfer",
                message,
                range: Some(span),
                rewrite: None,
            });
            (
                EditorMode::Incomplete,
                None,
                None,
                None,
                vec![transfer_need(need)],
                None,
            )
        }
        TransferScan::Invalid {
            code,
            message,
            span,
        } => {
            diagnostics.push(Diagnostic {
                severity: Severity::Error,
                code,
                message,
                range: Some(span),
                rewrite: None,
            });
            (EditorMode::Incomplete, None, None, None, Vec::new(), None)
        }
        TransferScan::Valid {
            sources,
            arrow,
            destination,
        } => {
            spans.extend(sources.iter().map(transfer_source_span));
            spans.push(transfer_span(
                arrow.start,
                arrow.end,
                SpanKind::TransferArrow,
            ));
            spans.push(transfer_span(
                destination.start,
                destination.end,
                SpanKind::TransferDestination,
            ));
            add_transfer_selector_spans(&mut spans, &destination);
            let mode = match destination.kind {
                super::transfer::TransferDestinationKind::Note => {
                    EditorMode::TaskMove
                }
                super::transfer::TransferDestinationKind::Task
                | super::transfer::TransferDestinationKind::Drawer
                | super::transfer::TransferDestinationKind::Section => {
                    EditorMode::TaskMerge
                }
            };
            let section = destination
                .drawer
                .clone()
                .or_else(|| destination.section.clone());
            let transfer = EditorTransfer {
                sources: sources
                    .into_iter()
                    .map(|source| EditorTransferSource {
                        start: source.start,
                        end: source.end,
                        marker: source.marker,
                        route: source.route,
                        block_id: source.block_id,
                    })
                    .collect(),
                arrow: EditorTransferArrow {
                    start: arrow.start,
                    end: arrow.end,
                },
                destination: EditorTransferDestination {
                    start: destination.start,
                    end: destination.end,
                    kind: match destination.kind {
                        super::transfer::TransferDestinationKind::Note => {
                            EditorTransferDestinationKind::Note
                        }
                        super::transfer::TransferDestinationKind::Task => {
                            EditorTransferDestinationKind::Task
                        }
                        super::transfer::TransferDestinationKind::Drawer => {
                            EditorTransferDestinationKind::Drawer
                        }
                        super::transfer::TransferDestinationKind::Section => {
                            EditorTransferDestinationKind::Section
                        }
                    },
                    route: destination.route.clone(),
                    block_id: destination.block_id.clone(),
                    drawer: destination.drawer.clone(),
                    section: destination.section.clone(),
                },
            };
            (
                mode,
                Some(destination.route),
                section,
                destination.block_id,
                Vec::new(),
                Some(transfer),
            )
        }
        TransferScan::Unclaimed | TransferScan::Escaped { .. } => {
            unreachable!("only claimed transfer scans reach this parser")
        }
    };
    spans.sort_by_key(|span| (span.start, span.end));
    let item_parse = empty_editor_item(
        item,
        mode,
        route,
        section,
        block_id,
        needs,
        spans,
        diagnostics,
    );
    let _ = transfer;
    EditorItemOutcome {
        item: item_parse,
        declarations: Vec::new(),
    }
}

fn empty_editor_item(
    item: &CaptureItem<'_>,
    mode: EditorMode,
    route: Option<String>,
    section: Option<String>,
    block_id: Option<String>,
    needs: Vec<Need>,
    spans: Vec<Span>,
    diagnostics: Vec<Diagnostic>,
) -> EditorItemParse {
    EditorItemParse {
        index: item.index,
        start: item.start,
        end: item.end,
        line_start: item.line_start,
        line_end: item.line_end,
        body: String::new(),
        mode,
        route,
        section,
        block_id,
        needs,
        ref_listen: false,
        pomodoro_start: None,
        pomodoro_adjust: None,
        pomodoro_shift: None,
        pomodoro_split: None,
        pomodoro_merge: None,
        pomodoro_close: None,
        pomodoro_past: None,
        dependencies: Vec::new(),
        blocked_tasks: Vec::new(),
        relations: Vec::new(),
        dependency_target: None,
        task_complete: None,
        list: item.list.clone(),
        spans,
        diagnostics,
        sub_bullets: Vec::new(),
        has_local_destination: false,
        local_destination_markers: Vec::new(),
        drawer_bullet_target: None,
        targets: Vec::new(),
        merge_in: None,
    }
}

fn transfer_source_span(source: &super::transfer::TransferSource) -> Span {
    transfer_span(source.start, source.end, SpanKind::TransferSource)
}

fn transfer_span(start: usize, end: usize, kind: SpanKind) -> Span {
    Span {
        start,
        end,
        kind,
        project_note: None,
        title: None,
        key: None,
    }
}

fn transfer_need(need: super::transfer::TransferNeed) -> Need {
    match need {
        super::transfer::TransferNeed::Destination => Need::TransferDestination,
        super::transfer::TransferNeed::Task => Need::TaskParent,
        super::transfer::TransferNeed::BlockId => Need::BlockId,
        super::transfer::TransferNeed::Drawer => Need::Drawer,
        super::transfer::TransferNeed::Section => Need::Section,
    }
}

fn add_incomplete_transfer_selector_span(
    spans: &mut Vec<Span>,
    source: &str,
    range: (usize, usize),
    need: super::transfer::TransferNeed,
) {
    let Some(token) = source.get(range.0..range.1) else {
        return;
    };
    if need == super::transfer::TransferNeed::Task {
        spans.push(transfer_span(
            range.0,
            range.1,
            SpanKind::InteractivePlaceholder,
        ));
        return;
    }
    let Some(hash) = token.find('#') else {
        if need == super::transfer::TransferNeed::BlockId
            && let Some(plus) = token.find('+')
        {
            let at = range.0 + plus + 1;
            spans.push(transfer_span(at, at, SpanKind::TaskBlockId));
        }
        return;
    };
    match need {
        super::transfer::TransferNeed::BlockId => {
            let end = range.0 + hash;
            let plus = token.find('+').map(|plus| range.0 + plus + 1);
            let start = plus.unwrap_or(end).min(end);
            spans.push(transfer_span(start, end, SpanKind::TaskBlockId));
        }
        super::transfer::TransferNeed::Drawer => {
            let start = range.0 + hash + 1;
            spans.push(transfer_span(start, range.1, SpanKind::Drawer));
        }
        super::transfer::TransferNeed::Section => {
            let start = range.0 + hash + 1;
            spans.push(transfer_span(start, range.1, SpanKind::Section));
        }
        super::transfer::TransferNeed::Destination
        | super::transfer::TransferNeed::Task => {}
    }
}

fn add_transfer_selector_spans(
    spans: &mut Vec<Span>,
    destination: &super::transfer::TransferDestination,
) {
    let Some(hash) = destination.marker.find('#') else {
        return;
    };
    let start = destination.start + hash + 1;
    let end = destination.end;
    if let Some(drawer) = destination.drawer.as_deref() {
        let title = if destination.drawer_exact {
            drawer.to_string()
        } else {
            super::normalize_task_text(&drawer.replace('-', " "))
                .to_ascii_uppercase()
        };
        let key = crate::native::capture_drawers::drawer_key(&title);
        let mut span = transfer_span(start, end, SpanKind::Drawer);
        span.title = Some(title);
        span.key = Some(key);
        spans.push(span);
    } else if destination.section.is_some() {
        spans.push(transfer_span(start, end, SpanKind::Section));
    }
}

fn parse_editor_item_singleton_with<'a>(
    item: &CaptureItem<'a>,
    options: &EditorParseOptions<'_>,
) -> EditorItemOutcome<'a> {
    parse_editor_item_singleton_with_target_run(item, options, None)
}

fn parse_editor_item_singleton_with_target_run<'a>(
    item: &CaptureItem<'a>,
    options: &EditorParseOptions<'_>,
    target_run: Option<&super::item::TargetRunContext>,
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
    let has_template = resolved
        .as_ref()
        .is_some_and(|directives| directives.template);
    if let Some(list) = item.list.as_ref() {
        if has_template {
            return list_refusal_item(
                item,
                list,
                Some(("template_list", super::draft::template_list_error())),
                None,
            );
        }
        if has_note {
            let suggestion =
                list_bullet_marker_suggestion("%n", "", &list.route);
            let header = list_header_text(item, list);
            let file = format!("{}.md", list.route);
            return list_refusal_item(
                item,
                list,
                Some((
                    "list_bullet_marker",
                    list_bullet_marker_error(&header, &file, "%n", &suggestion),
                )),
                None,
            );
        }
        if has_listen {
            let suggestion =
                list_bullet_marker_suggestion("%l", "", &list.route);
            let header = list_header_text(item, list);
            let file = format!("{}.md", list.route);
            return list_refusal_item(
                item,
                list,
                Some((
                    "list_bullet_marker",
                    list_bullet_marker_error(&header, &file, "%l", &suggestion),
                )),
                None,
            );
        }
    }
    if occurrences.is_empty() && directive_occurrences.is_empty() {
        let mut outcome =
            parse_editor_item_inner(item, options, false, false, target_run);
        attach_list_header_marker_diagnostic(item, &mut outcome.item);
        return outcome;
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
    let inner = parse_editor_item_inner(
        &masked_item,
        options,
        has_clip,
        has_listen,
        target_run,
    );
    let declarations =
        remap_type_marker_declarations(inner.declarations, item.source);
    let mut outcome = EditorItemOutcome {
        item: inner.item,
        declarations,
    };
    if has_note {
        apply_note_directive(&mut outcome.item);
    }
    attach_type_marker_metadata(&mut outcome.item, &occurrences, has_clip);
    attach_directive_metadata(&mut outcome.item, &directive_occurrences);
    if has_listen {
        apply_listen_directive(&mut outcome.item);
    }
    if !has_note && !has_listen && !has_template {
        attach_list_header_marker_diagnostic(item, &mut outcome.item);
    }
    outcome
}

/// Target-run fan-in for the editor: reuse shared selection and the
/// existing classifier once per member, then combine into one
/// [`EditorItemParse`]. Leading/terminal selection matches execution;
/// `SeenMarkers` never flags the second member as a cross-line duplicate
/// because each member parses in its own masked view. Returns `None` when
/// the item holds no run and no refusal, so the singleton path runs.
fn try_parse_editor_target_run<'a>(
    item: &CaptureItem<'a>,
    options: &EditorParseOptions<'_>,
) -> Option<EditorItemOutcome<'a>> {
    let (run, refusal) = super::item::find_editor_target_run(item);
    let Some(run) = run else {
        let refusal = refusal?;
        let mut outcome = parse_editor_item_singleton_with(item, options);
        if let Some(diagnostic) =
            super::target_runs::editor_refusal_diagnostic(&refusal)
            && !outcome.item.diagnostics.iter().any(|existing| {
                existing.code == diagnostic.code
                    && existing.range == diagnostic.range
                    && existing.message == diagnostic.message
            })
        {
            outcome.item.diagnostics.push(diagnostic);
        }
        return Some(outcome);
    };
    let mut combined = combine_editor_target_members(item, &run, options);
    if let Some(diagnostic) =
        retired_priority_marker_diagnostic(&run.terminal_markers)
        && !combined.item.diagnostics.iter().any(|existing| {
            existing.code == diagnostic.code
                && existing.range == diagnostic.range
                && existing.message == diagnostic.message
        })
    {
        combined.item.diagnostics.push(diagnostic);
    }
    if let Some(refusal) = refusal
        && let Some(diagnostic) =
            super::target_runs::editor_refusal_diagnostic(&refusal)
        && !combined.item.diagnostics.iter().any(|existing| {
            existing.code == diagnostic.code
                && existing.range == diagnostic.range
                && existing.message == diagnostic.message
        })
    {
        combined.item.diagnostics.push(diagnostic);
    }
    Some(combined)
}

/// Parse every member view through the established singleton pipeline and
/// merge into one item. Each view masks every other run member with spaces,
/// so absolute byte offsets never shift and each member's classifier runs
/// exactly like its single-target equivalent. Shared spans, diagnostics,
/// and sub-bullets deduplicate by value; member-specific spans and exact
/// diagnostics are all kept. Legacy mode/route/section/block-ID,
/// dependency owner, type-marker subject, and drawer target stay the first
/// member's values; every local destination marker is retained.
fn combine_editor_target_members<'a>(
    item: &CaptureItem<'a>,
    run: &super::item::TargetRunContext,
    options: &EditorParseOptions<'_>,
) -> EditorItemOutcome<'a> {
    let mut members: Vec<EditorItemOutcome<'a>> = Vec::new();
    for index in 0..run.selection.members.len() {
        let masked_source =
            super::target_runs::masked_source_for_member_with_terminals(
                item.source,
                &run.selection.members,
                index,
                &run.terminal_spans,
            );
        let lines = item
            .lines
            .iter()
            .map(|line| ItemLine {
                raw: RawLine {
                    text: masked_source
                        .get(line.raw.start..line.raw.end)
                        .unwrap_or(""),
                    start: line.raw.start,
                    end: line.raw.end,
                },
                line_number: line.line_number,
            })
            .collect::<Vec<_>>();
        let view = CaptureItem {
            source: masked_source.as_str(),
            index: item.index,
            start: item.start,
            end: item.end,
            line_start: item.line_start,
            line_end: item.line_end,
            lines,
            list: item.list.clone(),
        };
        let parsed = parse_editor_item_singleton_with_target_run(
            &view,
            options,
            Some(run),
        );
        let declarations =
            remap_type_marker_declarations(parsed.declarations, item.source);
        members.push(EditorItemOutcome {
            item: parsed.item,
            declarations,
        });
    }
    let Some(first) = members.first() else {
        return parse_editor_item_singleton_with(item, options);
    };
    let mut needs: Vec<Need> = Vec::new();
    for outcome in &members {
        for need in &outcome.item.needs {
            if !needs.contains(need) {
                needs.push(*need);
            }
        }
    }
    let mut spans: Vec<Span> = Vec::new();
    for outcome in &members {
        for span in &outcome.item.spans {
            if !spans.contains(span) {
                spans.push(span.clone());
            }
        }
    }
    spans.sort_by_key(|span| (span.start, span.end));
    let mut diagnostics: Vec<Diagnostic> = Vec::new();
    for outcome in &members {
        for diagnostic in &outcome.item.diagnostics {
            if !diagnostics.iter().any(|existing| {
                existing.code == diagnostic.code
                    && existing.range == diagnostic.range
                    && existing.message == diagnostic.message
                    && existing.severity == diagnostic.severity
            }) {
                diagnostics.push(diagnostic.clone());
            }
        }
    }
    let mut sub_bullets: Vec<AuthoredSubBullet> = Vec::new();
    for outcome in &members {
        for bullet in &outcome.item.sub_bullets {
            if !sub_bullets.contains(bullet) {
                sub_bullets.push(bullet.clone());
            }
        }
    }
    let mut local_destination_markers: Vec<LocalDestinationMarker> = Vec::new();
    for outcome in &members {
        for marker in &outcome.item.local_destination_markers {
            if !local_destination_markers.contains(marker) {
                local_destination_markers.push(marker.clone());
            }
        }
    }
    let targets = run
        .selection
        .members
        .iter()
        .zip(members.iter())
        .map(|(member, outcome)| EditorTarget {
            start: member.start,
            end: member.end,
            marker: member.marker_text.clone(),
            mode: outcome.item.mode,
            route: outcome.item.route.clone(),
            block_id: outcome.item.block_id.clone(),
            section: outcome.item.section.clone(),
        })
        .collect::<Vec<_>>();
    let declarations = first.declarations.clone();
    let item = EditorItemParse {
        index: first.item.index,
        start: first.item.start,
        end: first.item.end,
        line_start: first.item.line_start,
        line_end: first.item.line_end,
        body: first.item.body.clone(),
        mode: first.item.mode,
        route: first.item.route.clone(),
        section: first.item.section.clone(),
        block_id: first.item.block_id.clone(),
        needs,
        ref_listen: first.item.ref_listen,
        pomodoro_start: first.item.pomodoro_start.clone(),
        pomodoro_adjust: first.item.pomodoro_adjust.clone(),
        pomodoro_shift: first.item.pomodoro_shift.clone(),
        pomodoro_split: first.item.pomodoro_split.clone(),
        pomodoro_merge: first.item.pomodoro_merge.clone(),
        pomodoro_close: first.item.pomodoro_close.clone(),
        pomodoro_past: first.item.pomodoro_past.clone(),
        dependencies: first.item.dependencies.clone(),
        blocked_tasks: first.item.blocked_tasks.clone(),
        relations: first.item.relations.clone(),
        dependency_target: first.item.dependency_target.clone(),
        task_complete: first.item.task_complete.clone(),
        list: first.item.list.clone(),
        spans,
        diagnostics,
        sub_bullets,
        has_local_destination: members
            .iter()
            .any(|outcome| outcome.item.has_local_destination),
        local_destination_markers,
        drawer_bullet_target: first.item.drawer_bullet_target.clone(),
        targets,
        merge_in: None,
    };
    EditorItemOutcome { item, declarations }
}

/// Refuse two equal task IDs inside one capture list, mirroring the
/// execution `duplicate_project_task_id` refusal: the diagnostic lands on
/// the second bullet and names both draft lines, with the ID token's range.
fn attach_list_duplicate_id_diagnostics(
    outcomes: &mut [EditorItemOutcome<'_>],
) {
    let mut seen: Vec<(usize, usize, String, usize)> = Vec::new();
    for outcome in outcomes.iter_mut() {
        let Some(list) = outcome
            .item
            .list
            .as_ref()
            .filter(|list| !list.ownerless && list.kind == ListKind::Tasks)
        else {
            continue;
        };
        let Some(block_id) = outcome.item.block_id.clone() else {
            continue;
        };
        if let Some((_, _, _, first_line)) = seen.iter().find(|seen| {
            seen.0 == list.header_start
                && seen.1 == list.header_end
                && seen.2 == block_id
        }) {
            let first_line = *first_line;
            let range = duplicate_id_token_range(&outcome.item.spans);
            outcome.item.diagnostics.push(Diagnostic {
                severity: Severity::Error,
                code: "duplicate_project_task_id",
                message: duplicate_project_task_id_error(
                    &block_id,
                    first_line,
                    outcome.item.line_start,
                ),
                range,
                rewrite: None,
            });
            continue;
        }
        seen.push((
            list.header_start,
            list.header_end,
            block_id,
            outcome.item.line_start,
        ));
    }
}

/// The ID token's range: the block-ID span widened left over its `:` link
/// sigil when one directly precedes it.
fn duplicate_id_token_range(spans: &[Span]) -> Option<(usize, usize)> {
    let block = spans
        .iter()
        .find(|span| span.kind == SpanKind::ProjectTaskBlockId)?;
    let start = spans
        .iter()
        .filter(|span| {
            span.kind == SpanKind::ProjectTaskLinkMarker
                && span.end == block.start
        })
        .map(|span| span.start)
        .min()
        .unwrap_or(block.start);
    Some((start, block.end))
}

/// A route marker plus only item-wide markers on the first line with
/// bullets below: the header must stand alone. Mirrors the execution
/// `list_header_marker` refusal with the same code and copy.
fn attach_list_header_marker_diagnostic(
    item: &CaptureItem<'_>,
    parsed: &mut EditorItemParse,
) {
    if parsed.list.is_some() {
        return;
    }
    if !matches!(parsed.mode, EditorMode::Task | EditorMode::Bullet) {
        return;
    }
    // Either the marker was selected and the body emptied, or a bare
    // `@route` stayed literal (its trailing markers extracted first).
    let body_empty = parsed.body.trim().is_empty();
    let body_is_bare_header = !body_empty
        && parsed.route.is_none()
        && item.lines.first().is_some_and(|parent| {
            parent.raw.text.split_whitespace().next()
                == Some(parsed.body.trim())
        });
    if !body_empty && !body_is_bare_header {
        return;
    }
    if !parsed.dependencies.is_empty()
        || !parsed.blocked_tasks.is_empty()
        || !parsed.relations.is_empty()
    {
        return;
    }
    let Some(first) = parsed
        .sub_bullets
        .iter()
        .find(|bullet| bullet.depth == AuthoredDepth::First)
    else {
        return;
    };
    let Some(parent) = item.lines.first() else {
        return;
    };
    let mut tokens = parent.raw.text.split_whitespace();
    let Some(first_token) = tokens.next() else {
        return;
    };
    let Some(stray) = tokens.next() else {
        return;
    };
    let marker_text = parsed
        .local_destination_markers
        .first()
        .map(|marker| marker.text.as_str())
        .unwrap_or("");
    // Either the marker was selected, or a bare `@route` stayed literal.
    let first_is_header =
        super::tokens::parse_terminal_route_token(first_token)
            .ok()
            .flatten()
            .is_some_and(|route| {
                matches!(
                    route.kind,
                    CaptureKind::Task | CaptureKind::Bullet { .. }
                )
            });
    if first_token != marker_text && !first_is_header {
        return;
    }
    if parsed
        .diagnostics
        .iter()
        .any(|diagnostic| diagnostic.code == "list_header_marker")
    {
        return;
    }
    // Underline the marker that must move to the bullet: the second token
    // on the header line, located after the header marker itself.
    let range = parent.raw.text.find(first_token).and_then(|token_start| {
        let search_from = token_start + first_token.len();
        parent.raw.text[search_from..].find(stray).map(|offset| {
            let start = parent.raw.start + search_from + offset;
            (start, start + stray.len())
        })
    });
    parsed.diagnostics.push(Diagnostic {
        severity: Severity::Error,
        code: "list_header_marker",
        message: list_header_marker_error(
            first_token,
            stray,
            first.body.trim(),
        ),
        range,
        rewrite: None,
    });
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
            rewrite: None,
        });
        return;
    }
    parsed.mode = EditorMode::PomodoroNote;
    // A Pomodoro note's bullets are note lines, never drawers.
    parsed.drawer_bullet_target = None;
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
        rewrite: None,
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
    has_clip: bool,
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
                    rewrite: None,
                });
                parsed.spans.sort_by_key(|span| (span.start, span.end));
                return;
            }
            // A relationship-only SubBullet owner accepts the marker.
            // A note-less SubBullet whose notes come from first-level
            // bullets or a clip directive retypes the target the same way.
            let has_first_level_notes = parsed
                .sub_bullets
                .iter()
                .any(|bullet| bullet.depth == AuthoredDepth::First);
            let relationship_sub_bullet = parsed.mode == EditorMode::SubBullet
                && parsed.body.trim().is_empty()
                && (parsed.sub_bullets.is_empty()
                    && matches!(
                        parsed.dependency_target.as_ref(),
                        Some(target) if target.kind
                            == DependencyTargetKind::ExistingTask
                    )
                    || has_first_level_notes
                    || has_clip);
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
                    rewrite: None,
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
                    rewrite: None,
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
                rewrite: None,
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
    use crate::native::capture_drawers::reserved_selector_error;

    let has_first_level = sub_bullets
        .iter()
        .any(|bullet| bullet.depth == AuthoredDepth::First);
    let is_glued = glued_section.is_some_and(|section| !section.is_empty());
    // A note-less `@route+id` item with first-level bullets files each
    // bullet as its own note, so the existing target is an under-task
    // position for drawer bullets. Glued drawer items stay
    // not-under-task: every bullet already lives inside that drawer.
    let under_task = is_under_task_mode(*mode)
        || (*mode == EditorMode::SubBullet
            && parent_body.trim().is_empty()
            && !is_glued
            && has_first_level);
    // Managed names (`TASKS`, `SCHEDULE LOG`, …) are refused as task
    // drawers and as existing-target sections, but a newly created project
    // note owns its structural headings, so `- TASKS` there is a valid
    // merge. Structural `^prj` headings and near-duplicates depend on the
    // vault and are left to the commit-time resolver.
    let reserved_applies = under_task
        && !matches!(
            *mode,
            EditorMode::ProjectNote | EditorMode::PomodoroProjectNote
        );
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
                selector,
                title,
                key,
                leftover,
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
                        rewrite: None,
                    });
                    continue;
                }
                if reserved_applies
                    && let Some(error) =
                        reserved_selector_error(&selector.text, selector.exact)
                {
                    diagnostics.push(Diagnostic {
                        severity: Severity::Error,
                        code: error.code,
                        message: error.message,
                        range: Some((span.start, span.end)),
                        rewrite: None,
                    });
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
                    if reserved_applies
                        && let Some(error) =
                            reserved_selector_error(&title, true)
                    {
                        diagnostics.push(Diagnostic {
                            severity: Severity::Error,
                            code: error.code,
                            message: error.message,
                            range: Some((start, end)),
                            rewrite: None,
                        });
                    }
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
                    rewrite: None,
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
        EditorMode::PomodoroSplit => (
            "invalid_pomodoro_split",
            "Pomodoro split `=/` cannot be combined with %c; capture the split alone"
                .to_string(),
        ),
        EditorMode::PomodoroMerge => (
            "invalid_pomodoro_merge",
            "Pomodoro merge `=*` cannot be combined with %c; capture the merge alone"
                .to_string(),
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
        rewrite: None,
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
        EditorMode::PomodoroSplit => {
            Some(super::type_marker::TypeMarkerSubject::Split)
        }
        EditorMode::PomodoroMerge => {
            Some(super::type_marker::TypeMarkerSubject::Merge)
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

/// The header token text of a list item, trimmed.
pub(super) fn list_header_text(
    item: &CaptureItem<'_>,
    list: &ListContext,
) -> String {
    item.source
        .get(list.header_start..list.header_end)
        .unwrap_or("")
        .trim()
        .to_string()
}

/// One refusal diagnostic item for a list header block: `incomplete` with
/// the header spans and no other content, so a cursor or diagnostic on the
/// header line resolves to the list's first item. A `None` diagnostic marks
/// a still-typing incomplete state (the empty list), which needs
/// `list_item` but reports no error.
pub(super) fn list_refusal_item<'a>(
    item: &CaptureItem<'a>,
    list: &ListContext,
    diagnostic: Option<(&'static str, String)>,
    range: Option<(usize, usize)>,
) -> EditorItemOutcome<'a> {
    let spans = list_header_spans(item, list);
    let mut diagnostics = Vec::new();
    if let Some((code, message)) = diagnostic {
        diagnostics.push(Diagnostic {
            severity: Severity::Error,
            code,
            message,
            range,
            rewrite: None,
        });
    }
    EditorItemOutcome {
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
            needs: Vec::new(),
            ref_listen: false,
            pomodoro_start: None,
            pomodoro_adjust: None,
            pomodoro_shift: None,
            pomodoro_split: None,
            pomodoro_merge: None,
            pomodoro_close: None,
            pomodoro_past: None,
            dependencies: Vec::new(),
            blocked_tasks: Vec::new(),
            relations: Vec::new(),
            dependency_target: None,
            task_complete: None,
            list: Some(list.clone()),
            spans,
            diagnostics,
            sub_bullets: Vec::new(),
            has_local_destination: true,
            local_destination_markers: Vec::new(),
            drawer_bullet_target: None,
            targets: Vec::new(),
            merge_in: None,
        },
        declarations: Vec::new(),
    }
}

/// Exactly the spans a leading `@route` / `@route#section` marker gets,
/// re-lexed on the header token so the editor and execution agree.
pub(super) fn list_header_spans(
    item: &CaptureItem<'_>,
    list: &ListContext,
) -> Vec<Span> {
    let text = item
        .source
        .get(list.header_start..list.header_end)
        .unwrap_or("");
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Vec::new();
    }
    let leading = text.len() - text.trim_start().len();
    let token = Token {
        text: trimmed,
        start: list.header_start + leading,
        end: list.header_start + leading + trimmed.len(),
    };
    super::editor_classify::classify_route_token(&token)
        .map(|marker| marker.spans)
        .unwrap_or_default()
}

/// Parse one list bullet block for the editor: the same modes, spans,
/// needs, and diagnostics the execution parser reports, with an unfinished
/// ID as `incomplete` needing `block_id` plus a placeholder span and no
/// diagnostic.
pub(super) fn parse_editor_list_item<'a>(
    item: &CaptureItem<'a>,
    options: &EditorParseOptions<'_>,
) -> EditorItemOutcome<'a> {
    let _ = options;
    let list = item.list.clone().expect("list bullet item");
    let header = list_header_text(item, &list);
    let file = format!("{}.md", list.route);
    let notes = list.kind == ListKind::Notes;
    let mut spans = list_header_spans(item, &list);
    let mut diagnostics: Vec<Diagnostic> = Vec::new();
    let mut needs: Vec<Need> = Vec::new();
    let error_item = |spans: Vec<Span>,
                      needs: Vec<Need>,
                      code: &'static str,
                      message: String,
                      range: Option<(usize, usize)>| {
        let mut outcome =
            list_refusal_item(item, &list, Some((code, message)), range);
        outcome.item.spans = spans;
        outcome.item.needs = needs;
        outcome
    };
    if list.ownerless {
        let line = &item.lines[0];
        let (code, message) = match classify_authored_line(line.raw) {
            AuthoredLineClass::Item(authored)
                if authored.depth == AuthoredDepth::Nested =>
            {
                (
                    "orphaned_nested_bullet",
                    orphaned_nested_bullet_error(line.line_number),
                )
            }
            _ => (
                "invalid_child_line",
                invalid_child_line_error(line.line_number),
            ),
        };
        return error_item(
            spans.clone(),
            needs.clone(),
            code,
            message,
            Some((line.raw.start, line.raw.end)),
        );
    }
    if list.count == 0 {
        // A lone `@route` task header is a note pull (§12.20): mode
        // `note_pull` with no needs or diagnostics, keeping the route
        // spans and task-list metadata (`count: 0`) so clients can still
        // offer "start a list" from it.
        if !notes && list.section.is_none() {
            let mut outcome = list_refusal_item(
                item,
                &list,
                None,
                Some((list.header_start, list.header_end)),
            );
            outcome.item.spans = spans;
            outcome.item.needs = Vec::new();
            outcome.item.mode = EditorMode::NotePull;
            outcome.item.route = Some(list.route.clone());
            outcome.item.section = None;
            return outcome;
        }
        let mut outcome = list_refusal_item(
            item,
            &list,
            None,
            Some((list.header_start, list.header_end)),
        );
        outcome.item.spans = spans;
        outcome.item.needs = vec![Need::ListItem];
        outcome.item.route = Some(list.route.clone());
        outcome.item.section = list.section.clone();
        return outcome;
    }
    let parent_line = item.lines[0];
    let raw_body = item
        .source
        .get(parent_line.raw.start..parent_line.raw.end)
        .unwrap_or("");
    let mut body_text = raw_body.trim().to_string();
    if !notes {
        let (checkbox, rest) = split_leading_checkbox(raw_body.trim_start());
        if let Some(status) = checkbox {
            if status != ' ' {
                return error_item(
                    spans.clone(),
                    needs.clone(),
                    "list_bullet_checkbox",
                    list_bullet_checkbox_error(status),
                    Some((parent_line.raw.start, parent_line.raw.end)),
                );
            }
            body_text = rest.to_string();
        }
    }
    if body_text.trim_start().starts_with('#') {
        let mut words = body_text.trim_start().split_whitespace();
        let first_word = words.next().unwrap_or("#");
        let rest_words: Vec<&str> = words.collect();
        let example = if rest_words.is_empty() {
            format!("- {first_word}")
        } else {
            format!("- {} {first_word}", rest_words.join(" "))
        };
        return error_item(
            spans.clone(),
            needs.clone(),
            "drawer_bullet_position",
            drawer_bullet_position_list_error(notes, &example),
            Some((parent_line.raw.start, parent_line.raw.end)),
        );
    }
    let parent_tokens = tokenize_line_with_spans(&parent_line.raw);
    let parent_parse = parse_editor_line(
        parent_tokens,
        parent_line.raw.text,
        parent_line.raw.start,
        false,
        !notes,
    );
    let mut declarations = global_declarations_from_tokens(
        parent_parse.declarations,
        parent_line.line_number,
    );
    spans.extend(parent_parse.terminal_spans);
    spans.extend(parent_parse.dependencies.spans.iter().cloned());
    diagnostics.extend(parent_parse.diagnostics);
    // A route marker on the bullet would file it elsewhere.
    if let Some(marker) = parent_parse.marker {
        let range = marker.spans.first().map(|span| (span.start, span.end));
        let marker_text = parent_parse.marker_text.clone().unwrap_or_default();
        let suggestion = list_bullet_marker_suggestion(
            &marker_text,
            parent_parse.body.trim(),
            &list.route,
        );
        return error_item(
            spans.clone(),
            needs.clone(),
            "list_bullet_marker",
            list_bullet_marker_error(&header, &file, &marker_text, &suggestion),
            range,
        );
    }
    if let Some(affix) =
        list_affix_token(&parent_parse.body_tokens, item.source)
    {
        let suggestion = list_bullet_marker_suggestion(
            affix,
            parent_parse.body.trim(),
            &list.route,
        );
        return error_item(
            spans.clone(),
            needs.clone(),
            "list_bullet_marker",
            list_bullet_marker_error(&header, &file, affix, &suggestion),
            parent_parse
                .body_tokens
                .iter()
                .find(|token| token.text == affix)
                .map(|token| (token.start, token.end)),
        );
    }
    // The stripped ID decides the mode: ` :id` links (pomodoro task), ` ^id`
    // names (plain task), and a section bullet never takes one.
    let (mut mode, mut block_id) = match list.kind {
        ListKind::Tasks => (EditorMode::Task, None),
        ListKind::Notes => (EditorMode::Bullet, None),
    };
    let route = Some(list.route.clone());
    let section = if notes { list.section.clone() } else { None };
    let owner_body = parent_parse.body.clone();
    if let Some(stripped) = parent_parse.stripped_task_id {
        let token = stripped.token;
        match stripped.lex {
            TaskIdLex::Lone { .. } => {
                mode = EditorMode::Incomplete;
                if !needs.contains(&Need::BlockId) {
                    needs.push(Need::BlockId);
                }
                spans.push(Span {
                    start: token.start,
                    end: token.end,
                    kind: SpanKind::InteractivePlaceholder,
                    project_note: None,
                    title: None,
                    key: None,
                });
            }
            TaskIdLex::Invalid { raw, .. } => {
                return error_item(
                    spans.clone(),
                    needs.clone(),
                    "invalid_project_task_id",
                    invalid_project_task_id_charset_error(&raw),
                    Some((token.start, token.end)),
                );
            }
            TaskIdLex::Valid { sigil, id } => {
                if is_reserved_project_task_id(&id) {
                    return error_item(
                        spans.clone(),
                        needs.clone(),
                        "invalid_project_task_id",
                        reserved_project_task_id_error(&id),
                        Some((token.start, token.end)),
                    );
                }
                if owner_body.trim().is_empty() {
                    return error_item(
                        spans.clone(),
                        needs.clone(),
                        "invalid_project_task_id",
                        empty_project_task_body_error(parent_line.line_number),
                        Some((token.start, token.end)),
                    );
                }
                if sigil == ':' {
                    mode = EditorMode::PomodoroTask;
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
                block_id = Some(id);
            }
        }
    }
    if parent_parse.body.trim().is_empty() && block_id.is_none() {
        mode = EditorMode::Incomplete;
    }
    let mut sub_bullets = Vec::new();
    for line in item.lines.iter().skip(1) {
        let line_number = line.line_number;
        let authored = match classify_authored_line(line.raw) {
            AuthoredLineClass::EmptyOrPlaceholder => continue,
            AuthoredLineClass::Invalid => {
                return error_item(
                    spans.clone(),
                    needs.clone(),
                    "invalid_child_line",
                    invalid_child_line_error(line_number),
                    Some((line.raw.start, line.raw.end)),
                );
            }
            AuthoredLineClass::Item(authored) => authored,
        };
        let child_line = RawLine {
            text: authored.body,
            start: authored.body_start,
            end: line.raw.end,
        };
        let child_tokens = tokenize_line_with_spans(&child_line);
        let child_parse = parse_editor_line(
            child_tokens,
            child_line.text,
            child_line.start,
            false,
            !notes,
        );
        declarations.extend(global_declarations_from_tokens(
            child_parse.declarations,
            line_number,
        ));
        spans.extend(child_parse.terminal_spans);
        spans.extend(child_parse.dependencies.spans.iter().cloned());
        diagnostics.extend(child_parse.diagnostics);
        if let Some(marker) = child_parse.marker {
            let range = marker.spans.first().map(|span| (span.start, span.end));
            let marker_text =
                child_parse.marker_text.clone().unwrap_or_default();
            let suggestion = list_bullet_marker_suggestion(
                &marker_text,
                owner_body.trim(),
                &list.route,
            );
            return error_item(
                spans.clone(),
                needs.clone(),
                "list_bullet_marker",
                list_bullet_marker_error(
                    &header,
                    &file,
                    &marker_text,
                    &suggestion,
                ),
                range,
            );
        }
        if let Some(affix) =
            list_affix_token(&child_parse.body_tokens, item.source)
        {
            let suggestion = list_bullet_marker_suggestion(
                affix,
                owner_body.trim(),
                &list.route,
            );
            return error_item(
                spans.clone(),
                needs.clone(),
                "list_bullet_marker",
                list_bullet_marker_error(&header, &file, affix, &suggestion),
                child_parse
                    .body_tokens
                    .iter()
                    .find(|token| token.text == affix)
                    .map(|token| (token.start, token.end)),
            );
        }
        if let Some(stripped) = child_parse.stripped_task_id {
            let token = stripped.token;
            match stripped.lex {
                TaskIdLex::Invalid { raw, .. } => {
                    return error_item(
                        spans.clone(),
                        needs.clone(),
                        "invalid_project_task_id",
                        invalid_project_task_id_charset_error(&raw),
                        Some((token.start, token.end)),
                    );
                }
                TaskIdLex::Valid { id, .. } => {
                    if is_reserved_project_task_id(&id) {
                        return error_item(
                            spans.clone(),
                            needs.clone(),
                            "invalid_project_task_id",
                            reserved_project_task_id_error(&id),
                            Some((token.start, token.end)),
                        );
                    }
                    return error_item(
                        spans.clone(),
                        needs.clone(),
                        "misplaced_project_task_id",
                        misplaced_list_task_id_error(
                            token.text,
                            owner_body.trim(),
                        ),
                        Some((token.start, token.end)),
                    );
                }
                TaskIdLex::Lone { .. } => {
                    mode = EditorMode::Incomplete;
                    if !needs.contains(&Need::BlockId) {
                        needs.push(Need::BlockId);
                    }
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
        }
        if child_parse.body.trim().is_empty() {
            diagnostics.push(Diagnostic {
                severity: Severity::Error,
                code: "empty_child_after_markers",
                message: empty_child_after_markers_error(line_number),
                range: Some((line.raw.start, line.raw.end)),
                rewrite: None,
            });
            continue;
        }
        // The bullet line is this item's text line, so its nested lines sit
        // one level higher than their lexical depth, mirroring the execution
        // parser and the equivalence invariant.
        let depth = match authored.depth {
            AuthoredDepth::Nested => AuthoredDepth::First,
            AuthoredDepth::First => AuthoredDepth::First,
        };
        sub_bullets.push(AuthoredSubBullet {
            body: child_parse.body,
            depth,
            task_id: None,
            move_task: None,
            move_choice: None,
            drawer: None,
        });
    }
    spans.sort_by_key(|span| (span.start, span.end));
    EditorItemOutcome {
        item: EditorItemParse {
            index: item.index,
            start: item.start,
            end: item.end,
            line_start: item.line_start,
            line_end: item.line_end,
            body: owner_body,
            mode,
            route,
            section,
            block_id,
            needs,
            ref_listen: false,
            pomodoro_start: None,
            pomodoro_adjust: None,
            pomodoro_shift: None,
            pomodoro_split: None,
            pomodoro_merge: None,
            pomodoro_close: None,
            pomodoro_past: None,
            dependencies: Vec::new(),
            blocked_tasks: Vec::new(),
            relations: Vec::new(),
            dependency_target: None,
            task_complete: None,
            list: Some(list),
            spans,
            diagnostics,
            sub_bullets,
            has_local_destination: true,
            local_destination_markers: Vec::new(),
            drawer_bullet_target: None,
            targets: Vec::new(),
            merge_in: None,
        },
        declarations,
    }
}

/// A trailing parent selector (`+`/`+query`) or task completion
/// (`!note:id`) among surviving body tokens: both would file the bullet
/// elsewhere, so the bullet is refused instead.
/// Interpret one move-bullet lexer hit in the editor project-note
/// post-pass, in source order. Accepted moves record an empty body, the
/// move, and the marker and block-ID spans; a `:` move also counts as a
/// link for the unused-`#pomodoro` rule and upgrades the mode. Violations
/// become diagnostics with the same codes and messages `bob capture`
/// reports. A lone sigil marks the item incomplete needing
/// `project_task_move` instead of recording anything.
#[allow(clippy::too_many_arguments)]
fn interpret_editor_move(
    hit: MoveBulletHit,
    pass: &mut ProjectTaskPass,
    sub_bullets: &mut [AuthoredSubBullet],
    spans: &mut Vec<Span>,
    needs: &mut Vec<Need>,
    mode: &mut EditorMode,
    diagnostics: &mut Vec<Diagnostic>,
    has_pending_colon_link: &mut bool,
    index: usize,
    line_number: usize,
    route: &str,
    new_id: Option<&str>,
) {
    let push_span =
        |spans: &mut Vec<Span>, start: usize, end: usize, kind: SpanKind| {
            spans.push(Span {
                start,
                end,
                kind,
                project_note: None,
                title: None,
                key: None,
            });
        };
    match hit {
        MoveBulletHit::Invalid { code, kind, span } => {
            let message = match &kind {
                MoveBulletInvalid::Text { sigil, id } => {
                    super::move_bullet::move_bullet_text_error(
                        *sigil, id, route,
                    )
                }
                MoveBulletInvalid::Marker { token } => {
                    super::move_bullet::move_bullet_marker_error(token)
                }
                MoveBulletInvalid::Checkbox { status } => {
                    super::move_bullet::move_bullet_checkbox_error(*status)
                }
                MoveBulletInvalid::CrossNote { sigil, id } => {
                    super::move_bullet::move_bullet_cross_note_error(
                        *sigil, id, route,
                    )
                }
                MoveBulletInvalid::Reserved { sigil, id } => {
                    super::move_bullet::move_bullet_reserved_error(
                        *sigil, id, route,
                    )
                }
                MoveBulletInvalid::BadId { raw } => {
                    invalid_project_task_id_charset_error(raw)
                }
                MoveBulletInvalid::ChoiceText => {
                    super::move_bullet::move_choice_text_error(route)
                }
                MoveBulletInvalid::ChoiceColon => {
                    super::move_bullet::move_choice_colon_error()
                }
            };
            diagnostics.push(Diagnostic {
                severity: Severity::Error,
                code,
                message,
                range: Some((span.start, span.end)),
                rewrite: None,
            });
        }
        MoveBulletHit::Choice { span } => {
            if sub_bullets
                .get(index + 1)
                .is_some_and(|next| next.depth == AuthoredDepth::Nested)
            {
                diagnostics.push(Diagnostic {
                    severity: Severity::Error,
                    code: "move_choice_nested",
                    message: super::move_bullet::move_choice_nested_error(),
                    range: Some((span.start, span.end)),
                    rewrite: None,
                });
                return;
            }
            if let Some(first) = pass.register_choice(line_number) {
                diagnostics.push(Diagnostic {
                    severity: Severity::Error,
                    code: "move_choice_duplicate",
                    message: super::move_bullet::move_choice_duplicate_error(
                        first,
                    ),
                    range: Some((span.start, span.end)),
                    rewrite: None,
                });
                return;
            }
            // A choice counts as a possible link for the unused-`#pomodoro`
            // rule, like an unfinished `- :`, but never upgrades the mode.
            *has_pending_colon_link = true;
            sub_bullets[index].body.clear();
            sub_bullets[index].move_choice =
                Some(super::model::MoveChoiceBullet {
                    range: (span.start, span.end),
                });
            push_span(
                spans,
                span.start,
                span.end,
                SpanKind::ProjectTaskMoveChoiceMarker,
            );
        }
        MoveBulletHit::Unfinished { sigil, span } => {
            *mode = EditorMode::Incomplete;
            if sigil == ':' {
                *has_pending_colon_link = true;
            }
            if !needs.contains(&Need::ProjectTaskMove) {
                needs.push(Need::ProjectTaskMove);
            }
            // The move stays visible to clients as an entry with a null
            // block ID, mirroring how complete moves report.
            sub_bullets[index].body.clear();
            sub_bullets[index].move_task = Some(MoveBullet {
                sigil,
                block_id: None,
                range: (span.start, span.end),
                via: super::model::MoveVia::Bullet,
                minted: false,
            });
            let marker = if sigil == ':' {
                SpanKind::ProjectTaskMoveLinkMarker
            } else {
                SpanKind::ProjectTaskMoveMarker
            };
            push_span(spans, span.start, span.end, marker);
            push_span(
                spans,
                span.start,
                span.end,
                SpanKind::InteractivePlaceholder,
            );
        }
        MoveBulletHit::Complete {
            sigil,
            id,
            sigil_span,
            id_span,
            ..
        } => {
            let range = (sigil_span.0, id_span.1);
            if let Some(new_id) = new_id
                && sub_bullets
                    .get(index + 1)
                    .is_some_and(|next| next.depth == AuthoredDepth::Nested)
            {
                let basename =
                    crate::native::capture_project_note::project_note_basename(
                        route, new_id,
                    );
                let stem = basename.strip_suffix(".md").unwrap_or(&basename);
                diagnostics.push(Diagnostic {
                    severity: Severity::Error,
                    code: "move_bullet_nested",
                    message: super::move_bullet::move_bullet_nested_error(
                        stem, &id,
                    ),
                    range: Some(range),
                    rewrite: None,
                });
                return;
            }
            match pass.register_move(sigil, &id, line_number) {
                MoveIdOutcome::Accept => {
                    sub_bullets[index].body.clear();
                    sub_bullets[index].move_task = Some(MoveBullet {
                        sigil,
                        block_id: Some(id),
                        range,
                        via: super::model::MoveVia::Bullet,
                        minted: false,
                    });
                    let marker = if sigil == ':' {
                        SpanKind::ProjectTaskMoveLinkMarker
                    } else {
                        SpanKind::ProjectTaskMoveMarker
                    };
                    push_span(spans, sigil_span.0, sigil_span.1, marker);
                    push_span(
                        spans,
                        id_span.0,
                        id_span.1,
                        SpanKind::ProjectTaskMoveBlockId,
                    );
                }
                MoveIdOutcome::Duplicate { first_line } => {
                    diagnostics.push(Diagnostic {
                        severity: Severity::Error,
                        code: "move_bullet_duplicate",
                        message:
                            super::move_bullet::move_bullet_duplicate_error(
                                sigil, &id, first_line,
                            ),
                        range: Some(range),
                        rewrite: None,
                    });
                }
                MoveIdOutcome::Collision { authored_line } => {
                    diagnostics.push(Diagnostic {
                        severity: Severity::Error,
                        code: "move_bullet_id_collision",
                        message:
                            super::move_bullet::move_bullet_id_collision_error(
                                &id,
                                route,
                                line_number,
                                authored_line,
                            ),
                        range: Some(range),
                        rewrite: None,
                    });
                }
            }
        }
    }
}

fn list_affix_token<'a>(tokens: &[Token<'a>], source: &str) -> Option<&'a str> {
    for token in tokens {
        if crate::native::capture_links::is_protected_markup_range(
            source,
            token.start,
            token.end,
        ) {
            continue;
        }
        if token.text.starts_with('!')
            && token.text.contains(':')
            && !token.text.starts_with("!!")
            && !token.text.starts_with("![")
        {
            return Some(token.text);
        }
    }
    if let Some(token) = tokens.last()
        && let Some(query) = token.text.strip_prefix('+')
        && !query.starts_with('+')
        && !query.starts_with('-')
        && !query
            .as_bytes()
            .first()
            .is_some_and(|byte| byte.is_ascii_digit())
        && !crate::native::capture_links::is_protected_markup_range(
            source,
            token.start,
            token.end,
        )
    {
        return Some(token.text);
    }
    None
}

pub(super) fn parse_editor_item_inner<'a>(
    item: &CaptureItem<'a>,
    options: &EditorParseOptions<'_>,
    has_clip: bool,
    has_listen: bool,
    target_run: Option<&super::item::TargetRunContext>,
) -> EditorItemOutcome<'a> {
    // List bullets parse as their own task-bullet grammar: no whole-item
    // claims, no operator or picker forms.
    if item.list.is_some() {
        return parse_editor_list_item(item, options);
    }
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
        if let Some(error) = item
            .lines
            .first()
            .and_then(super::item::early_link_operator_body_diagnostic)
        {
            let mut adjustment = adjustment;
            adjustment.item.mode = EditorMode::PomodoroLink;
            adjustment.item.pomodoro_shift = None;
            adjustment.item.diagnostics = vec![Diagnostic {
                severity: Severity::Error,
                code: "invalid_pomodoro_link",
                message: error.message,
                range: Some(error.range),
                rewrite: None,
            }];
            return adjustment;
        }
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
    let mut parent_parse = parse_editor_line(
        parent_tokens,
        parent_line.text,
        parent_line.start,
        true,
        false,
    );
    extend_target_run_terminal_spans(
        &mut parent_parse.terminal_spans,
        target_run,
        parent_item_line.line_number,
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

    // Project-note task bullets take the list ID rule: the ID may sit
    // anywhere in the trailing marker run, so it leaves before marker
    // extraction and schedule/priority markers behind it still resolve.
    // The project-note marker may sit on a child line: a sealed marker
    // there routes the whole item, so every sibling takes the ID rule.
    let parent_takes_ids = matches!(
        mode,
        EditorMode::ProjectNote | EditorMode::PomodoroProjectNote
    ) || item.lines.iter().skip(1).any(|line| {
        matches!(
            classify_authored_line(line.raw),
            AuthoredLineClass::Item(authored)
                if child_line_seals_project_note(authored.body)
        )
    });
    let mut sub_bullets = Vec::new();
    let mut child_task_tokens: Vec<Option<Token<'_>>> = Vec::new();
    let mut child_task_stripped: Vec<Option<StrippedTaskId<'_>>> = Vec::new();
    let mut child_task_lines: Vec<usize> = Vec::new();
    let mut child_move_hits: Vec<Option<MoveBulletHit>> = Vec::new();
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
                    rewrite: None,
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
                rewrite: None,
            });
            continue;
        }

        let child_line = RawLine {
            text: authored.body,
            start: authored.body_start,
            end: raw.end,
        };
        let child_tokens = tokenize_line_with_spans(&child_line);
        let mut child_parse = parse_editor_line(
            child_tokens,
            child_line.text,
            child_line.start,
            false,
            parent_takes_ids,
        );
        extend_target_run_terminal_spans(
            &mut child_parse.terminal_spans,
            target_run,
            line_number,
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
        // Move bullets claim the line from its raw shape before the
        // trailing-ID rule runs; the project-note post-pass interprets
        // the hit and the line never reaches the new-task path.
        let move_hit = (parent_takes_ids
            && authored.depth == AuthoredDepth::First)
            .then(|| {
                super::move_bullet::scan_move_bullet(
                    authored.body,
                    authored.body_start,
                    item.source,
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
                rewrite: None,
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
                rewrite: None,
            });
        }

        // A stripped task ID defers to the project-note post-pass, which
        // reports the ID refusal with the execution code.
        let id_pending = child_parse.stripped_task_id.is_some();
        if child_parse.body.is_empty() && drawer_hit.is_none() && !id_pending {
            // A child holding only dependency modifiers contributes no
            // empty bullet: its prerequisites belong to the item's
            // dependent. Any other emptied child stays an error.
            if child_parse.dependencies.stripped_tokens == 0 {
                diagnostics.push(Diagnostic {
                    severity: Severity::Error,
                    code: "empty_child_after_markers",
                    message: empty_child_after_markers_error(line_number),
                    range: Some((raw.start, raw.end)),
                    rewrite: None,
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
            child_task_stripped.push(child_parse.stripped_task_id.clone());
            child_task_lines.push(line_number);
            child_move_hits.push(move_hit);
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
            // A stripped task ID defers to the project-note post-pass
            // without a diagnostic; everything else keeps the empty-child
            // refusal and stays out of the bullet list.
            let empty_bullet = child_body.is_empty() && drawer.is_none();
            if empty_bullet
                && !id_pending
                && child_parse.dependencies.stripped_tokens == 0
            {
                diagnostics.push(Diagnostic {
                    severity: Severity::Error,
                    code: "empty_child_after_markers",
                    message: empty_child_after_markers_error(line_number),
                    range: Some((raw.start, raw.end)),
                    rewrite: None,
                });
            }
            if !empty_bullet || id_pending {
                sub_bullets.push(AuthoredSubBullet {
                    body: child_body,
                    depth: authored.depth,
                    task_id: None,
                    move_task: None,
                    move_choice: None,
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
            rewrite: None,
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
        // A glued drawer capture with no parent note text needs its note
        // text, unless first-level bullets already supply the notes: each
        // one becomes its own note inside the drawer. Clipboard notes
        // supply the notes the same way.
        let has_first_level_note = sub_bullets
            .iter()
            .any(|bullet| bullet.depth == AuthoredDepth::First)
            || seen.clip;
        if has_first_level_note {
            needs.retain(|need| *need != Need::DrawerText);
        } else {
            if !needs.contains(&Need::DrawerText) {
                needs.push(Need::DrawerText);
            }
            if mode != EditorMode::Incomplete {
                // Keep SubBullet mode but mark incomplete via needs; the
                // editor shows the drawer menu and the missing-text hint.
                // Diagnostics stay empty so the menu can open.
            }
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
            rewrite: None,
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
                rewrite: None,
            });
        }
    }

    // A task body made entirely of session operators after a link start is
    // almost always a spaced timing spelling. Keep it editable as a link
    // item, but point the diagnostic at those operators and teach the glued
    // `+` spelling when it is unambiguous.
    if mode == EditorMode::PomodoroTask
        && let Some(marker_text) = parent_parse.marker_text.as_deref()
        && let Some(error) = super::item::pomodoro_link_operator_body_diagnostic(
            parent_line.text,
            marker_text,
            &parent_parse.body_tokens,
        )
    {
        diagnostics.push(Diagnostic {
            severity: Severity::Error,
            code: "invalid_pomodoro_link",
            message: error.message,
            range: Some(error.range),
            rewrite: None,
        });
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
                rewrite: None,
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
                rewrite: None,
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
                rewrite: None,
            });
        }
    }

    // Project task IDs (` :id` / ` ^id`) and move bullets (`- ^<id>` /
    // `- :<id>`) are a post-pass over the parsed lines, mirroring
    // execution's evaluation order through the shared `ProjectTaskPass`:
    // the parent line, then each child in source order, then the
    // unused-`#pomodoro` rule. Accepted IDs are stripped from the
    // sub-bullet bodies and recorded on them; accepted moves are recorded
    // with empty bodies and their own spans; violations become
    // diagnostics. A lone trailing sigil is an unfinished ID: `incomplete`
    // needing `block_id` with a placeholder span over the sigil and no
    // diagnostic. A lone move sigil is an unfinished move: `incomplete`
    // needing `project_task_move` with marker and placeholder spans over
    // the sigil and no diagnostic. A project note with at least one
    // accepted ` :` task or `:` move upgrades to `pomodoro_project_note`.
    //
    // Both sealed forms run this pass so `bob capture` and `capture-parse`
    // agree on every rule. The entry mode tells the create form from the
    // offer form for the unused-`#pomodoro` diagnostic, which execution
    // only reports for the create form.
    let is_create_note = mode == EditorMode::ProjectNote;
    if is_create_note || mode == EditorMode::PomodoroProjectNote {
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
                rewrite: None,
            });
        }
        let route_name = route.as_deref().unwrap_or_default();
        let new_id = block_id.as_deref();
        for index in 0..sub_bullets.len() {
            let token = child_task_tokens[index];
            let stripped = child_task_stripped.get(index).cloned().flatten();
            let line_number = child_task_lines[index];
            // The stripped token carries the span; it left the body before
            // marker extraction, so the body below is already clean.
            let token =
                stripped.as_ref().map(|stripped| stripped.token).or(token);
            // A claimed move bullet never reaches the new-task path: its
            // hit decides spans, needs, and diagnostics here. Without a
            // parent route the move cannot name its source, so the line
            // keeps the trailing-ID path exactly as before.
            if route.is_some()
                && let Some(hit) = child_move_hits[index].clone()
            {
                interpret_editor_move(
                    hit,
                    &mut pass,
                    &mut sub_bullets,
                    &mut spans,
                    &mut needs,
                    &mut mode,
                    &mut diagnostics,
                    &mut has_pending_colon_link,
                    index,
                    line_number,
                    route_name,
                    new_id,
                );
                continue;
            }
            match pass.check_child(
                stripped,
                &sub_bullets[index].body,
                sub_bullets[index].depth,
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
                ChildTaskOutcome::Accept { sigil, id } => {
                    if let Some(move_line) = pass.move_line_for(&id) {
                        diagnostics.push(Diagnostic {
                            severity: Severity::Error,
                            code: "move_bullet_id_collision",
                            message:
                                super::move_bullet::move_bullet_id_collision_error(
                                    &id,
                                    route_name,
                                    move_line,
                                    line_number,
                                ),
                            range: token
                                .map(|token| (token.start, token.end)),
                            rewrite: None,
                        });
                        continue;
                    }
                    let link = sigil == ':';
                    sub_bullets[index].task_id =
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
                        rewrite: None,
                    });
                }
            }
        }
        // A `#pomodoro` name picks the Pomodoro that ` :<id>` Task Links
        // go under. The diagnostic covers the `#name` component. An
        // unfinished ` :` ID or pending `- :` move counts as a link (a
        // lone `^` does not, per Rule 7). Only the create form reports it
        // at parse time; the offer form resolves offers during planning.
        if is_create_note
            && section.is_some()
            && !pass.has_link
            && !has_pending_colon_link
        {
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
                rewrite: None,
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
                    let trailing = parent_line
                        .text
                        .get(token_end.saturating_sub(parent_line.start)..)
                        .unwrap_or_default()
                        .trim();
                    let operator_error =
                        super::item::pomodoro_link_operator_body_error(
                            parent_line.text,
                            trailing,
                            caret.token.text,
                        );
                    let (message, conflict_range) = match operator_error {
                        Some(operator_error) => {
                            let start = parent_line
                                .text
                                .find(trailing)
                                .map(|start| parent_line.start + start)
                                .unwrap_or(token_start);
                            (
                                operator_error,
                                Some((start, start + trailing.len())),
                            )
                        }
                        None => (message, Some((token_start, token_end))),
                    };
                    diagnostics.push(Diagnostic {
                        severity: Severity::Error,
                        code: "invalid_pomodoro_link",
                        message,
                        range: conflict_range,
                        rewrite: None,
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
                        rewrite: None,
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
                        rewrite: None,
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
                        rewrite: None,
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
                    rewrite: None,
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
                rewrite: None,
            });
        }
        for invalid in item_dependencies.relation_invalid.iter() {
            diagnostics.push(Diagnostic {
                severity: Severity::Error,
                code: "invalid_relation",
                message: invalid.message.clone(),
                range: Some((invalid.start, invalid.end)),
                rewrite: None,
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
        // A note-less owner with first-level bullets is the multi-note
        // form: the bullets are notes, so the text diagnostic only applies
        // when item-wide markers are also present.
        let multi_note_form = !seen.schedule
            && !seen.priority
            && !seen.clip
            && sub_bullets
                .iter()
                .any(|bullet| bullet.depth == AuthoredDepth::First);
        if matches!(
            dependency_target.as_ref().map(|target| &target.kind),
            Some(DependencyTargetKind::ExistingTask)
        ) && body.is_empty()
            && (seen.schedule
                || seen.priority
                || seen.clip
                || (!sub_bullets.is_empty() && !multi_note_form))
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
                rewrite: None,
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
                rewrite: None,
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
                rewrite: None,
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
                rewrite: None,
            });
        }
    }

    // A bare `- #` rewrites the mode to `Incomplete`, so the bullet
    // target is classified from the mode on entry.
    let entry_mode = mode;
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
    let drawer_bullet_target = super::drawer_bullet::classify_target(
        entry_mode,
        route.as_deref(),
        block_id.as_deref(),
        &body,
        section.as_deref(),
        super::drawer_bullet::item_has_first_level_line(item),
        spans
            .iter()
            .any(|span| span.kind == SpanKind::TaskToggleExplicitToggle),
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
            pomodoro_split: None,
            pomodoro_merge: None,
            pomodoro_close,
            pomodoro_past: None,
            dependencies: item_dependencies.dependency_entries(),
            blocked_tasks: item_dependencies.blocked_task_entries(),
            relations: item_dependencies.relation_entries(),
            dependency_target,
            task_complete: None,
            list: None,
            spans,
            diagnostics,
            sub_bullets,
            has_local_destination,
            local_destination_markers,
            drawer_bullet_target,
            targets: Vec::new(),
            merge_in: None,
        },
        declarations,
    }
}

fn extend_target_run_terminal_spans(
    spans: &mut Vec<Span>,
    target_run: Option<&super::item::TargetRunContext>,
    line_number: usize,
) {
    let Some(target_run) = target_run.filter(|run| run.line == line_number)
    else {
        return;
    };
    for (kind, start, end) in &target_run.terminal_spans {
        let span = Span {
            start: *start,
            end: *end,
            kind: *kind,
            project_note: None,
            title: None,
            key: None,
        };
        if !spans.contains(&span) {
            spans.push(span);
        }
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
    let found = draft.items.iter().find(|item| {
        item.lines
            .iter()
            .any(|line| cursor >= line.raw.start && cursor <= line.raw.end)
    });
    // A cursor or diagnostic on a header line belongs to the list's first
    // item: per-bullet items hold zero-copy bullet slices, so the header
    // line itself sits in no item's range.
    let index = match found {
        Some(item) => item.index,
        None => {
            let header =
                split_physical_lines(raw_text).into_iter().find(|line| {
                    cursor >= line.start
                        && cursor <= line.end
                        && detect_list_header(line).is_some()
                })?;
            draft
                .items
                .iter()
                .find(|item| {
                    item.list.as_ref().is_some_and(|list| {
                        !list.ownerless
                            && list.header_start == header.start
                            && list.header_end == header.end
                    })
                })?
                .index
        }
    };
    parse_for_editor(raw_text)
        .items
        .into_iter()
        .find(|item| item.index == index)
}
