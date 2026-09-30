//! Line resolution and error constructors.

use super::draft::*;
use super::markers::*;
use super::model::*;
use super::tokens::*;

/// One physical line's resolved item-wide markers and (when a route was
/// recognized on this line) its route/mode token. `body` is the line's
/// remaining text after every recognized marker is removed; it is empty
/// exactly when the line held no non-marker tokens.
pub(super) struct LineOutcome<'a> {
    pub(super) body: String,
    pub(super) markers: TerminalMarkers,
    pub(super) route: Option<LineRoute>,
    pub(super) declarations: Vec<Token<'a>>,
}

pub(super) struct LineRoute {
    pub(super) token: RouteToken,
    pub(super) marker_text: String,
}

/// Resolve one physical line's whitespace tokens exactly like the original
/// single-line grammar resolved the whole draft. `leading` allows a
/// first-token route to win and is only ever set for the parent line, which
/// is the only line that preserves the established leading-route form.
/// `detect_route` is false whenever `--route`/`--section` already fixed the
/// route, in which case every `@...`-shaped token stays literal on every
/// line, exactly like the single-line forced-route path did.
pub(super) fn resolve_line<'a>(
    mut tokens: Vec<Token<'a>>,
    leading: bool,
    detect_route: bool,
    parse_clip_markers: bool,
) -> Result<LineOutcome<'a>, String> {
    // A trailing exact `#now` is the weekly-bet tag, not a route or a
    // legacy marker: resolve the route in front of it, then move the tag to
    // the end of the body. With a forced route every `@...` token stays
    // literal, so `#now` stays literal body text there too.
    let trailing_now = (detect_route
        && tokens.last().is_some_and(|token| is_now_tag(token.text)))
    .then(|| tokens.pop())
    .flatten()
    .is_some();
    match resolve_line_inner(tokens, leading, detect_route, parse_clip_markers)
    {
        Err(message) if trailing_now && message == missing_text_error() => {
            Err(now_tag_body_error())
        }
        Ok(mut outcome) if trailing_now => {
            if outcome.body.is_empty() {
                return Err(now_tag_body_error());
            }
            outcome.body.push_str(" #now");
            Ok(outcome)
        }
        outcome => outcome,
    }
}

/// [`resolve_line`] without the trailing-`#now` handling: every return
/// below sees the line with the tag already removed.
fn resolve_line_inner<'a>(
    mut tokens: Vec<Token<'a>>,
    leading: bool,
    detect_route: bool,
    parse_clip_markers: bool,
) -> Result<LineOutcome<'a>, String> {
    let declarations = take_global_declarations(&mut tokens);
    let (markers, _) =
        extract_terminal_markers(&mut tokens, parse_clip_markers);
    if tokens.is_empty() {
        return Ok(LineOutcome {
            body: String::new(),
            markers,
            route: None,
            declarations,
        });
    }

    let token_texts = tokens.iter().map(|token| token.text).collect::<Vec<_>>();
    reject_legacy_bullet_markers(&token_texts, detect_route)?;

    // The bare `#` Pomodoro-note marker claims the route/mode slot from the
    // final token position, exactly like an `@route` token does, but it
    // never has a partially-typed form and never coexists with one.
    if tokens
        .last()
        .is_some_and(|token| is_pomodoro_note_marker(token.text))
    {
        if !detect_route {
            return Err(pomodoro_note_forced_route_conflict_error());
        }
        let marker_text = tokens.last().expect("last token").text.to_string();
        tokens.pop();
        if tokens.is_empty() {
            return Err(missing_text_error());
        }
        if (leading
            && tokens
                .first()
                .is_some_and(|token| is_route_marker(token.text)))
            || tokens
                .last()
                .is_some_and(|token| is_route_marker(token.text))
        {
            return Err(pomodoro_note_route_conflict_error());
        }
        return Ok(LineOutcome {
            body: join_parse_tokens(&tokens),
            markers,
            route: Some(LineRoute {
                token: RouteToken {
                    route: None,
                    kind: CaptureKind::PomodoroNote,
                },
                marker_text,
            }),
            declarations,
        });
    }

    if !detect_route {
        return Ok(LineOutcome {
            body: join_parse_tokens(&tokens),
            markers,
            route: None,
            declarations,
        });
    }

    // Leading route wins: when the first token is a route token followed by
    // body text, route by it and do not inspect later route-looking tokens.
    if leading && let Some(token) = parse_terminal_route_token(tokens[0].text)?
    {
        let rest = &tokens[1..];
        if rest.is_empty() {
            if matches!(token.kind, CaptureKind::Task) {
                // A bare `@foo` with no body stays literal task text.
            } else if matches!(
                token.kind,
                CaptureKind::SubBullet {
                    target: SubBulletTarget::BlockId(_),
                    ..
                } | CaptureKind::TaskToggle { .. }
                    | CaptureKind::Pomodoro { .. }
            ) {
                // A bare `@route+block-id[#name]`, `@route+block-id!`, or
                // `@route:block-id[#pomodoro][=<X>]` routes with an empty
                // body; `parse_capture_item` decides whether the finished
                // item qualifies as a task-toggle or Pomodoro-link operation
                // once children and other item-wide markers are known.
                return Ok(LineOutcome {
                    body: String::new(),
                    markers,
                    route: Some(LineRoute {
                        token,
                        marker_text: tokens[0].text.to_string(),
                    }),
                    declarations,
                });
            } else {
                return Err(missing_text_error());
            }
        } else {
            if rest.iter().any(|token| token.text == "#") {
                return Err(pomodoro_note_route_conflict_error());
            }
            return Ok(LineOutcome {
                body: join_parse_tokens(rest),
                markers,
                route: Some(LineRoute {
                    token,
                    marker_text: tokens[0].text.to_string(),
                }),
                declarations,
            });
        }
    }

    validate_special_terminal_markers_line(&token_texts, leading)?;

    // Otherwise a trailing route token routes the body that precedes it.
    if let Some((last, rest)) = tokens.split_last()
        && !rest.is_empty()
        && let Some(token) = parse_terminal_route_token(last.text)?
    {
        if rest.iter().any(|token| token.text == "#") {
            return Err(pomodoro_note_route_conflict_error());
        }
        return Ok(LineOutcome {
            body: join_parse_tokens(rest),
            markers,
            route: Some(LineRoute {
                token,
                marker_text: last.text.to_string(),
            }),
            declarations,
        });
    }

    Ok(LineOutcome {
        body: join_parse_tokens(&tokens),
        markers,
        route: None,
        declarations,
    })
}

