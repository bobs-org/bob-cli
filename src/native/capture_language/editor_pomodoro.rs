//! Special Pomodoro item parsing for the editor view.

use super::close_selection::*;
use super::editor_model::*;
use super::editor_parse::*;
use super::item::*;
use super::markers::*;
use super::model::*;
use super::tokens::*;

/// Whole-item session operator for the live editor: one sign resizes,
/// two signs shift. Mirrors [`parse_pomodoro_adjust_item`]'s execution
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

/// Whole-item `=`-family parser for the live editor. Mirrors
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
        } => {
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
    let selection_after_x = whole_item_close_after_x(first);
    if selection_after_x.is_none() && !first.eq_ignore_ascii_case("=x") {
        return None;
    }
    let leading = parent_text.len() - parent_text.trim_start().len();
    let token_start = parent.raw.start + leading;
    let token_end = token_start + 2;
    let close_span = Span {
        start: token_start,
        end: token_end,
        kind: SpanKind::PomodoroClose,
    };
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
    // text is the precise range.
    let rest_start_in_trimmed =
        parent_trimmed.find(first).expect("first token") + first.len();
    let rest = parent_trimmed[rest_start_in_trimmed..].trim_start();
    let rest_offset = parent_text
        .find(rest)
        .unwrap_or(token_end - parent.raw.start);
    let range = if rest.is_empty() {
        (item.start, item.end)
    } else {
        let rest_start = parent.raw.start + rest_offset;
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
