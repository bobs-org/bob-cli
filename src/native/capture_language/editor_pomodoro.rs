//! Special Pomodoro item parsing for the editor view.

use super::close_selection::*;
use super::editor_model::*;
use super::editor_parse::*;
use super::item::*;
use super::markers::*;
use super::model::*;
use super::tokens::*;

/// Whole-item session operator for the live editor: one sign resizes,
/// two signs shift. A same-line chain has already been split into
/// single-token items upstream in `draft.rs`. Mirrors
/// [`parse_pomodoro_adjust_item`]'s execution
/// grammar but never fails: an exact operator reports `pomodoro_adjust`
/// or `pomodoro_shift` with a span covering only the token and an additive
/// spec (a bare sign is one unit, never incomplete), and zero/overflow/
/// shape near misses report the family mode plus an
/// `invalid_pomodoro_adjustment` / `invalid_pomodoro_shift` diagnostic
/// instead of becoming a task. Purely lexical: it never guesses current
/// ledger times.
pub(super) fn parse_editor_adjust_item<'a>(
    item: &CaptureItem<'a>,
) -> Option<EditorItemOutcome<'a>> {
    let parent = item.lines.first().expect("nonempty item");
    let parent_text = parent.raw.text;
    let parent_trimmed = parent_text.trim();
    let (operator, digits, prefix_len) =
        session_operator_token(parent_trimmed)?;
    let leading = parent_text.len() - parent_text.trim_start().len();
    let token_start = parent.raw.start + leading;
    let token_end = token_start + prefix_len;
    let (mode, span_kind, shape_message, overflow_message, zero_message, code) =
        match operator {
            SessionOperator::Resize { .. } => (
                EditorMode::PomodoroAdjust,
                SpanKind::PomodoroAdjust,
                POMODORO_ADJUST_SHAPE_ERROR,
                POMODORO_ADJUST_OVERFLOW_ERROR,
                POMODORO_ADJUST_ZERO_ERROR,
                "invalid_pomodoro_adjustment",
            ),
            SessionOperator::Shift { .. } => (
                EditorMode::PomodoroShift,
                SpanKind::PomodoroShift,
                POMODORO_SHIFT_SHAPE_ERROR,
                POMODORO_SHIFT_OVERFLOW_ERROR,
                POMODORO_SHIFT_ZERO_ERROR,
                "invalid_pomodoro_shift",
            ),
        };
    let span = Span {
        start: token_start,
        end: token_end,
        kind: span_kind,
    };
    let exact = parent_trimmed.len() == prefix_len && item.lines.len() == 1;
    if !exact {
        if digits.is_empty()
            && !(parent_trimmed.len() == prefix_len && item.lines.len() > 1)
        {
            return None;
        }
        return Some(editor_operator_outcome(
            item,
            parent_trimmed,
            mode,
            span,
            code,
            shape_message,
            Some((item.start, item.end)),
        ));
    }
    let units = if digits.is_empty() {
        1
    } else {
        match digits.parse::<u64>() {
            Ok(units) => units,
            Err(_) => {
                return Some(editor_operator_outcome(
                    item,
                    parent_trimmed,
                    mode,
                    span,
                    code,
                    overflow_message,
                    Some((token_start, token_end)),
                ));
            }
        }
    };
    if units == 0 {
        return Some(editor_operator_outcome(
            item,
            parent_trimmed,
            mode,
            span,
            code,
            zero_message,
            Some((token_start, token_end)),
        ));
    }
    let raw = parent_trimmed.to_string();
    let (pomodoro_adjust, pomodoro_shift) = match operator {
        SessionOperator::Resize { plus } => (
            Some(PomodoroAdjustSpec {
                raw: raw.clone(),
                plus,
                units,
            }),
            None,
        ),
        SessionOperator::Shift { later } => (
            None,
            Some(PomodoroShiftSpec {
                raw: raw.clone(),
                later,
                units,
            }),
        ),
    };
    Some(EditorItemOutcome {
        item: EditorItemParse {
            index: item.index,
            start: item.start,
            end: item.end,
            line_start: item.line_start,
            line_end: item.line_end,
            body: raw,
            mode,
            route: None,
            section: None,
            block_id: None,
            needs: Vec::new(),
            pomodoro_start: None,
            pomodoro_adjust,
            pomodoro_shift,
            pomodoro_close: None,
            spans: vec![span],
            diagnostics: Vec::new(),
            sub_bullets: Vec::new(),
            has_local_destination: false,
            local_destination_markers: Vec::new(),
        },
        declarations: Vec::new(),
    })
}