pub(super) fn join_parse_tokens<T: ParseToken>(tokens: &[T]) -> String {
    tokens
        .iter()
        .map(|token| token.text())
        .collect::<Vec<_>>()
        .join(" ")
}

/// Accumulate the four item-wide marker slots (route/mode, schedule,
/// priority, clipboard) across every physical line. Each slot may be set by
/// at most one line; a second line that resolves the same slot is ambiguous.
#[derive(Default)]
pub(super) struct AggregateMarkers {
    pub(super) clip: Option<ClipRequest>,
    pub(super) scheduled_offset: Option<u64>,
    pub(super) priority_level: Option<u64>,
    pub(super) route: Option<LineRoute>,
}

impl AggregateMarkers {
    pub(super) fn absorb(
        &mut self,
        markers: TerminalMarkers,
        route: Option<LineRoute>,
    ) -> Result<(), String> {
        if let Some(clip) = markers.clip {
            if self.clip.is_some() {
                return Err(duplicate_marker_error("clipboard marker (%)"));
            }
            self.clip = Some(clip);
        }
        if let Some(offset) = markers.scheduled_offset {
            if self.scheduled_offset.is_some() {
                return Err(duplicate_marker_error("schedule marker (s:<N>)"));
            }
            self.scheduled_offset = Some(offset);
        }
        if let Some(level) = markers.priority_level {
            if self.priority_level.is_some() {
                return Err(duplicate_marker_error("priority marker (p:<N>)"));
            }
            self.priority_level = Some(level);
        }
        if let Some(route) = route {
            if self.route.is_some() {
                return Err(duplicate_marker_error(
                    "route/mode marker (@route or #)",
                ));
            }
            self.route = Some(route);
        }
        Ok(())
    }
}

pub(super) fn duplicate_marker_error(kind: &str) -> String {
    format!(
        "a {kind} may appear on only one line of the capture; found a \
second one"
    )
}

pub(super) fn invalid_child_line_error(line_number: usize) -> String {
    format!(
        "capture line {line_number} must be a column-zero bullet or a \
two-space nested bullet using \"-\", \"*\", or \"+\" followed by a space or \
tab, or be left blank"
    )
}

pub(super) fn empty_child_after_markers_error(line_number: usize) -> String {
    format!(
        "capture line {line_number} has no text left after its capture \
markers were removed"
    )
}

pub(super) fn orphaned_nested_bullet_error(line_number: usize) -> String {
    format!(
        "capture line {line_number} is a nested bullet but has no preceding \
first-level authored bullet to attach to"
    )
}

pub(crate) fn missing_text_error() -> String {
    "task text is required; pass TEXT or pipe it on stdin".to_string()
}

pub(super) fn missing_capture_item_error() -> String {
    MISSING_CAPTURE_ITEM_ERROR.to_string()
}

pub(super) fn unsupported_global_destination_error(token: &str) -> String {
    format!("{GLOBAL_DESTINATION_SHAPE_ERROR}; {token} is not supported")
}

pub(super) fn legacy_marker_error() -> String {
    "bullet section markers must be appended to an @route token; use \
@foo#bar instead of #bar @foo"
        .to_string()
}

pub(super) fn pomodoro_note_route_conflict_error() -> String {
    "the '#' Pomodoro-note marker cannot be combined with an @route marker"
        .to_string()
}

pub(super) fn pomodoro_note_forced_route_conflict_error() -> String {
    "the '#' Pomodoro-note marker cannot be combined with --route".to_string()
}

pub(super) fn pomodoro_note_schedule_conflict_error() -> String {
    "the '#' Pomodoro-note marker cannot be combined with 's:<N>'".to_string()
}

pub(super) fn pomodoro_note_priority_conflict_error() -> String {
    "the '#' Pomodoro-note marker cannot be combined with 'p:<N>'".to_string()
}

pub(crate) fn normalize_task_text(raw_text: &str) -> String {
    raw_text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Canonical whitespace-free selector slug for task-section and Pomodoro-name
/// third components: trim, collapse internal whitespace to one space,
/// ASCII-lowercase, then replace each remaining space with `-`.
pub(crate) fn selector_slug(text: &str) -> String {
    let mut result = String::new();
    for word in text.split_whitespace() {
        if !result.is_empty() {
            result.push('-');
        }
        result.push_str(&word.to_ascii_lowercase());
    }
    result
}
