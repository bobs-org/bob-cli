//! Editor token classification and diagnostics.

use super::close_selection::*;
use super::editor_model::*;
use super::line::*;
use super::markers::*;
use super::model::*;
use super::tokens::*;

/// A lexed `@<route>:<block-id>=x…` close suffix: either a valid selection
/// with its list spans, or a dangling separator with the partial spec and
/// the spans typed so far. Ranges are absolute byte offsets.
enum EditorCloseSuffix {
    Valid {
        in_progress: Option<(usize, usize)>,
        complete: Option<(usize, usize)>,
    },
    Incomplete {
        spec: PomodoroCloseSpec,
        in_progress: Option<(usize, usize)>,
        complete: Option<(usize, usize)>,
        separator: (usize, usize),
    },
}

/// Mirror `parse_capture_text_with_clip_control`'s precedence: the leading
/// token wins when `leading` is set (only ever true for the parent line),
/// and only a plain `@route` token needs body text on the other side before
/// it routes at all.
pub(super) fn select_marker_token(
    tokens: &[Token<'_>],
    leading: bool,
) -> Option<(usize, TokenParse)> {
    if leading
        && let Some(first) = tokens.first()
        && let Some(parse) = classify_editor_token(first)
    {
        let requires_body = matches!(
            &parse,
            TokenParse::Marker(marker) if marker.requires_body
        );
        if !(requires_body && tokens.len() == 1) {
            return Some((0, parse));
        }
    }

    if tokens.len() >= 2 {
        let last = tokens.len() - 1;
        if is_pomodoro_note_marker(tokens[last].text) {
            return Some((
                last,
                TokenParse::Marker(pomodoro_note_marker_parse(&tokens[last])),
            ));
        }
        if let Some(parse) = classify_editor_token(&tokens[last]) {
            return Some((last, parse));
        }
    }

    None
}

/// The bare `#` token never has a partially-typed form, needs nothing else,
/// and -- unlike every `@...` marker -- is only ever recognized as the final
/// token of a line, so it is classified directly in
/// [`select_marker_token`]'s trailing slot rather than through
/// [`classify_editor_token`] (which a leading check also consults).
pub(super) fn pomodoro_note_marker_parse(token: &Token<'_>) -> MarkerParse {
    MarkerParse {
        mode: EditorMode::PomodoroNote,
        route: None,
        section: None,
        block_id: None,
        needs: Vec::new(),
        spans: vec![Span {
            start: token.start,
            end: token.end,
            kind: SpanKind::PomodoroNote,
        }],
        requires_body: false,
        pomodoro_start: None,
        pomodoro_close: None,
    }
}

/// Classify one `@...` token, returning `None` when the token is not
/// route-shaped at all and therefore stays literal body text.
pub(super) fn classify_global_token(token: &Token<'_>) -> TokenParse {
    let rest = match token.text.strip_prefix("@@") {
        Some(rest) => rest,
        None => {
            return TokenParse::Invalid(token_diagnostic(
                token,
                "invalid_global_destination",
                GLOBAL_DESTINATION_SHAPE_ERROR,
            ));
        }
    };
    if rest.ends_with('!') {
        return TokenParse::Invalid(token_diagnostic(
            token,
            "unsupported_explicit_toggle",
            EXPLICIT_TOGGLE_GLOBAL_ERROR,
        ));
    }
    if rest.contains('#') || rest.contains('^') || rest.contains(':') {
        return TokenParse::Invalid(token_diagnostic(
            token,
            "invalid_global_destination",
            &unsupported_global_destination_error(token.text),
        ));
    }
    if let Some((route_part, block_part)) = rest.split_once('+') {
        if !route_part.is_empty() && !is_route_token(route_part) {
            return TokenParse::Invalid(token_diagnostic(
                token,
                "invalid_global_destination",
                GLOBAL_DESTINATION_ROUTE_ERROR,
            ));
        }
        if !block_part.is_empty() && !is_block_id(block_part) {
            return TokenParse::Invalid(token_diagnostic(
                token,
                "invalid_global_destination",
                GLOBAL_DESTINATION_BLOCK_ID_ERROR,
            ));
        }
        return TokenParse::Marker(marker_parse(
            token,
            MarkerShape {
                sigil_len: 2,
                route_part,
                separator_len: 1,
                right_part: block_part,
                route_kind: SpanKind::GlobalSubBulletRoute,
                right_kind: SpanKind::GlobalSubBulletBlockId,
                complete_mode: EditorMode::SubBullet,
                right_need: Need::Task,
                third: None,
                suffix: None,
            },
        ));
    }
    if rest.is_empty() {
        return TokenParse::Marker(MarkerParse {
            mode: EditorMode::Incomplete,
            route: None,
            section: None,
            block_id: None,
            needs: vec![Need::Route],
            spans: vec![Span {
                start: token.start,
                end: token.end,
                kind: SpanKind::InteractivePlaceholder,
            }],
            requires_body: false,
            pomodoro_start: None,
            pomodoro_close: None,
        });
    }
    if !is_route_token(rest) {
        return TokenParse::Invalid(token_diagnostic(
            token,
            "invalid_global_destination",
            GLOBAL_DESTINATION_ROUTE_ERROR,
        ));
    }
    TokenParse::Marker(MarkerParse {
        mode: EditorMode::Task,
        route: Some(rest.to_ascii_lowercase()),
        section: None,
        block_id: None,
        needs: Vec::new(),
        spans: vec![Span {
            start: token.start,
            end: token.end,
            kind: SpanKind::GlobalRoute,
        }],
        requires_body: false,
        pomodoro_start: None,
        pomodoro_close: None,
    })
}

pub(super) fn classify_editor_token(token: &Token<'_>) -> Option<TokenParse> {
    let text = token.text;
    if !text.starts_with('@') || text.starts_with("@@") {
        return None;
    }
    if is_sub_bullet_marker_candidate(text) {
        return Some(classify_sub_bullet_token(token));
    }
    if is_task_block_id_marker_candidate(text) {
        return Some(classify_task_block_id_token(token));
    }
    if is_retired_double_colon_marker_candidate(text) {
        return Some(classify_retired_double_colon_token(token));
    }
    if is_pomodoro_marker_candidate(text)
        || is_incomplete_pomodoro_marker_candidate(text)
    {
        return Some(classify_pomodoro_token(token));
    }
    classify_route_token(token).map(TokenParse::Marker)
}

/// `@:` and `@:<block-id>` never route in execution because the route is
/// still empty, so `is_pomodoro_marker_candidate` rejects them. They are
/// valid interactive states while the user is picking a target.
pub(super) fn is_incomplete_pomodoro_marker_candidate(token: &str) -> bool {
    if token.starts_with("@!") {
        return false;
    }
    token
        .strip_prefix('@')
        .is_some_and(|marker| marker.starts_with(':'))
}

pub(super) fn classify_sub_bullet_token(token: &Token<'_>) -> TokenParse {
    if let Some(message) = explicit_toggle_unsupported_message(token.text) {
        return TokenParse::Invalid(token_diagnostic(
            token,
            "unsupported_explicit_toggle",
            message,
        ));
    }
    let (token_text, explicit_toggle) =
        match exact_explicit_toggle_prefix(token.text) {
            Some(prefix) => (prefix, true),
            None => (token.text, false),
        };
    let marker = &token_text[1..];
    let (route_part, rest) =
        marker.split_once('+').expect("sub-bullet candidate");
    let (block_part, section_part) = match rest.split_once('#') {
        Some((block, section)) => (block, Some(section)),
        None => (rest, None),
    };

    if !route_part.is_empty() && !is_route_token(route_part) {
        return TokenParse::Invalid(token_diagnostic(
            token,
            "invalid_sub_bullet_route",
            SUB_BULLET_ROUTE_ERROR,
        ));
    }
    if !block_part.is_empty() && !is_block_id(block_part) {
        return TokenParse::Invalid(token_diagnostic(
            token,
            "invalid_sub_bullet_block_id",
            SUB_BULLET_BLOCK_ID_ERROR,
        ));
    }
    // The strict task-section charset (no `+`) is enforced later, once the
    // finished item shows whether this stays a sub-bullet capture or becomes
    // a task toggle -- whose trailing name is a Pomodoro name and takes the
    // wider Pomodoro charset instead. See `parse_editor_item`.
    if section_part.is_some_and(|section| {
        !section.is_empty() && !is_pomodoro_selector_component(section)
    }) {
        return TokenParse::Invalid(token_diagnostic(
            token,
            "invalid_sub_bullet_section",
            SUB_BULLET_SECTION_ERROR,
        ));
    }

    let (route_kind, right_kind, complete_mode, third_kind, third_need) =
        if explicit_toggle {
            (
                SpanKind::TaskToggleRoute,
                SpanKind::TaskToggleBlockId,
                EditorMode::TaskToggle,
                SpanKind::TaskTogglePomodoroName,
                Need::PomodoroName,
            )
        } else {
            (
                SpanKind::SubBulletRoute,
                SpanKind::SubBulletBlockId,
                EditorMode::SubBullet,
                SpanKind::SubBulletSection,
                Need::TaskSection,
            )
        };
    TokenParse::Marker(marker_parse(
        token,
        MarkerShape {
            sigil_len: 1,
            route_part,
            separator_len: 1,
            right_part: block_part,
            route_kind,
            right_kind,
            complete_mode,
            right_need: Need::Task,
            third: section_part.map(|part| MarkerThird {
                separator_len: 1,
                part,
                kind: third_kind,
                need: third_need,
            }),
            suffix: explicit_toggle.then_some(MarkerSuffix {
                len: 1,
                kind: SpanKind::TaskToggleExplicitToggle,
            }),
        },
    ))
}

pub(super) fn classify_task_block_id_token(token: &Token<'_>) -> TokenParse {
    let marker = &token.text[1..];
    let (route_part, block_part) =
        marker.split_once('^').expect("task block-ID candidate");

    if !route_part.is_empty() && !is_route_token(route_part) {
        return TokenParse::Invalid(token_diagnostic(
            token,
            "invalid_task_block_id_route",
            TASK_BLOCK_ID_ROUTE_ERROR,
        ));
    }
    // Mirror the execution grammar: a `+` before the `#` is a project-note
    // attempt that used the `^` family, which takes no Pomodoro name.
    if let Some((before_hash, _)) = block_part.split_once('#')
        && before_hash.ends_with('+')
    {
        return TokenParse::Invalid(token_diagnostic(
            token,
            "invalid_project_note_marker",
            PROJECT_NOTE_POMODORO_NAME_ERROR,
        ));
    }
    let (block_part, project_note) = match block_part.strip_suffix('+') {
        Some(stripped) => (stripped, true),
        None => (block_part, false),
    };
    if !block_part.is_empty() && !is_block_id(block_part) {
        return TokenParse::Invalid(token_diagnostic(
            token,
            "invalid_task_block_id",
            TASK_BLOCK_ID_ERROR,
        ));
    }

    let mut marker_parse = marker_parse(
        token,
        MarkerShape {
            sigil_len: 1,
            route_part,
            separator_len: 1,
            right_part: block_part,
            route_kind: SpanKind::TaskBlockIdRoute,
            right_kind: SpanKind::TaskBlockId,
            complete_mode: if project_note {
                EditorMode::ProjectNote
            } else {
                EditorMode::Task
            },
            right_need: Need::BlockId,
            third: None,
            suffix: project_note.then_some(MarkerSuffix {
                len: 1,
                kind: SpanKind::ProjectNoteMarker,
            }),
        },
    );
    // `@^<id>+` carries the project-note intent even while the route is
    // still missing; an empty block ID stays an ordinary incomplete state.
    if project_note && !block_part.is_empty() && route_part.is_empty() {
        marker_parse.mode = EditorMode::ProjectNote;
    }
    TokenParse::Marker(marker_parse)
}

pub(super) fn classify_retired_double_colon_token(
    token: &Token<'_>,
) -> TokenParse {
    TokenParse::Invalid(token_diagnostic(
        token,
        "retired_task_block_id_marker",
        RETIRED_DOUBLE_COLON_ERROR,
    ))
}

pub(super) fn classify_pomodoro_token(token: &Token<'_>) -> TokenParse {
    let legacy = token.text.starts_with("@!");
    let sigil_len = if legacy { 2 } else { 1 };
    let marker = &token.text[sigil_len..];
    let (route_part, rest, separator) = match marker.split_once(':') {
        Some((route, rest)) => (route, rest, true),
        None => (marker, "", false),
    };
    let (rest_before_start, start_part) = match rest.split_once('=') {
        Some((before, suffix)) => (before, Some(suffix)),
        None => (rest, None),
    };
    let (block_part, name_part) = match rest_before_start.split_once('#') {
        Some((block, name)) => (block, Some(name)),
        None => (rest_before_start, None),
    };

    if legacy && separator && route_part.is_empty() {
        // `@!:id` has never been a legal shorthand: the Hammerspoon grammar
        // and `bob capture` both require a route before the colon.
        return TokenParse::Invalid(token_diagnostic(
            token,
            "invalid_pomodoro_route",
            POMODORO_ROUTE_ERROR,
        ));
    }
    if !route_part.is_empty() && !is_route_token(route_part) {
        return TokenParse::Invalid(token_diagnostic(
            token,
            "invalid_pomodoro_route",
            POMODORO_ROUTE_ERROR,
        ));
    }
    // The project-note `+` sits immediately after the block ID, before any
    // `#`. A `+` anywhere else -- notably inside the Pomodoro name -- is
    // ordinary Pomodoro-name charset, so `@sase:deep-fix#bugs+` keeps
    // naming the Pomodoro `bugs+`.
    let (block_part, project_note) = match block_part.strip_suffix('+') {
        Some(stripped) => (stripped, true),
        None => (block_part, false),
    };
    if !block_part.is_empty() && !is_block_id(block_part) {
        return TokenParse::Invalid(token_diagnostic(
            token,
            "invalid_pomodoro_block_id",
            POMODORO_BLOCK_ID_ERROR,
        ));
    }
    if name_part.is_some_and(|name| {
        !name.is_empty() && !is_pomodoro_selector_component(name)
    }) {
        return TokenParse::Invalid(token_diagnostic(
            token,
            "invalid_pomodoro_name",
            POMODORO_NAME_ERROR,
        ));
    }
    let mut close_spec = None;
    let mut close_suffix: Option<EditorCloseSuffix> = None;
    let start_spec = match start_part {
        None => None,
        Some(raw)
            if raw.eq_ignore_ascii_case("x")
                || link_close_after_x(raw).is_some() =>
        {
            // A close-shaped suffix lexes through the shared selection
            // lexer: a valid selection reports the spec plus list spans, a
            // dangling separator reports the partial spec as an incomplete
            // state below, and a malformed list is an
            // `invalid_pomodoro_close` diagnostic with its precise range.
            let display = format!("={raw}");
            let after_x = link_close_after_x(raw).unwrap_or("");
            let eq_rel = token.text.find('=').unwrap_or(token.text.len());
            let after_x_base = token.start + eq_rel + 2;
            match lex_close_selection(after_x, after_x_base, &display) {
                Ok(CloseSelectionOutcome::Valid(lex)) => {
                    close_suffix = Some(EditorCloseSuffix::Valid {
                        in_progress: lex.in_progress_range,
                        complete: lex.complete_range,
                    });
                    close_spec = Some(close_spec_from_lex(display, &lex));
                }
                Ok(CloseSelectionOutcome::Incomplete(incomplete)) => {
                    close_suffix = Some(EditorCloseSuffix::Incomplete {
                        spec: close_spec_from_incomplete(display, &incomplete),
                        in_progress: incomplete.in_progress_range,
                        complete: incomplete.complete_range,
                        separator: incomplete.separator_range,
                    });
                }
                Err(error) => {
                    return TokenParse::Invalid(Diagnostic {
                        severity: Severity::Error,
                        code: "invalid_pomodoro_close",
                        message: error.message,
                        range: Some(error.range),
                    });
                }
            }
            None
        }
        Some(raw) => match parse_pomodoro_start_suffix(raw) {
            Ok(spec) => Some(spec),
            Err(_) => {
                return TokenParse::Invalid(token_diagnostic(
                    token,
                    "invalid_pomodoro_start",
                    POMODORO_START_SHAPE_ERROR,
                ));
            }
        },
    };
    if project_note && start_spec.is_some() {
        return TokenParse::Invalid(token_diagnostic(
            token,
            "invalid_pomodoro_start",
            POMODORO_START_PROJECT_NOTE_ERROR,
        ));
    }
    if project_note && close_spec.is_some() {
        return TokenParse::Invalid(token_diagnostic(
            token,
            "invalid_pomodoro_close",
            POMODORO_CLOSE_PROJECT_NOTE_ERROR,
        ));
    }
    if let (Some(name), Some(_)) = (name_part, close_spec.as_ref())
        && !name.is_empty()
    {
        // `#name=x`: the `#name` component is the precise range.
        let hash = token.text.find('#').unwrap_or(token.text.len());
        let eq = token.text.find('=').unwrap_or(token.text.len());
        return TokenParse::Invalid(Diagnostic {
            severity: Severity::Error,
            code: "invalid_pomodoro_close",
            message: format!(
                "`=x` always closes the running Pomodoro; remove `#{name}` (drop `=x` to link under a named Pomodoro instead)"
            ),
            range: Some((token.start + hash, token.start + eq)),
        });
    }

    let separator_len = usize::from(separator);
    // The session suffix is a trailing component like the project-note `+`
    // sigil: `marker_parse` ends the Pomodoro-name span before it and emits
    // the `pomodoro_start`/`pomodoro_close` span itself, so the name and
    // suffix spans never overlap (and `merge_spans` never drops the span).
    let start_suffix = start_spec.as_ref().map(|spec| MarkerSuffix {
        len: 1 + spec.raw.len(),
        kind: SpanKind::PomodoroStart,
    });
    // The typed suffix length: the full `=x…` selection for a valid close,
    // or what was typed so far for a dangling separator.
    let close_suffix_len = match &close_suffix {
        Some(EditorCloseSuffix::Valid { .. }) => {
            close_spec.as_ref().map(|spec| spec.raw.len())
        }
        Some(EditorCloseSuffix::Incomplete { spec, .. }) => {
            Some(spec.raw.len())
        }
        None => None,
    };
    let close_suffix_span = close_suffix_len.map(|len| MarkerSuffix {
        len,
        kind: SpanKind::PomodoroClose,
    });
    let mut marker_parse = marker_parse(
        token,
        MarkerShape {
            sigil_len,
            route_part,
            separator_len,
            right_part: block_part,
            route_kind: SpanKind::PomodoroRoute,
            right_kind: SpanKind::PomodoroBlockId,
            complete_mode: if project_note {
                EditorMode::PomodoroProjectNote
            } else {
                EditorMode::PomodoroTask
            },
            right_need: Need::PomodoroId,
            third: name_part.map(|part| MarkerThird {
                // `+#` spans two bytes once the project-note sigil is
                // present; a plain `#` spans one.
                separator_len: 1 + usize::from(project_note),
                part,
                kind: SpanKind::PomodoroName,
                need: Need::PomodoroName,
            }),
            suffix: start_suffix.or(close_suffix_span).or((project_note
                && name_part.is_none())
            .then_some(MarkerSuffix {
                len: 1,
                kind: SpanKind::ProjectNoteMarker,
            })),
        },
    );
    marker_parse.pomodoro_start = start_spec;
    match close_suffix {
        Some(EditorCloseSuffix::Valid {
            in_progress,
            complete,
        }) => {
            // Split the whole-suffix `pomodoro_close` span into `=x` plus
            // the list spans. (A plain `=x` re-emits the identical single
            // span.)
            let spec = close_spec.expect("valid close spec");
            let suffix_start = token.end - spec.raw.len();
            if marker_parse
                .spans
                .last()
                .is_some_and(|span| span.kind == SpanKind::PomodoroClose)
            {
                marker_parse.spans.pop();
            }
            marker_parse.spans.push(Span {
                start: suffix_start,
                end: suffix_start + 2,
                kind: SpanKind::PomodoroClose,
            });
            if let Some((start, end)) = in_progress {
                marker_parse.spans.push(Span {
                    start,
                    end,
                    kind: SpanKind::PomodoroCloseInProgress,
                });
            }
            if let Some((start, end)) = complete {
                marker_parse.spans.push(Span {
                    start,
                    end,
                    kind: SpanKind::PomodoroCloseComplete,
                });
            }
            marker_parse.pomodoro_close = Some(spec);
        }
        Some(EditorCloseSuffix::Incomplete {
            spec,
            in_progress,
            complete,
            separator,
        }) => {
            // The dangling separator is an editing state: the partial spec,
            // the spans typed so far, and one `interactive_placeholder`
            // span over the separator.
            let suffix_start = token.end - spec.raw.len();
            if marker_parse
                .spans
                .last()
                .is_some_and(|span| span.kind == SpanKind::PomodoroClose)
            {
                marker_parse.spans.pop();
            }
            marker_parse.spans.push(Span {
                start: suffix_start,
                end: suffix_start + 2,
                kind: SpanKind::PomodoroClose,
            });
            if let Some((start, end)) = in_progress {
                marker_parse.spans.push(Span {
                    start,
                    end,
                    kind: SpanKind::PomodoroCloseInProgress,
                });
            }
            if let Some((start, end)) = complete {
                marker_parse.spans.push(Span {
                    start,
                    end,
                    kind: SpanKind::PomodoroCloseComplete,
                });
            }
            marker_parse.spans.push(Span {
                start: separator.0,
                end: separator.1,
                kind: SpanKind::InteractivePlaceholder,
            });
            marker_parse.pomodoro_close = Some(spec);
            marker_parse.mode = EditorMode::Incomplete;
            if !marker_parse.needs.contains(&Need::PomodoroCloseTask) {
                marker_parse.needs.push(Need::PomodoroCloseTask);
            }
        }
        None => {
            marker_parse.pomodoro_close = close_spec;
        }
    }
    if project_note && name_part.is_some() {
        // `marker_parse` leaves the `+#` separator bytes uncovered, so span
        // the `+` sigil explicitly: ahead of the Pomodoro name, or split out
        // of the `#` placeholder when the name is still missing.
        let plus_start = token.start
            + sigil_len
            + route_part.len()
            + separator_len
            + block_part.len();
        let plus_span = Span {
            start: plus_start,
            end: plus_start + 1,
            kind: SpanKind::ProjectNoteMarker,
        };
        if name_part.is_some_and(|part| !part.is_empty()) {
            marker_parse
                .spans
                .insert(marker_parse.spans.len().saturating_sub(1), plus_span);
        } else if marker_parse.spans.pop().is_some() {
            marker_parse.spans.push(plus_span);
            marker_parse.spans.push(Span {
                start: plus_start + 1,
                end: plus_start + 2,
                kind: SpanKind::InteractivePlaceholder,
            });
        }
    }
    // `@:<id>+` carries the project-note intent even while the route is
    // still missing; an empty block ID stays an ordinary incomplete state.
    if project_note && !block_part.is_empty() && route_part.is_empty() {
        marker_parse.mode = EditorMode::PomodoroProjectNote;
    }
    TokenParse::Marker(marker_parse)
}

/// Classify the remaining `@` forms: a bare `@`, the `@#`/`@#prefix` target
/// pickers, `@route#`/`@route#prefix` bullets, and a plain `@route` task.
pub(super) fn classify_route_token(token: &Token<'_>) -> Option<MarkerParse> {
    let rest = token.text.strip_prefix('@')?;

    let Some((route_part, prefix)) = rest.split_once('#') else {
        if rest.is_empty() {
            return Some(MarkerParse {
                mode: EditorMode::Incomplete,
                route: None,
                section: None,
                block_id: None,
                needs: vec![Need::Route],
                spans: vec![Span {
                    start: token.start,
                    end: token.end,
                    kind: SpanKind::InteractivePlaceholder,
                }],
                requires_body: false,
                pomodoro_start: None,
                pomodoro_close: None,
            });
        }
        if !is_route_token(rest) {
            return None;
        }
        return Some(MarkerParse {
            mode: EditorMode::Task,
            route: Some(rest.to_ascii_lowercase()),
            section: None,
            block_id: None,
            needs: Vec::new(),
            spans: vec![Span {
                start: token.start,
                end: token.end,
                kind: SpanKind::Route,
            }],
            requires_body: true,
            pomodoro_start: None,
            pomodoro_close: None,
        });
    };

    if !route_part.is_empty() && !is_route_token(route_part) {
        // `@bad.route#x` never routed and is not an interactive state either,
        // so it stays literal exactly like `bob capture` leaves it.
        return None;
    }

    Some(marker_parse(
        token,
        MarkerShape {
            sigil_len: 1,
            route_part,
            separator_len: 1,
            right_part: prefix,
            route_kind: SpanKind::Route,
            right_kind: SpanKind::Section,
            complete_mode: EditorMode::Bullet,
            right_need: Need::Section,
            third: None,
            suffix: None,
        },
    ))
}

/// The shared shape of every `@<route><separator><right>` marker, with an
/// optional third component for `@route+block-id#section` and
/// `@route:id#pomodoro`.
pub(super) struct MarkerShape<'a> {
    pub(super) sigil_len: usize,
    pub(super) route_part: &'a str,
    pub(super) separator_len: usize,
    pub(super) right_part: &'a str,
    pub(super) route_kind: SpanKind,
    pub(super) right_kind: SpanKind,
    pub(super) complete_mode: EditorMode,
    pub(super) right_need: Need,
    pub(super) third: Option<MarkerThird<'a>>,
    pub(super) suffix: Option<MarkerSuffix>,
}