/// Build an operator near-miss outcome: the family mode, the token span,
/// and one diagnostic, with no spec.
pub(super) fn editor_operator_outcome<'a>(
    item: &CaptureItem<'a>,
    parent_trimmed: &str,
    mode: EditorMode,
    span: Span,
    code: &'static str,
    message: &str,
    range: Option<(usize, usize)>,
) -> EditorItemOutcome<'a> {
    EditorItemOutcome {
        item: EditorItemParse {
            index: item.index,
            start: item.start,
            end: item.end,
            line_start: item.line_start,
            line_end: item.line_end,
            body: parent_trimmed.to_string(),
            mode,
            route: None,
            section: None,
            block_id: None,
            needs: Vec::new(),
            pomodoro_start: None,
            pomodoro_adjust: None,
            pomodoro_shift: None,
            pomodoro_close: None,
            spans: vec![span],
            diagnostics: vec![Diagnostic {
                severity: Severity::Error,
                code,
                message: message.to_string(),
                range,
            }],
            sub_bullets: Vec::new(),
            has_local_destination: false,
            local_destination_markers: Vec::new(),
        },
        declarations: Vec::new(),
    }
}

/// Build a whole-item start near-miss outcome: `pomodoro_start` mode, the
/// token span, and one `invalid_pomodoro_start` diagnostic, with no spec.
pub(super) fn editor_start_invalid_outcome<'a>(
    item: &CaptureItem<'a>,
    parent_trimmed: &str,
    span: Span,
    message: String,
    range: Option<(usize, usize)>,
) -> EditorItemOutcome<'a> {
    EditorItemOutcome {
        item: EditorItemParse {
            index: item.index,
            start: item.start,
            end: item.end,
            line_start: item.line_start,
            line_end: item.line_end,
            body: parent_trimmed.to_string(),
            mode: EditorMode::PomodoroStart,
            route: None,
            section: None,
            block_id: None,
            needs: Vec::new(),
            pomodoro_start: None,
            pomodoro_adjust: None,
            pomodoro_shift: None,
            pomodoro_close: None,
            spans: vec![span],
            diagnostics: vec![Diagnostic {
                severity: Severity::Error,
                code: "invalid_pomodoro_start",
                message,
                range,
            }],
            sub_bullets: Vec::new(),
            has_local_destination: false,
            local_destination_markers: Vec::new(),
        },
        declarations: Vec::new(),
    }
}

/// Whole-item `=<X>#name` named start for the live editor. Mirrors the
/// named-start branch of [`parse_pomodoro_equals_item`] but never fails:
/// an exact token reports `pomodoro_start` with its spec, `section` set to
/// the typed selector, a `pomodoro_start` span over `=<X>` plus a
/// `pomodoro_name` span over the name bytes only (the `#` is in no span),
/// while an empty name reports `incomplete` needing `pomodoro_name` with
/// the partial spec and an `interactive_placeholder` span over `#`. A near
/// miss reports `pomodoro_start` plus one `invalid_pomodoro_start`
/// diagnostic reusing the execution texts: E2/E3 over the name, E4 over
/// the extra text or child line, overflow over `=<X>`. Purely lexical: it
/// never guesses current ledger times.
pub(super) fn parse_editor_named_start_item<'a>(
    item: &CaptureItem<'a>,
    parent_trimmed: &str,
    suffix: String,
    name: String,
    len: usize,
) -> Option<EditorItemOutcome<'a>> {
    let parent = item.lines.first().expect("nonempty item");
    let leading = parent.raw.text.len() - parent.raw.text.trim_start().len();
    let token_start = parent.raw.start + leading;
    let prefix_len = 1 + suffix.len();
    let hash_start = token_start + prefix_len;
    let name_start = hash_start + 1;
    let name_end = name_start + name.len();
    let mut spans = vec![Span {
        start: token_start,
        end: token_start + prefix_len,
        kind: SpanKind::PomodoroStart,
    }];
    if name.is_empty() {
        spans.push(Span {
            start: hash_start,
            end: hash_start + 1,
            kind: SpanKind::InteractivePlaceholder,
        });
    } else {
        spans.push(Span {
            start: name_start,
            end: name_end,
            kind: SpanKind::PomodoroName,
        });
    }
    let token_text = parent_trimmed.get(..len).unwrap_or(parent_trimmed);
    let exact = parent_trimmed.len() == len && item.lines.len() == 1;
    if exact {
        if name.is_empty() {
            // `=<X>#` is an editing state, never a mistake: the partial
            // spec, the spans typed so far, and no diagnostic.
            return Some(editor_named_incomplete_outcome(
                item,
                parent_trimmed,
                spans,
                parse_pomodoro_start_suffix(&suffix).ok(),
            ));
        }
        if let Some((message, range)) = named_start_token_diagnostic(
            token_text,
            &suffix,
            &name,
            token_start,
            prefix_len,
            name_start,
            name_end,
        ) {
            return Some(editor_named_invalid_outcome(
                item,
                parent_trimmed,
                spans,
                message,
                Some(range),
            ));
        }
        let spec = parse_pomodoro_start_suffix(&suffix)
            .expect("named token checked before spec");
        return Some(EditorItemOutcome {
            item: EditorItemParse {
                index: item.index,
                start: item.start,
                end: item.end,
                line_start: item.line_start,
                line_end: item.line_end,
                body: parent_trimmed.to_string(),
                mode: EditorMode::PomodoroStart,
                route: None,
                section: Some(name),
                block_id: None,
                needs: Vec::new(),
                pomodoro_start: Some(spec),
                pomodoro_adjust: None,
                pomodoro_shift: None,
                pomodoro_close: None,
                spans,
                diagnostics: Vec::new(),
                sub_bullets: Vec::new(),
                has_local_destination: false,
                local_destination_markers: Vec::new(),
            },
            declarations: Vec::new(),
        });
    }
    // Extra text or child lines: a broken token reports its own diagnostic
    // first (E1 through E3, or overflow), exactly like execution.
    let has_extra = parent_trimmed.len() > len;
    if name.is_empty()
        && has_extra
        && let Some(message) = named_nospace_error(&suffix, parent_trimmed, len)
    {
        return Some(editor_named_invalid_outcome(
            item,
            parent_trimmed,
            spans,
            message,
            Some(extra_text_range(token_start, parent_trimmed, len)),
        ));
    }
    if let Some((message, range)) = named_start_token_diagnostic(
        token_text,
        &suffix,
        &name,
        token_start,
        prefix_len,
        name_start,
        name_end,
    ) {
        let range = if name.is_empty() && !has_extra {
            // `=<X>#` with child lines only: the child line is the range,
            // matching the unnamed start's child-line policy.
            let child = &item.lines[1];
            Some((child.raw.start, child.raw.end))
        } else {
            Some(range)
        };
        return Some(editor_named_invalid_outcome(
            item,
            parent_trimmed,
            spans,
            message,
            range,
        ));
    }
    // A well-formed token with extra text or child lines: E4. An empty
    // name here means child lines only (extra text took the no-space
    // branch above).
    if name.is_empty() {
        let child = &item.lines[1];
        return Some(editor_named_invalid_outcome(
            item,
            parent_trimmed,
            spans,
            pomodoro_named_start_incomplete_error(token_text),
            Some((child.raw.start, child.raw.end)),
        ));
    }
    let message =
        named_shape_error(token_text, &suffix, &name, parent_trimmed, len);
    let range = if has_extra {
        extra_text_range(token_start, parent_trimmed, len)
    } else {
        let child = &item.lines[1];
        (child.raw.start, child.raw.end)
    };
    Some(editor_named_invalid_outcome(
        item,
        parent_trimmed,
        spans,
        message,
        Some(range),
    ))
}