pub(super) struct MarkerThird<'a> {
    pub(super) separator_len: usize,
    pub(super) part: &'a str,
    pub(super) kind: SpanKind,
    pub(super) need: Need,
}

pub(super) struct MarkerSuffix {
    pub(super) len: usize,
    pub(super) kind: SpanKind,
}

/// Build the mode, needs, and spans for one marker from its component parts.
///
/// Spans never overlap and always sit on `char` boundaries. When a component
/// is still empty its sigil and separator become one
/// `interactive_placeholder` span so an editor can highlight the caret
/// position the user still has to fill in.
pub(super) fn marker_parse(
    token: &Token<'_>,
    shape: MarkerShape<'_>,
) -> MarkerParse {
    let suffix_len =
        shape.suffix.as_ref().map(|suffix| suffix.len).unwrap_or(0);
    let content_end = token.end - suffix_len;
    let route_end = token.start + shape.sigil_len + shape.route_part.len();
    let has_route = !shape.route_part.is_empty();
    let has_right = !shape.right_part.is_empty();
    let has_third_sep = shape.third.is_some();
    let third_part = shape.third.as_ref().map(|third| third.part).unwrap_or("");
    let third_sep_len = shape
        .third
        .as_ref()
        .map(|third| third.separator_len)
        .unwrap_or(0);
    let has_third = has_third_sep && !third_part.is_empty();
    let right_start = route_end + shape.separator_len;
    let right_end = right_start + shape.right_part.len();
    let third_start = right_end + third_sep_len;

    let mut spans = Vec::new();
    if has_route {
        spans.push(Span {
            start: token.start,
            end: route_end,
            kind: shape.route_kind,
        });
        if !has_right && shape.separator_len > 0 {
            spans.push(Span {
                start: route_end,
                end: right_start,
                kind: SpanKind::InteractivePlaceholder,
            });
        }
    } else {
        let placeholder_end = route_end + shape.separator_len;
        spans.push(Span {
            start: token.start,
            end: placeholder_end,
            kind: SpanKind::InteractivePlaceholder,
        });
    }
    if has_right {
        spans.push(Span {
            start: right_start,
            end: right_end,
            kind: shape.right_kind,
        });
    }
    if let Some(third) = shape.third.as_ref() {
        if third.part.is_empty() {
            spans.push(Span {
                start: right_end,
                end: third_start,
                kind: SpanKind::InteractivePlaceholder,
            });
        } else {
            spans.push(Span {
                start: third_start,
                end: content_end,
                kind: third.kind,
            });
        }
    }
    if let Some(suffix) = shape.suffix.as_ref() {
        spans.push(Span {
            start: content_end,
            end: token.end,
            kind: suffix.kind,
        });
    }

    // A `@route#` bullet is executable today (it means "any non-Tasks
    // section"), so it keeps its complete mode while still reporting the
    // section it could still resolve. A section can only be offered once the
    // route that owns its headings is known. A trailing `#` on a sub-bullet
    // marker is required once typed: `@route+id#` is incomplete.
    let section_is_optional =
        shape.right_need == Need::Section && !has_third_sep;
    let third_ok = !has_third_sep || has_third;

    let mut needs = Vec::new();
    if !has_route {
        needs.push(Need::Route);
    }
    if !has_right && (has_route || !section_is_optional) {
        needs.push(shape.right_need);
    }
    if let Some(third) = shape.third.as_ref()
        && third.part.is_empty()
    {
        needs.push(third.need);
    }

    let mode = if has_route && (has_right || section_is_optional) && third_ok {
        shape.complete_mode
    } else {
        EditorMode::Incomplete
    };

    let section = if has_third {
        Some(third_part.to_string())
    } else if shape.right_need == Need::Section && has_right {
        Some(shape.right_part.to_string())
    } else {
        None
    };

    MarkerParse {
        mode,
        route: has_route.then(|| shape.route_part.to_ascii_lowercase()),
        section,
        block_id: (shape.right_need != Need::Section && has_right)
            .then(|| shape.right_part.to_string()),
        needs,
        spans,
        requires_body: false,
        pomodoro_start: None,
        pomodoro_close: None,
    }
}