/// Classify a named start token's own error the way execution's
/// `named_token_error` does (E1/E3/E2/overflow order), paired with the
/// editor range for that error: E1 over the `#`, E2/E3 over the name,
/// overflow over `=<X>`. `None` when the token itself is well-formed.
fn named_start_token_diagnostic(
    token: &str,
    suffix: &str,
    name: &str,
    token_start: usize,
    prefix_len: usize,
    name_start: usize,
    name_end: usize,
) -> Option<(String, (usize, usize))> {
    if name.is_empty() {
        return Some((
            pomodoro_named_start_incomplete_error(token),
            (token_start + prefix_len, token_start + prefix_len + 1),
        ));
    }
    if let Some((before, after)) = name.split_once('=')
        && suffix.is_empty()
        && parse_pomodoro_start_suffix(after).is_ok()
    {
        return Some((
            pomodoro_named_start_order_error(token, before, after),
            (name_start, name_end),
        ));
    }
    if !is_pomodoro_selector_component(name) {
        return Some((
            pomodoro_named_start_name_error(name, token),
            (name_start, name_end),
        ));
    }
    if let Err(message) = parse_pomodoro_start_suffix(suffix) {
        return Some((message, (token_start, token_start + prefix_len)));
    }
    None
}

/// Range of the trimmed extra text after a whole-item token, mirroring the
/// unnamed start's extra-text policy.
fn extra_text_range(
    token_start: usize,
    parent_trimmed: &str,
    len: usize,
) -> (usize, usize) {
    let rest_in_trimmed = &parent_trimmed[len..];
    let rest_trimmed = rest_in_trimmed.trim_start();
    let offset_in_trimmed = len + (rest_in_trimmed.len() - rest_trimmed.len());
    let rest_start = token_start + offset_in_trimmed;
    (rest_start, rest_start + rest_trimmed.len())
}

/// Build an `incomplete` named-start outcome: the spans typed so far, the
/// partial spec, and a `pomodoro_name` need, with no diagnostic.
fn editor_named_incomplete_outcome<'a>(
    item: &CaptureItem<'a>,
    parent_trimmed: &str,
    spans: Vec<Span>,
    pomodoro_start: Option<PomodoroStartSpec>,
) -> EditorItemOutcome<'a> {
    EditorItemOutcome {
        item: EditorItemParse {
            index: item.index,
            start: item.start,
            end: item.end,
            line_start: item.line_start,
            line_end: item.line_end,
            body: parent_trimmed.to_string(),
            mode: EditorMode::Incomplete,
            route: None,
            section: None,
            block_id: None,
            needs: vec![Need::PomodoroName],
            pomodoro_start,
            pomodoro_adjust: None,
            pomodoro_shift: None,
            pomodoro_close: None,
            spans,
            diagnostics: Vec::new(),
            sub_bullets: Vec::new(),
            has_local_destination: false,
            local_destination_markers: Vec::new(),
        },
        declarations: Vec::new(),
    }
}

/// Build a named-start near-miss outcome: `pomodoro_start` mode, the spans
/// typed so far, and one `invalid_pomodoro_start` diagnostic, with no spec.
fn editor_named_invalid_outcome<'a>(
    item: &CaptureItem<'a>,
    parent_trimmed: &str,
    spans: Vec<Span>,
    message: String,
    range: Option<(usize, usize)>,
) -> EditorItemOutcome<'a> {
    let mut outcome = editor_start_invalid_outcome(
        item,
        parent_trimmed,
        spans[0],
        message,
        range,
    );
    outcome.item.spans = spans;
    outcome
}

/// Whole-item `=`/`=<X>` start for the live editor. Mirrors the start
/// branch of [`parse_pomodoro_equals_item`] but never fails: an exact
/// single-token start reports `pomodoro_start` with its spec and one span
/// over the whole token, while a counted token with extra text (or an
/// exact token with child lines) reports `pomodoro_start` plus an
/// `invalid_pomodoro_start` diagnostic. Bare tokens with prose stay prose.
/// Range policy matches `invalid_pomodoro_close`: the extra text for
/// trailing text, the child line for child-line misses, the token for
/// overflow.
pub(super) fn parse_editor_start_item<'a>(
    item: &CaptureItem<'a>,
    parent_trimmed: &str,
    suffix: String,
    counted: bool,
    len: usize,
) -> Option<EditorItemOutcome<'a>> {
    let parent = item.lines.first().expect("nonempty item");
    let leading = parent.raw.text.len() - parent.raw.text.trim_start().len();
    let token_start = parent.raw.start + leading;
    let token_end = token_start + len;
    let span = Span {
        start: token_start,
        end: token_end,
        kind: SpanKind::PomodoroStart,
    };
    let exact = parent_trimmed.len() == len && item.lines.len() == 1;
    if !exact {
        if counted || (parent_trimmed.len() == len && item.lines.len() > 1) {
            let token_text = &parent_trimmed[..len];
            let message = pomodoro_start_shape_error(token_text, &suffix);
            // Exact token with child lines: the child line is the range.
            if parent_trimmed.len() == len {
                let child = &item.lines[1];
                return Some(editor_start_invalid_outcome(
                    item,
                    parent_trimmed,
                    span,
                    message,
                    Some((child.raw.start, child.raw.end)),
                ));
            }
            // Counted token with trailing text: the extra text is the
            // range, mirroring the close's extra-text policy.
            let rest_in_trimmed = &parent_trimmed[len..];
            let rest_trimmed = rest_in_trimmed.trim_start();
            if rest_trimmed.is_empty() {
                return Some(editor_start_invalid_outcome(
                    item,
                    parent_trimmed,
                    span,
                    message,
                    Some((item.start, item.end)),
                ));
            }
            let offset_in_trimmed =
                len + (rest_in_trimmed.len() - rest_trimmed.len());
            let rest_start = token_start + offset_in_trimmed;
            return Some(editor_start_invalid_outcome(
                item,
                parent_trimmed,
                span,
                message,
                Some((rest_start, rest_start + rest_trimmed.len())),
            ));
        }
        return None;
    }
    match parse_pomodoro_start_suffix(&suffix) {
        Ok(spec) => Some(EditorItemOutcome {
            item: EditorItemParse {
                index: item.index,
                start: item.start,
                end: item.end,
                line_start: item.line_start,
                line_end: item.line_end,
                body: parent_trimmed.to_string(),
                mode: EditorMode::PomodoroStart,
                route: None,
                section: None,
                block_id: None,
                needs: Vec::new(),
                pomodoro_start: Some(spec),
                pomodoro_adjust: None,
                pomodoro_shift: None,
                pomodoro_close: None,
                spans: vec![span],
                diagnostics: Vec::new(),
                sub_bullets: Vec::new(),
                has_local_destination: false,
                local_destination_markers: Vec::new(),
            },
            declarations: Vec::new(),
        }),
        Err(message) => Some(editor_start_invalid_outcome(
            item,
            parent_trimmed,
            span,
            message,
            Some((token_start, token_end)),
        )),
    }
}