pub(super) fn token_diagnostic(
    token: &Token<'_>,
    code: &'static str,
    message: &str,
) -> Diagnostic {
    Diagnostic {
        severity: Severity::Error,
        code,
        message: message.to_string(),
        range: Some((token.start, token.end)),
    }
}

/// Surface the retired standalone `#...` bullet marker as a diagnostic. The
/// rule itself is [`reject_legacy_bullet_markers`], so the editor and
/// `bob capture` can never disagree about which inputs are affected.
pub(super) fn legacy_bullet_marker_diagnostic(
    tokens: &[Token<'_>],
) -> Option<Diagnostic> {
    let texts: Vec<&str> = tokens.iter().map(|token| token.text).collect();
    let message = reject_legacy_bullet_markers(&texts, true).err()?;
    let last = tokens.last()?;
    let offender = if last.text.starts_with('#') {
        last
    } else {
        tokens.get(tokens.len().checked_sub(2)?)?
    };

    Some(Diagnostic {
        severity: Severity::Error,
        code: "legacy_bullet_marker",
        message,
        range: Some((offender.start, offender.end)),
    })
}

/// Diagnose a same-line combination of the bare `#` Pomodoro-note marker
/// with an `@route`-shaped marker, mirroring [`resolve_line`]'s route
/// conflict rejection so the editor and `bob capture` never disagree.
/// Schedule (`s:<N>`) and priority (`p:<N>`) conflicts are item-wide and
/// diagnosed separately in [`parse_editor_item`] once the whole item's mode
/// is known; a forced `--route` conflict never reaches the editor, which has
/// no forced-route flag.
pub(super) fn pomodoro_note_conflict_diagnostic(
    tokens: &[Token<'_>],
    leading: bool,
) -> Option<Diagnostic> {
    let texts: Vec<&str> = tokens.iter().map(|token| token.text).collect();
    if !texts.contains(&"#") {
        return None;
    }
    let hash = tokens.iter().find(|token| token.text == "#")?;

    let conflict = match texts.last() {
        Some(&"#") => {
            let remaining = &texts[..texts.len() - 1];
            (leading
                && remaining
                    .first()
                    .is_some_and(|token| is_route_marker(token)))
                || remaining.last().is_some_and(|token| is_route_marker(token))
        }
        Some(&last) => {
            (leading
                && texts.first().is_some_and(|token| is_route_marker(token)))
                || is_route_marker(last)
        }
        None => false,
    };

    conflict.then(|| Diagnostic {
        severity: Severity::Error,
        code: "pomodoro_note_conflict",
        message: pomodoro_note_route_conflict_error(),
        range: Some((hash.start, hash.end)),
    })
}

// ---------------------------------------------------------------------------
// Cursor-aware completion
// ---------------------------------------------------------------------------