/// Whole-item `=`-family parser for the live editor. A same-line chain has
/// already been split into single-token items upstream in `draft.rs`.
/// Mirrors
/// [`parse_pomodoro_equals_item`]'s execution grammar but never fails: an
/// exact `=x[<N>][!<M>]` reports `pomodoro_close` with spans covering `=x`
/// plus each typed list and an additive spec, an exact `=`/`=<X>` reports
/// `pomodoro_start` with its spec and one `PomodoroStart` span over the
/// whole token, a dangling separator reports `incomplete` needing
/// `pomodoro_close_task` with the partial spec and a placeholder span, and
/// every near miss reports its family mode plus an `invalid_pomodoro_*`
/// diagnostic instead of becoming a task. A bare `=` is a complete start.
/// Purely lexical: it never guesses current ledger times. Bare tokens with
/// prose (`= foo`, `==`), other close shapes (`=xx`, `=xa`), and mid-body
/// tokens (`Plan =3`, `a=3`, `Plan =x1`) stay ordinary prose.
pub(super) fn parse_editor_close_item<'a>(
    item: &CaptureItem<'a>,
) -> Option<EditorItemOutcome<'a>> {
    let parent = item.lines.first().expect("nonempty item");
    let parent_text = parent.raw.text;
    let parent_trimmed = parent_text.trim();
    let token = session_equals_token(parent_trimmed)?;
    match token {
        EqualsToken::Start {
            suffix,
            counted,
            len,
            name,
        } => {
            if let Some(selector) = name {
                return parse_editor_named_start_item(
                    item,
                    parent_trimmed,
                    suffix,
                    selector,
                    len,
                );
            }
            return parse_editor_start_item(
                item,
                parent_trimmed,
                suffix,
                counted,
                len,
            );
        }
        EqualsToken::Close => {}
    }
    let first = parent_trimmed.split_whitespace().next()?;
    let leading = parent_text.len() - parent_text.trim_start().len();
    let token_start = parent.raw.start + leading;
    let token_end = token_start + 2;
    let close_span = Span {
        start: token_start,
        end: token_end,
        kind: SpanKind::PomodoroClose,
    };
    // `=x#…` is a claimed close near miss with a teaching error, whether
    // or not the item carries extra text or child lines. It used to be
    // prose.
    if is_close_hash_token(first) {
        let hash = first.find('#').expect("close hash");
        return Some(editor_close_outcome(
            item,
            parent_trimmed,
            EditorMode::PomodoroClose,
            None,
            vec![close_span],
            Vec::new(),
            vec![Diagnostic {
                severity: Severity::Error,
                code: "invalid_pomodoro_close",
                message: pomodoro_close_hash_error(&first[hash + 1..]),
                range: Some((token_start + 2, token_start + first.len())),
            }],
        ));
    }
    let selection_after_x = whole_item_close_after_x(first);
    if selection_after_x.is_none() && !first.eq_ignore_ascii_case("=x") {
        return None;
    }
    let single_token_parent = parent_trimmed == first;
    if single_token_parent && item.lines.len() == 1 {
        // An exact close: plain `=x`, a valid selection, or a dangling
        // separator (an editing state, never a mistake).
        let lexed = selection_after_x.map(|after_x| {
            let base = token_start + (first.len() - after_x.len());
            lex_close_selection(after_x, base, first)
        });
        match lexed {
            None => {
                return Some(editor_close_outcome(
                    item,
                    parent_trimmed,
                    EditorMode::PomodoroClose,
                    Some(PomodoroCloseSpec::plain(first.to_string())),
                    vec![close_span],
                    Vec::new(),
                    Vec::new(),
                ));
            }
            Some(Ok(CloseSelectionOutcome::Valid(lex))) => {
                let mut spans = vec![close_span];
                if let Some((start, end)) = lex.in_progress_range {
                    spans.push(Span {
                        start,
                        end,
                        kind: SpanKind::PomodoroCloseInProgress,
                    });
                }
                if let Some((start, end)) = lex.complete_range {
                    spans.push(Span {
                        start,
                        end,
                        kind: SpanKind::PomodoroCloseComplete,
                    });
                }
                return Some(editor_close_outcome(
                    item,
                    parent_trimmed,
                    EditorMode::PomodoroClose,
                    Some(close_spec_from_lex(first.to_string(), &lex)),
                    spans,
                    Vec::new(),
                    Vec::new(),
                ));
            }
            Some(Ok(CloseSelectionOutcome::Incomplete(incomplete))) => {
                let mut spans = vec![close_span];
                if let Some((start, end)) = incomplete.in_progress_range {
                    spans.push(Span {
                        start,
                        end,
                        kind: SpanKind::PomodoroCloseInProgress,
                    });
                }
                if let Some((start, end)) = incomplete.complete_range {
                    spans.push(Span {
                        start,
                        end,
                        kind: SpanKind::PomodoroCloseComplete,
                    });
                }
                spans.push(Span {
                    start: incomplete.separator_range.0,
                    end: incomplete.separator_range.1,
                    kind: SpanKind::InteractivePlaceholder,
                });
                return Some(editor_close_outcome(
                    item,
                    parent_trimmed,
                    EditorMode::Incomplete,
                    Some(close_spec_from_incomplete(
                        first.to_string(),
                        &incomplete,
                    )),
                    spans,
                    vec![Need::PomodoroCloseTask],
                    Vec::new(),
                ));
            }
            Some(Err(error)) => {
                return Some(editor_close_outcome(
                    item,
                    parent_trimmed,
                    EditorMode::PomodoroClose,
                    None,
                    vec![close_span],
                    Vec::new(),
                    vec![Diagnostic {
                        severity: Severity::Error,
                        code: "invalid_pomodoro_close",
                        message: error.message,
                        range: Some(error.range),
                    }],
                ));
            }
        }
    }
    if single_token_parent {
        // A single-token parent with authored child lines: a broken list
        // reports its own diagnostic, anything else the shape error on the
        // child line.
        if let Some(after_x) = selection_after_x {
            let base = token_start + (first.len() - after_x.len());
            if let Err(error) = lex_close_selection(after_x, base, first) {
                return Some(editor_close_outcome(
                    item,
                    parent_trimmed,
                    EditorMode::PomodoroClose,
                    None,
                    vec![close_span],
                    Vec::new(),
                    vec![Diagnostic {
                        severity: Severity::Error,
                        code: "invalid_pomodoro_close",
                        message: error.message,
                        range: Some(error.range),
                    }],
                ));
            }
        }
        let child = &item.lines[1];
        return Some(editor_close_outcome(
            item,
            parent_trimmed,
            EditorMode::PomodoroClose,
            None,
            vec![close_span],
            Vec::new(),
            vec![Diagnostic {
                severity: Severity::Error,
                code: "invalid_pomodoro_close",
                message: POMODORO_CLOSE_SHAPE_ERROR.to_string(),
                range: Some((child.raw.start, child.raw.end)),
            }],
        ));
    }
    // Extra text on the parent line: a broken first token reports its own
    // diagnostic first, a spaceless join that forms a selection gets the
    // no-spaces hint, and anything else gets the shape error. The extra
    // text is the precise range, computed from the first token's end plus
    // skipped whitespace so it cannot land inside the close token itself.
    let after_first = &parent_trimmed[first.len()..];
    let rest = after_first.trim_start();
    let skipped = after_first.len() - rest.len();
    let range = if rest.is_empty() {
        (item.start, item.end)
    } else {
        let rest_start = token_start + first.len() + skipped;
        (rest_start, rest_start + rest.len())
    };
    if let Some(after_x) = selection_after_x {
        let base = token_start + (first.len() - after_x.len());
        if let Err(error) = lex_close_selection(after_x, base, first) {
            return Some(editor_close_outcome(
                item,
                parent_trimmed,
                EditorMode::PomodoroClose,
                None,
                vec![close_span],
                Vec::new(),
                vec![Diagnostic {
                    severity: Severity::Error,
                    code: "invalid_pomodoro_close",
                    message: error.message,
                    range: Some(error.range),
                }],
            ));
        }
    }
    let nospace: String = parent_trimmed.split_whitespace().collect();
    let message = if whole_item_close_after_x(&nospace).is_some_and(|after_x| {
        lex_close_selection(after_x, 0, &nospace).is_ok()
    }) {
        close_selection_no_spaces_error()
    } else {
        POMODORO_CLOSE_SHAPE_ERROR.to_string()
    };
    Some(editor_close_outcome(
        item,
        parent_trimmed,
        EditorMode::PomodoroClose,
        None,
        vec![close_span],
        Vec::new(),
        vec![Diagnostic {
            severity: Severity::Error,
            code: "invalid_pomodoro_close",
            message,
            range: Some(range),
        }],
    ))
}

/// Build a whole-item close outcome for the live editor: the mode, the
/// additive spec (or none for a near miss), the `=x`/list spans, the needs,
/// and at most one diagnostic.
#[allow(clippy::too_many_arguments)]
fn editor_close_outcome<'a>(
    item: &CaptureItem<'a>,
    parent_trimmed: &str,
    mode: EditorMode,
    pomodoro_close: Option<PomodoroCloseSpec>,
    spans: Vec<Span>,
    needs: Vec<Need>,
    diagnostics: Vec<Diagnostic>,
) -> EditorItemOutcome<'a> {
    EditorItemOutcome {
        item: EditorItemParse {
            index: item.index,
            start: item.start,
            end: item.end,
            line_start: item.line_start,
            line_end: item.line_end,
            body: parent_trimmed.to_string(),
            mode,
            route: None,
            section: None,
            block_id: None,
            needs,
            pomodoro_start: None,
            pomodoro_adjust: None,
            pomodoro_shift: None,
            pomodoro_close,
            spans,
            diagnostics,
            sub_bullets: Vec::new(),
            has_local_destination: false,
            local_destination_markers: Vec::new(),
        },
        declarations: Vec::new(),
    }
}
