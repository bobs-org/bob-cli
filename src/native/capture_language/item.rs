//! Capture-item and session-operator parsing.

use super::bare_name::*;
use super::close_log::*;
use super::close_selection::*;
use super::dependencies::*;
use super::draft::*;
use super::editor_parse::*;
use super::line::*;
use super::markers::*;
use super::model::*;
use super::move_bullet::MoveBulletHit;
use super::move_bullet::MoveBulletInvalid;
use super::pomodoro_literal::{
    canonicalize_exact_name, decode_close_hash_name, lex_exact_name_after_hash,
    opens_exact_name,
};
use super::project_tasks::*;
use super::start_selection::*;
use super::tokens::*;
use crate::native::url_routing::{classify_token, RefIntent, RoutingEntry};

pub(super) struct ParsedCaptureItemOutcome<'a> {
    pub(super) index: usize,
    pub(super) start: usize,
    pub(super) end: usize,
    pub(super) line_start: usize,
    pub(super) line_end: usize,
    pub(super) parsed: ParsedCaptureText,
    pub(super) declarations: Vec<GlobalDeclarationToken<'a>>,
    pub(super) local_destination_marker: Option<String>,
    pub(super) list: Option<ListContext>,
    /// Set by draft assembly after one member has been resolved.
    pub(super) fanout: Option<FanoutContext>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct TargetRunContext {
    pub(super) line: usize,
    pub(super) selection: super::target_runs::TargetRunSelection,
    pub(super) terminal_markers: TerminalMarkers,
    pub(super) terminal_spans:
        Vec<(super::editor_model::SpanKind, usize, usize)>,
}

fn apply_target_run_terminal_markers(
    destination: &mut TerminalMarkers,
    extracted: &TerminalMarkers,
) -> Result<(), String> {
    if let Some(offset) = extracted.scheduled_offset {
        if destination.scheduled_offset.is_some() {
            return Err(duplicate_marker_error("schedule marker (s:<N>)"));
        }
        destination.scheduled_offset = Some(offset);
    }
    if let Some(level) = extracted.priority_level {
        if destination.priority_level.is_some() {
            return Err(duplicate_marker_error("priority marker ($<N>)"));
        }
        destination.priority_level = Some(level);
    }
    if let Some(retired) = &extracted.retired_priority {
        return Err(retired_priority_marker_error(&retired.token));
    }
    destination
        .priority_escapes
        .extend(extracted.priority_escapes.iter().copied());
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct CaptureItemParseError {
    pub(super) code: Option<&'static str>,
    pub(super) message: String,
}

impl CaptureItemParseError {
    fn generic(message: String) -> Self {
        Self {
            code: None,
            message,
        }
    }

    pub(crate) fn coded(code: &'static str, message: String) -> Self {
        Self {
            code: Some(code),
            message,
        }
    }
}

impl From<String> for CaptureItemParseError {
    fn from(message: String) -> Self {
        Self::generic(message)
    }
}

/// Attempt run-aware parent-line resolution. A one-member or literal line
/// falls back to the established singleton parser; a selected run carries
/// its extracted terminal evidence alongside the member selection.
fn try_resolve_parent_target_run<'a>(
    mut tokens: Vec<Token<'a>>,
    detect_route: bool,
) -> Option<
    Result<
        (
            LineOutcome<'a>,
            super::target_runs::TargetRunSelection,
            Vec<(super::editor_model::SpanKind, usize, usize)>,
        ),
        CaptureItemParseError,
    >,
> {
    use super::target_runs;
    let declarations = take_global_declarations(&mut tokens);
    let (markers, terminal_spans) =
        target_runs::extract_terminal_markers_crossing_runs(&mut tokens);
    if tokens.is_empty() {
        return None;
    }
    let refusal =
        target_runs::detect_target_refusals(&tokens, true, detect_route);
    if let Some((code, message, _)) =
        target_runs::strict_refusal_parts(&refusal)
    {
        return Some(Err(CaptureItemParseError::coded(code, message)));
    }
    let selection =
        target_runs::select_target_run(&tokens, true, detect_route)?;
    if selection.members.len() < 2 {
        return None;
    }
    if let Some(retired) = &markers.retired_priority {
        return Some(Err(CaptureItemParseError::generic(
            retired_priority_marker_error(&retired.token),
        )));
    }
    if let Err(message) =
        target_runs::validate_selected_members(&selection.members)
    {
        return Some(Err(CaptureItemParseError::generic(message)));
    }
    let first_text = selection.members.first()?.marker_text.clone();
    let route_token = match parse_terminal_route_token(&first_text) {
        Ok(Some(token)) => token,
        Ok(None) => return None,
        Err(message) => {
            return Some(Err(CaptureItemParseError::generic(message)))
        }
    };
    // Body is everything outside the selected run; the run is contiguous.
    let first_start = selection.members.first()?.token_index;
    let last_end = selection.members.last()?.token_index + 1;
    // Recompute run span on the post-extraction token list by matching the
    // selected absolute byte ranges back to positions.
    let mut run_positions = Vec::new();
    for (index, token) in tokens.iter().enumerate() {
        if selection.members.iter().any(|member| {
            member.start == token.start && member.end == token.end
        }) {
            run_positions.push(index);
        }
    }
    let (run_start, run_end) =
        match (run_positions.first(), run_positions.last()) {
            (Some(first), Some(last)) => (*first, last + 1),
            _ => (first_start.min(tokens.len()), last_end.min(tokens.len())),
        };
    let remaining: Vec<Token<'a>> = tokens
        .iter()
        .enumerate()
        .filter(|(index, _)| *index < run_start || *index >= run_end)
        .map(|(_, token)| *token)
        .collect();
    let body = join_parse_tokens(&remaining, &markers.priority_escapes);
    Some(Ok((
        LineOutcome {
            body,
            markers,
            route: Some(LineRoute {
                token: route_token,
                marker_text: first_text,
            }),
            declarations,
        },
        selection,
        terminal_spans,
    )))
}

/// Attempt run-aware child-line resolution (trailing runs only). Whole-line
/// child runs fall back to the singleton path so existing empty-child
/// diagnostics stay unchanged.
fn try_resolve_child_target_run<'a>(
    mut tokens: Vec<Token<'a>>,
    detect_route: bool,
) -> Option<
    Result<
        (
            LineOutcome<'a>,
            super::target_runs::TargetRunSelection,
            Vec<(super::editor_model::SpanKind, usize, usize)>,
        ),
        CaptureItemParseError,
    >,
> {
    use super::target_runs;
    let declarations = take_global_declarations(&mut tokens);
    let (markers, terminal_spans) =
        target_runs::extract_terminal_markers_crossing_runs(&mut tokens);
    if tokens.is_empty() {
        return None;
    }
    let refusal =
        target_runs::detect_target_refusals(&tokens, false, detect_route);
    if let Some((code, message, _)) =
        target_runs::strict_refusal_parts(&refusal)
    {
        return Some(Err(CaptureItemParseError::coded(code, message)));
    }
    let selection =
        target_runs::select_target_run(&tokens, false, detect_route)?;
    if selection.members.len() < 2 {
        return None;
    }
    if selection.position == target_runs::TargetRunPosition::WholeLine {
        return None;
    }
    if let Some(retired) = &markers.retired_priority {
        return Some(Err(CaptureItemParseError::generic(
            retired_priority_marker_error(&retired.token),
        )));
    }
    if let Err(message) =
        target_runs::validate_selected_members(&selection.members)
    {
        return Some(Err(CaptureItemParseError::generic(message)));
    }
    let first_text = selection.members.first()?.marker_text.clone();
    let route_token = match parse_terminal_route_token(&first_text) {
        Ok(Some(token)) => token,
        Ok(None) => return None,
        Err(message) => {
            return Some(Err(CaptureItemParseError::generic(message)))
        }
    };
    let mut run_positions = Vec::new();
    for (index, token) in tokens.iter().enumerate() {
        if selection.members.iter().any(|member| {
            member.start == token.start && member.end == token.end
        }) {
            run_positions.push(index);
        }
    }
    let (run_start, run_end) =
        match (run_positions.first(), run_positions.last()) {
            (Some(first), Some(last)) => (*first, last + 1),
            _ => return None,
        };
    let remaining: Vec<Token<'a>> = tokens
        .iter()
        .enumerate()
        .filter(|(index, _)| *index < run_start || *index >= run_end)
        .map(|(_, token)| *token)
        .collect();
    let body = join_parse_tokens(&remaining, &markers.priority_escapes);
    Some(Ok((
        LineOutcome {
            body,
            markers,
            route: Some(LineRoute {
                token: route_token,
                marker_text: first_text,
            }),
            declarations,
        },
        selection,
        terminal_spans,
    )))
}

/// Refuse dependency modifiers on Pomodoro operator items (close, start,
/// adjustment, shift) before any other item parser: they have no
/// unambiguous single task owner, so the whole batch rolls back instead of
/// capturing without the prerequisites. A remaining bare `^route:id` is an
/// owner alias, not an operator.
fn reject_operator_dependencies(
    item: &CaptureItem<'_>,
    parent_line: &ItemLine<'_>,
    forced_route: Option<&str>,
    forced_section: Option<&str>,
    sigil: u8,
) -> Result<(), String> {
    // A claimed operator refuses even when its own shape is broken: the
    // item is operator-shaped, so its own diagnostic would hide the
    // refused dependency combination.
    let claimed = |result: Result<
        Option<ParsedCaptureItemOutcome<'_>>,
        String,
    >| { !matches!(result, Ok(None)) };
    if claimed(parse_pomodoro_equals_item(
        item,
        parent_line,
        forced_route,
        forced_section,
    )) || claimed(parse_pomodoro_adjust_item(
        item,
        parent_line,
        forced_route,
        forced_section,
    )) || claimed(parse_pomodoro_link_item(
        item,
        parent_line,
        forced_route,
        forced_section,
    )) {
        return Err(operator_relation_error(sigil));
    }
    Ok(())
}

/// Resolve a plain `@route` token to its route name. Returns `None`
/// unless the token is exactly `@route` (case-insensitive) with no `#`
/// section or other suffix, the parsed kind is `Task`, and the route is
/// a known route token.
fn plain_ref_route(text: &str) -> Option<String> {
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

/// Claim a whole-item bare URL as a reference item when the routing
/// policy admits it. Runs after the operator claims and before the generic
/// `resolve_line`, so any operator-shaped text keeps its existing meaning.
///
/// An admitted bare URL is a reference when it has exactly one ordinary
/// leading or trailing `@route`, a forced `-r` route, or no route at all
/// (the default inbox; a plain `@@route` declaration upgrades it to a
/// global parent during draft resolution). When `listen` is true (the
/// `%l` directive was typed) the claim produces a listen reference;
/// otherwise it produces a plain reference. The body/URL intent excludes
/// the route token. Policy-gated markers (`-R`, excluded hosts, disabled
/// routing, child lines, extra words/tags, schedule/priority/clipboard,
/// dependencies, operators, task refs, destination flags beyond `-r`)
/// retain existing task or diagnostic behavior. Attached `path@`,
/// queries/fragments, and encoded forms stay unsupported and fall through.
fn claim_reference_item<'a>(
    item: &CaptureItem<'a>,
    options: &CaptureParseOptions<'_>,
    has_dependencies: bool,
    forced_route: Option<&str>,
    listen: bool,
) -> Option<ParsedCaptureItemOutcome<'a>> {
    let policy = options.url_routing?;
    if options.explicit_destination || has_dependencies || item.lines.len() != 1
    {
        return None;
    }
    let parent = item.lines.first()?.raw;
    let tokens = tokenize_with_spans(parent.text);
    // A `@@` token is a global declaration, never an inline ref parent.
    if tokens.iter().any(|token| token.text.starts_with("@@")) {
        return None;
    }
    if tokens.len() == 1 {
        let token = tokens[0];
        let intent = classify_token(token.text)?;
        if !policy.admits(&intent, RoutingEntry::Capture) {
            return None;
        }
        // A forced `-r` route is the explicit parent; otherwise the item
        // claims with no local route and draft resolution applies a plain
        // `@@route` global or leaves the default inbox.
        let kind = if listen {
            CaptureKind::Ref(RefIntent::with_listen(intent))
        } else {
            CaptureKind::Ref(RefIntent::without_listen(intent))
        };
        if let Some(forced) = forced_route {
            let route = normalize_forced_route(forced).ok()?;
            return Some(parsed_capture_item_outcome(
                item,
                ParsedCaptureText {
                    body: token.text.to_string(),
                    clip: None,
                    route: Some(route),
                    kind,
                    scheduled_offset: None,
                    priority_level: None,
                    sub_bullets: Vec::new(),
                    dependencies: Vec::new(),
                    blocked_tasks: Vec::new(),
                    relations: Vec::new(),
                    dependency_target: None,
                    type_marker: None,
                },
                Vec::new(),
                Some(format!("--route {forced}")),
            ));
        }
        return Some(parsed_capture_item_outcome(
            item,
            ParsedCaptureText {
                body: token.text.to_string(),
                clip: None,
                route: None,
                kind,
                scheduled_offset: None,
                priority_level: None,
                sub_bullets: Vec::new(),
                dependencies: Vec::new(),
                blocked_tasks: Vec::new(),
                relations: Vec::new(),
                dependency_target: None,
                type_marker: None,
            },
            Vec::new(),
            None,
        ));
    }
    if tokens.len() == 2 {
        if forced_route.is_some() && !listen {
            return None;
        }
        if forced_route.is_some() && listen {
            // A forced route with `%l` only claims a lone URL; a second
            // token alongside the URL stays an ordinary task.
            return None;
        }
        // Exactly one URL plus exactly one ordinary `@route`, in either
        // order. Anything else (extra words, tags, markers, `+`/`:`/`#`
        // shapes) falls through to the generic task path.
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
        let route = plain_ref_route(route_token.text)?;
        let kind = if listen {
            CaptureKind::Ref(RefIntent::with_listen(intent))
        } else {
            CaptureKind::Ref(RefIntent::without_listen(intent))
        };
        return Some(parsed_capture_item_outcome(
            item,
            ParsedCaptureText {
                body: url_token.text.to_string(),
                clip: None,
                route: Some(route),
                kind,
                scheduled_offset: None,
                priority_level: None,
                sub_bullets: Vec::new(),
                dependencies: Vec::new(),
                blocked_tasks: Vec::new(),
                relations: Vec::new(),
                dependency_target: None,
                type_marker: None,
            },
            Vec::new(),
            Some(route_token.text.to_string()),
        ));
    }
    None
}

pub(super) fn parse_capture_item<'a>(
    item: &CaptureItem<'a>,
    forced_route: Option<&str>,
    forced_section: Option<&str>,
    options: &CaptureParseOptions<'_>,
    target_run: Option<&TargetRunContext>,
) -> Result<ParsedCaptureItemOutcome<'a>, CaptureItemParseError> {
    let occurrences = super::type_marker::collect_item(item);
    let type_masked_source =
        super::type_marker::masked_source(item, &occurrences);
    let type_masked_item =
        super::type_marker::masked_item(item, &type_masked_source);
    let directive_occurrences =
        super::directive::collect_item(&type_masked_item);
    let masked_source = super::directive::masked_source(
        &type_masked_item,
        &directive_occurrences,
    );
    let mut masked_item =
        super::directive::masked_item(&type_masked_item, &masked_source);
    let marker_only =
        super::type_marker::marker_only_child_lines(item, &occurrences);
    masked_item
        .lines
        .retain(|line| !marker_only.contains(&(line.raw.start, line.raw.end)));

    if let Some(parent) = masked_item.lines.first()
        && normalize_task_text(parent.raw.text).is_empty()
        && occurrences.iter().any(|item| item.is_marker())
    {
        return Err(type_marker_only_error(&occurrences));
    }
    if let Some(parent) = masked_item.lines.first()
        && normalize_task_text(parent.raw.text).is_empty()
        && directive_occurrences.iter().any(|item| item.is_marker())
    {
        return Err(directive_only_error(&directive_occurrences));
    }

    let resolved = super::directive::resolve(&directive_occurrences).ok();
    let clip = resolved
        .as_ref()
        .and_then(|directives| directives.clip.clone());
    let note = resolved.as_ref().is_some_and(|directives| directives.note);
    let listen = resolved
        .as_ref()
        .is_some_and(|directives| directives.listen);
    let template = resolved
        .as_ref()
        .is_some_and(|directives| directives.template);
    if masked_item.list.is_some() {
        let outcome = parse_list_bullet_item(
            &masked_item,
            clip,
            note,
            listen,
            template,
            options,
        )?;
        // Re-slice borrowed declarations back to the original draft text;
        // masking preserves byte offsets, so only the slices change.
        let declarations =
            remap_type_marker_declarations(outcome.declarations, item.source);
        return Ok(ParsedCaptureItemOutcome {
            index: outcome.index,
            start: outcome.start,
            end: outcome.end,
            line_start: outcome.line_start,
            line_end: outcome.line_end,
            parsed: outcome.parsed,
            declarations,
            local_destination_marker: outcome.local_destination_marker,
            list: outcome.list,
            fanout: outcome.fanout,
        });
    }
    if forced_route.is_none()
        && forced_section.is_none()
        && options.recognize_lists
        && !template
        && !note
        && !listen
        && is_bare_at_list_attempt(item)
    {
        return Err(CaptureItemParseError::coded(
            "incomplete_route",
            "`@` is incomplete: type a destination after `@` (for example `@dev`)"
                .to_string(),
        ));
    }
    let parsed = parse_capture_item_masked(
        &masked_item,
        forced_route,
        forced_section,
        options,
        clip,
        note,
        listen,
        target_run,
    );
    let mut outcome = match parsed {
        Ok(outcome) => {
            // A bare Task route stays literal through resolve, so a header
            // plus only item-wide markers never reaches `missing_text`:
            // catch it on the success path instead.
            if outcome.list.is_none()
                && forced_route.is_none()
                && let Some(marker_error) = list_header_marker_from_resolved(
                    item,
                    &occurrences,
                    &outcome,
                )
            {
                return Err(CaptureItemParseError::coded(
                    "list_header_marker",
                    marker_error,
                ));
            }
            outcome
        }
        Err(error) => {
            if super::type_marker::has_clock_tag_at_item_edge(item)
                && (error.contains(
                    "`^route:block-id` must be the whole capture item",
                ) || error
                    .contains("Pomodoro link capture cannot be combined"))
            {
                return Err(CaptureItemParseError::coded(
                    "clock_tag_on_existing_task",
                    "capture writes clock tags only on new tasks; edit the task line to add #off or #on"
                        .to_string(),
                ));
            }
            if error == missing_text_error()
                && let Some(marker_error) = list_header_marker_from_item(
                    item,
                    &occurrences,
                    forced_route.is_some(),
                )
            {
                return Err(CaptureItemParseError::coded(
                    "list_header_marker",
                    marker_error,
                ));
            }
            if error == missing_text_error()
                && occurrences.iter().any(|item| item.is_marker())
            {
                return Err(type_marker_only_error(&occurrences));
            }
            if error == missing_text_error()
                && directive_occurrences.iter().any(|item| item.is_marker())
            {
                return Err(directive_only_error(&directive_occurrences));
            }
            if error == super::tokens::INCOMPLETE_SECTION_ERROR {
                return Err(CaptureItemParseError::coded(
                    "incomplete_section",
                    error,
                ));
            }
            if error
                == "add the note text (`Postgres 17 minimum @foo+bar#requirements`)"
            {
                return Err(CaptureItemParseError::coded("drawer_text", error));
            }
            if error
                == "`!` takes no drawer: write `@route+id!` without a `#drawer`"
            {
                return Err(CaptureItemParseError::coded(
                    "drawer_toggle",
                    error,
                ));
            }
            if error.starts_with("invalid drawer: use `#slug`") {
                return Err(CaptureItemParseError::coded(
                    "invalid_drawer",
                    error,
                ));
            }
            if let Some(code) = super::drawer_bullet::code_for_message(&error) {
                return Err(CaptureItemParseError::coded(code, error));
            }
            if let Some(code) = super::move_bullet::code_for_message(&error) {
                return Err(CaptureItemParseError::coded(code, error));
            }
            // An unfinished ` :` / ` ^` names no ID: like the list
            // path, it reports `invalid_project_task_id`. The checkbox
            // copy starts "` :<id>` makes", so the " needs" tail keeps
            // this rule specific to the unfinished shape.
            if (error.starts_with("` :`") || error.starts_with("` ^`"))
                && error.contains(" needs a task ID:")
            {
                return Err(CaptureItemParseError::coded(
                    "invalid_project_task_id",
                    error,
                ));
            }
            if error.starts_with("a new task has no drawers:") {
                return Err(CaptureItemParseError::coded(
                    "invalid_drawer",
                    error,
                ));
            }
            if error.contains("twice on this line; name each task once") {
                return Err(CaptureItemParseError::coded(
                    "duplicate_capture_target",
                    error,
                ));
            }
            if error.contains("are at opposite ends of the line; put every") {
                return Err(CaptureItemParseError::coded(
                    "split_capture_targets",
                    error,
                ));
            }
            // Keep the established parser's diagnostics for malformed
            // underlying forms. Marker-only items are handled above so they
            // never collapse to the generic missing-text copy.
            return Err(CaptureItemParseError::generic(error));
        }
    };

    apply_directives(&mut outcome.parsed, &directive_occurrences)?;
    refuse_clip_on_incompatible_kind(&outcome.parsed)?;

    if super::type_marker::is_existing_task_action(&outcome.parsed)
        && super::type_marker::has_clock_tag_at_item_edge(item)
    {
        return Err(CaptureItemParseError::coded(
            "clock_tag_on_existing_task",
            "capture writes clock tags only on new tasks; edit the task line to add #off or #on"
                .to_string(),
        ));
    }

    let markers = occurrences
        .iter()
        .filter(|occurrence| occurrence.is_marker())
        .collect::<Vec<_>>();
    if markers.len() > 1 {
        return Err(CaptureItemParseError::coded(
            "duplicate_capture_marker",
            duplicate_marker_error("type marker (#<type>)"),
        ));
    }
    let Some(occurrence) = markers.first() else {
        let declarations =
            remap_type_marker_declarations(outcome.declarations, item.source);
        return Ok(ParsedCaptureItemOutcome {
            index: outcome.index,
            start: outcome.start,
            end: outcome.end,
            line_start: outcome.line_start,
            line_end: outcome.line_end,
            parsed: outcome.parsed,
            declarations,
            local_destination_marker: outcome.local_destination_marker,
            list: outcome.list,
            fanout: outcome.fanout,
        });
    };
    let marker = match occurrence {
        super::type_marker::TypeMarkerOccurrence::Complete(marker) => {
            marker.clone()
        }
        super::type_marker::TypeMarkerOccurrence::Incomplete { .. } => {
            return Err(CaptureItemParseError::coded(
                "type_marker_needs_task",
                super::type_marker::needs_task_error(None),
            ));
        }
        super::type_marker::TypeMarkerOccurrence::Invalid { raw, .. } => {
            return Err(CaptureItemParseError::coded(
                "invalid_type_marker",
                super::type_marker::invalid_marker_error(raw),
            ));
        }
        super::type_marker::TypeMarkerOccurrence::Escaped { .. } => {
            unreachable!("escaped occurrences are excluded above")
        }
    };
    if outcome.parsed.body.trim().is_empty()
        && super::type_marker::is_new_task(&outcome.parsed)
    {
        return Err(CaptureItemParseError::coded(
            "type_marker_needs_task",
            super::type_marker::needs_task_error(Some(&marker.name)),
        ));
    }
    if !super::type_marker::is_new_task(&outcome.parsed)
        && !super::type_marker::is_existing_type_marker_subject(&outcome.parsed)
    {
        return Err(CaptureItemParseError::coded(
            "type_marker_not_applicable",
            type_marker_not_applicable_error(
                &marker.name,
                &outcome.parsed.kind,
            ),
        ));
    }
    outcome.parsed.type_marker = Some(marker);
    let declarations =
        remap_type_marker_declarations(outcome.declarations, item.source);
    Ok(ParsedCaptureItemOutcome {
        index: outcome.index,
        start: outcome.start,
        end: outcome.end,
        line_start: outcome.line_start,
        line_end: outcome.line_end,
        parsed: outcome.parsed,
        declarations,
        local_destination_marker: outcome.local_destination_marker,
        list: outcome.list,
        fanout: outcome.fanout,
    })
}

/// Select an executable target run before the whole-item claims in
/// `parse_capture_item` run. The same line selectors used by the semantic
/// parser supply the run and refusal evidence; an item without a run keeps
/// the original single-item path.
pub(super) fn select_target_run_for_item(
    item: &CaptureItem<'_>,
    forced_route: Option<&str>,
    forced_section: Option<&str>,
) -> Result<Option<TargetRunContext>, CaptureItemParseError> {
    if forced_route.is_some() || forced_section.is_some() || item.list.is_some()
    {
        return Ok(None);
    }

    // Match the semantic parser's source-preserving type/directive masks
    // before selecting a route slot.
    let occurrences = super::type_marker::collect_item(item);
    let type_masked_source =
        super::type_marker::masked_source(item, &occurrences);
    let type_masked_item =
        super::type_marker::masked_item(item, &type_masked_source);
    let directive_occurrences =
        super::directive::collect_item(&type_masked_item);
    let masked_source = super::directive::masked_source(
        &type_masked_item,
        &directive_occurrences,
    );
    let mut masked_item =
        super::directive::masked_item(&type_masked_item, &masked_source);
    let marker_only =
        super::type_marker::marker_only_child_lines(item, &occurrences);
    masked_item
        .lines
        .retain(|line| !marker_only.contains(&(line.raw.start, line.raw.end)));
    let Some((parent_line, child_lines)) = masked_item.lines.split_first()
    else {
        return Ok(None);
    };

    let parent_tokens = tokenize_line_with_spans(&parent_line.raw);
    let parent_tokens = super::pomodoro_literal::merge_leading_literal_token(
        &parent_line.raw,
        parent_tokens,
    );
    let (parent_tokens, parent_dependencies) = extract_line_dependencies(
        parent_tokens,
        parent_line.raw.text,
        parent_line.raw.start,
        true,
    );
    let parent_tokens =
        unescape_execution_tokens(parent_tokens, &parent_dependencies.escapes);
    if let Some(result) =
        try_resolve_parent_target_run(parent_tokens.clone(), true)
    {
        return result.map(|(outcome, selection, terminal_spans)| {
            Some(TargetRunContext {
                line: parent_line.line_number,
                selection,
                terminal_markers: outcome.markers,
                terminal_spans,
            })
        });
    }

    // Child runs can follow a project-note task ID that the established
    // semantic pass strips before marker selection. Resolve the parent's
    // ordinary route and mirror that boundary decision here; if the parent
    // already has an unrelated strict error, let the parser report it.
    let parent_route = resolve_line(parent_tokens, true, true)
        .ok()
        .and_then(|outcome| outcome.route.map(|route| route.token.kind));
    let parent_is_project_note =
        matches!(parent_route, Some(CaptureKind::ProjectNote { .. }));
    let child_seals_note = child_lines.iter().any(|line| {
        matches!(
            classify_authored_line(line.raw),
            AuthoredLineClass::Item(authored)
                if child_line_seals_project_note(authored.body)
        )
    });
    let parent_takes_ids = parent_is_project_note || child_seals_note;

    for line in child_lines {
        let authored = match classify_authored_line(line.raw) {
            AuthoredLineClass::EmptyOrPlaceholder
            | AuthoredLineClass::Invalid => continue,
            AuthoredLineClass::Item(authored) => authored,
        };
        let child_line = RawLine {
            text: authored.body,
            start: authored.body_start,
            end: line.raw.end,
        };
        let tokens = tokenize_line_with_spans(&child_line);
        let (mut tokens, dependencies) = extract_line_dependencies(
            tokens,
            child_line.text,
            child_line.start,
            false,
        );
        if parent_takes_ids {
            let _ = strip_trailing_task_id(&mut tokens);
        }
        let tokens = unescape_execution_tokens(tokens, &dependencies.escapes);
        if let Some(result) = try_resolve_child_target_run(tokens, true) {
            return result.map(|(outcome, selection, terminal_spans)| {
                Some(TargetRunContext {
                    line: line.line_number,
                    selection,
                    terminal_markers: outcome.markers,
                    terminal_spans,
                })
            });
        }
    }
    Ok(None)
}

/// Editor target-run probe: the same source-preserving masks and line
/// selectors execution uses, but lexical only. Returns the selected run
/// (when one line holds at least two members, including partial/malformed
/// members) plus any duplicate/split refusal evidence with its exact
/// range. Validation failures never refuse here: each malformed member
/// receives its existing editor needs/diagnostics through per-member
/// parsing. Whole-line child runs fall back to the singleton path, exactly
/// like execution, unless they also refuse.
pub(super) fn find_editor_target_run(
    item: &CaptureItem<'_>,
) -> (
    Option<TargetRunContext>,
    Option<super::target_runs::TargetRefusal>,
) {
    if item.list.is_some() {
        return (None, None);
    }
    let occurrences = super::type_marker::collect_item(item);
    let type_masked_source =
        super::type_marker::masked_source(item, &occurrences);
    let type_masked_item =
        super::type_marker::masked_item(item, &type_masked_source);
    let directive_occurrences =
        super::directive::collect_item(&type_masked_item);
    let masked_source = super::directive::masked_source(
        &type_masked_item,
        &directive_occurrences,
    );
    let mut masked_item =
        super::directive::masked_item(&type_masked_item, &masked_source);
    let marker_only =
        super::type_marker::marker_only_child_lines(item, &occurrences);
    masked_item
        .lines
        .retain(|line| !marker_only.contains(&(line.raw.start, line.raw.end)));
    let Some((parent_line, child_lines)) = masked_item.lines.split_first()
    else {
        return (None, None);
    };

    let parent_tokens = tokenize_line_with_spans(&parent_line.raw);
    let parent_tokens = super::pomodoro_literal::merge_leading_literal_token(
        &parent_line.raw,
        parent_tokens,
    );
    let (parent_tokens, parent_dependencies) = extract_line_dependencies(
        parent_tokens,
        parent_line.raw.text,
        parent_line.raw.start,
        true,
    );
    let parent_tokens =
        unescape_execution_tokens(parent_tokens, &parent_dependencies.escapes);
    if let Some((run, refusal)) = editor_probe_tokens(
        parent_tokens.clone(),
        true,
        parent_line.line_number,
    ) && (run.is_some() || refusal.is_some())
    {
        return (run, refusal);
    }

    let parent_route = resolve_line(parent_tokens, true, true)
        .ok()
        .and_then(|outcome| outcome.route.map(|route| route.token.kind));
    let parent_is_project_note =
        matches!(parent_route, Some(CaptureKind::ProjectNote { .. }));
    let child_seals_note = child_lines.iter().any(|line| {
        matches!(
            classify_authored_line(line.raw),
            AuthoredLineClass::Item(authored)
                if child_line_seals_project_note(authored.body)
        )
    });
    let parent_takes_ids = parent_is_project_note || child_seals_note;

    for line in child_lines {
        let authored = match classify_authored_line(line.raw) {
            AuthoredLineClass::EmptyOrPlaceholder
            | AuthoredLineClass::Invalid => continue,
            AuthoredLineClass::Item(authored) => authored,
        };
        let child_line = RawLine {
            text: authored.body,
            start: authored.body_start,
            end: line.raw.end,
        };
        let tokens = tokenize_line_with_spans(&child_line);
        let (mut tokens, dependencies) = extract_line_dependencies(
            tokens,
            child_line.text,
            child_line.start,
            false,
        );
        if parent_takes_ids {
            let _ = strip_trailing_task_id(&mut tokens);
        }
        let tokens = unescape_execution_tokens(tokens, &dependencies.escapes);
        // Whole-line child runs stay singleton, like execution, unless
        // they refuse.
        if let Some((run, refusal)) =
            editor_probe_tokens(tokens, false, line.line_number)
            && (run.is_some() || refusal.is_some())
        {
            return (run, refusal);
        }
    }
    (None, None)
}

/// Lexical probe for one line's tokens: global declarations leave, terminal
/// markers extract across the run, then the shared refusal/selection
/// helpers decide. Retired priorities defer to the singleton path so the
/// existing diagnostic survives. Single-member selections are not runs.
fn editor_probe_tokens(
    mut tokens: Vec<Token<'_>>,
    leading: bool,
    line_number: usize,
) -> Option<(
    Option<TargetRunContext>,
    Option<super::target_runs::TargetRefusal>,
)> {
    use super::target_runs as runs;
    let _ = take_global_declarations(&mut tokens);
    let (markers, terminal_spans) =
        runs::extract_terminal_markers_crossing_runs(&mut tokens);
    if tokens.is_empty() {
        return Some((None, None));
    }
    let refusal = runs::detect_target_refusals(&tokens, leading, true);
    let has_refusal = !matches!(refusal, runs::TargetRefusal::None);
    let selection = runs::select_target_run(&tokens, leading, true);
    let Some(selection) = selection else {
        if has_refusal {
            return Some((None, Some(refusal)));
        }
        return Some((None, None));
    };
    if selection.members.len() < 2 {
        if has_refusal {
            return Some((None, Some(refusal)));
        }
        return Some((None, None));
    }
    if !leading
        && selection.position == runs::TargetRunPosition::WholeLine
        && !has_refusal
    {
        return Some((None, None));
    }
    // A retired priority extracted from a leading/trailing block belongs
    // to a selected run. For singleton input, leave it to the established
    // editor parser, which may keep a nonterminal `p:N` as literal text.
    let refusal = if has_refusal { Some(refusal) } else { None };
    Some((
        Some(TargetRunContext {
            line: line_number,
            selection,
            terminal_markers: markers,
            terminal_spans,
        }),
        refusal,
    ))
}

/// A route marker plus only item-wide markers on the first line with
/// bullets below: the header must stand alone. Returns the teaching error
/// naming the first stray marker, or `None` when the item is not that
/// shape. Forced destinations keep `@tokens` literal, so they never count.
fn list_header_marker_from_item(
    item: &CaptureItem<'_>,
    occurrences: &[super::type_marker::TypeMarkerOccurrence],
    forced: bool,
) -> Option<String> {
    if forced || item.list.is_some() {
        return None;
    }
    if super::dependencies::scan_item_dependencies(item).has_entries() {
        return None;
    }
    let (parent, children) = item.lines.split_first()?;
    let tokens = tokenize_line_with_spans(&parent.raw);
    let (tokens, _) = extract_line_dependencies(
        tokens,
        parent.raw.text,
        parent.raw.start,
        true,
    );
    let (first, rest) = tokens.split_first()?;
    if parent.raw.text.len() != parent.raw.text.trim_start().len() {
        return None;
    }
    let route = super::tokens::parse_terminal_route_token(first.text)
        .ok()
        .flatten()?;
    if !matches!(route.kind, CaptureKind::Task | CaptureKind::Bullet { .. }) {
        return None;
    }
    // Type markers are masked out of the token stream, so a masked `#fix`
    // reads as a bare route: recover it from the collected occurrences.
    let mut stray: Option<String> = rest
        .split_first()
        .map(|(marker, _)| marker.text.to_string());
    if stray.is_none() {
        stray = occurrences.iter().find_map(|occurrence| {
            if !occurrence.is_marker() {
                return None;
            }
            let span = occurrence.span();
            if span.start < parent.raw.start || span.end > parent.raw.end {
                return None;
            }
            Some(match occurrence {
                super::type_marker::TypeMarkerOccurrence::Complete(marker) => {
                    format!("#{}", marker.name)
                }
                super::type_marker::TypeMarkerOccurrence::Incomplete {
                    ..
                } => "#".to_string(),
                super::type_marker::TypeMarkerOccurrence::Invalid {
                    raw,
                    ..
                } => raw.clone(),
                super::type_marker::TypeMarkerOccurrence::Escaped {
                    ..
                } => return None,
            })
        });
    }
    let stray = stray?;
    if !rest
        .iter()
        .all(|token| is_list_header_marker_token(token.text))
    {
        return None;
    }
    for line in children {
        if let AuthoredLineClass::Item(authored) =
            classify_authored_line(line.raw)
            && authored.depth == AuthoredDepth::First
        {
            return Some(list_header_marker_error(
                first.text,
                &stray,
                authored.body.trim(),
            ));
        }
    }
    None
}

/// Parse one list bullet block: the bullet's body as a later line (task
/// text plus trailing markers, never a leading-route, operator, URL-ref,
/// caret-link, or picker form) with the header's route filled in. Section
/// bullets parse exactly like `<text> @route#section`.
fn parse_list_bullet_item<'a>(
    item: &CaptureItem<'a>,
    clip: Option<ClipRequest>,
    note: bool,
    listen: bool,
    template: bool,
    _options: &CaptureParseOptions<'_>,
) -> Result<ParsedCaptureItemOutcome<'a>, CaptureItemParseError> {
    let coded = |code: &'static str, message: String| {
        CaptureItemParseError::coded(code, message)
    };
    let list = item.list.clone().expect("list bullet item");
    let header_text = item
        .source
        .get(list.header_start..list.header_end)
        .unwrap_or("")
        .trim()
        .to_string();
    let file = format!("{}.md", list.route);
    let notes = list.kind == ListKind::Notes;
    if template {
        return Err(coded(
            "template_list",
            super::draft::template_list_error(),
        ));
    }
    if note {
        let suggestion = list_bullet_marker_suggestion("%n", "", &list.route);
        return Err(coded(
            "list_bullet_marker",
            list_bullet_marker_error(&header_text, &file, "%n", &suggestion),
        ));
    }
    if listen {
        let suggestion = list_bullet_marker_suggestion("%l", "", &list.route);
        return Err(coded(
            "list_bullet_marker",
            list_bullet_marker_error(&header_text, &file, "%l", &suggestion),
        ));
    }
    if list.ownerless {
        let line = &item.lines[0];
        return Err(match classify_authored_line(line.raw) {
            AuthoredLineClass::Item(authored)
                if authored.depth == AuthoredDepth::Nested =>
            {
                coded(
                    "orphaned_nested_bullet",
                    orphaned_nested_bullet_error(line.line_number),
                )
            }
            _ => coded(
                "invalid_child_line",
                invalid_child_line_error(line.line_number),
            ),
        });
    }
    if list.count == 0 {
        // A lone `@route` task header with no section is a note pull
        // (§12.20); `@route#section` alone and notes lists keep the
        // existing `empty_list` refusal.
        if !notes && list.section.is_none() {
            let route = list.route.clone();
            return Ok(parsed_capture_item_outcome(
                item,
                ParsedCaptureText {
                    body: String::new(),
                    clip,
                    route: Some(route.clone()),
                    kind: CaptureKind::NotePull { route },
                    scheduled_offset: None,
                    priority_level: None,
                    sub_bullets: Vec::new(),
                    dependencies: Vec::new(),
                    blocked_tasks: Vec::new(),
                    relations: Vec::new(),
                    dependency_target: None,
                    type_marker: None,
                },
                Vec::new(),
                Some(header_text),
            ));
        }
        return Err(coded("empty_list", empty_list_error(&header_text, notes)));
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
                return Err(coded(
                    "list_bullet_checkbox",
                    list_bullet_checkbox_error(status),
                ));
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
        return Err(coded(
            "drawer_bullet_position",
            drawer_bullet_position_list_error(notes, &example),
        ));
    }
    // Dependency modifiers leave before marker extraction, exactly like the
    // ordinary path; malformed or partial modifiers refuse here.
    let item_dependencies = scan_item_dependencies(item);
    list_bullet_dependency_errors(&item_dependencies)?;
    let mut parent_tokens = tokenize_line_with_spans(&parent_line.raw);
    let (stripped_tokens, parent_line_dependencies) = extract_line_dependencies(
        parent_tokens,
        parent_line.raw.text,
        parent_line.raw.start,
        true,
    );
    parent_tokens = stripped_tokens;
    let stripped = if notes {
        None
    } else {
        strip_trailing_task_id(&mut parent_tokens)
    };
    if let Some(stripped) = &stripped {
        match &stripped.lex {
            TaskIdLex::Invalid { raw, .. } => {
                return Err(coded(
                    "invalid_project_task_id",
                    invalid_project_task_id_charset_error(raw),
                ));
            }
            TaskIdLex::Valid { id, .. } if is_reserved_project_task_id(id) => {
                return Err(coded(
                    "invalid_project_task_id",
                    reserved_project_task_id_error(id),
                ));
            }
            _ => {}
        }
    }
    refuse_list_bullet_affixes(
        &parent_tokens,
        item.source,
        &header_text,
        &file,
        &list.route,
        body_text.trim(),
    )?;
    let parent_tokens = unescape_execution_tokens(
        parent_tokens,
        &parent_line_dependencies.escapes,
    );
    let parent_outcome = resolve_line(parent_tokens, false, true)
        .map_err(CaptureItemParseError::generic)?;
    let mut declarations = global_declarations_from_tokens(
        parent_outcome.declarations,
        parent_line.line_number,
    );
    if parent_outcome.body.is_empty() {
        if stripped.is_some() {
            return Err(coded(
                "invalid_project_task_id",
                empty_project_task_body_error(parent_line.line_number),
            ));
        }
        return Err(CaptureItemParseError::generic(missing_text_error()));
    }
    if let Some(marker_text) = parent_outcome
        .route
        .as_ref()
        .map(|route| route.marker_text.clone())
    {
        let suggestion = list_bullet_marker_suggestion(
            &marker_text,
            parent_outcome.body.trim(),
            &list.route,
        );
        return Err(coded(
            "list_bullet_marker",
            list_bullet_marker_error(
                &header_text,
                &file,
                &marker_text,
                &suggestion,
            ),
        ));
    }
    let (stripped_sigil, stripped_id) = match stripped {
        Some(stripped) => match stripped.lex {
            TaskIdLex::Lone { sigil } => {
                return Err(coded(
                    "invalid_project_task_id",
                    unfinished_list_task_id_error(
                        sigil,
                        parent_outcome.body.trim(),
                    ),
                ));
            }
            TaskIdLex::Valid { sigil, id } => (Some(sigil), Some(id)),
            TaskIdLex::Invalid { .. } => {
                unreachable!("invalid list IDs return above")
            }
        },
        None => (None, None),
    };
    let mut aggregate = AggregateMarkers::default();
    aggregate.clip = clip;
    aggregate
        .absorb(parent_outcome.markers, None)
        .map_err(CaptureItemParseError::generic)?;

    let mut sub_bullets = Vec::new();
    for line in item.lines.iter().skip(1) {
        let line_number = line.line_number;
        let authored = match classify_authored_line(line.raw) {
            AuthoredLineClass::EmptyOrPlaceholder => continue,
            AuthoredLineClass::Invalid => {
                return Err(coded(
                    "invalid_child_line",
                    invalid_child_line_error(line_number),
                ));
            }
            AuthoredLineClass::Item(authored) => authored,
        };
        let child_line = RawLine {
            text: authored.body,
            start: authored.body_start,
            end: line.raw.end,
        };
        let child_tokens = tokenize_line_with_spans(&child_line);
        let (stripped_tokens, child_line_dependencies) =
            extract_line_dependencies(
                child_tokens,
                child_line.text,
                child_line.start,
                false,
            );
        let mut child_tokens = stripped_tokens;
        let child_stripped = if notes {
            None
        } else {
            strip_trailing_task_id(&mut child_tokens)
        };
        if let Some(child_stripped) = &child_stripped {
            match &child_stripped.lex {
                TaskIdLex::Invalid { raw, .. } => {
                    return Err(coded(
                        "invalid_project_task_id",
                        invalid_project_task_id_charset_error(raw),
                    ));
                }
                TaskIdLex::Valid { id, .. }
                    if is_reserved_project_task_id(id) =>
                {
                    return Err(coded(
                        "invalid_project_task_id",
                        reserved_project_task_id_error(id),
                    ));
                }
                TaskIdLex::Valid { .. } => {
                    return Err(coded(
                        "misplaced_project_task_id",
                        misplaced_list_task_id_error(
                            child_stripped.token.text,
                            parent_outcome.body.trim(),
                        ),
                    ));
                }
                TaskIdLex::Lone { sigil } => {
                    return Err(coded(
                        "invalid_project_task_id",
                        unfinished_list_task_id_error(
                            *sigil,
                            parent_outcome.body.trim(),
                        ),
                    ));
                }
            }
        }
        refuse_list_bullet_affixes(
            &child_tokens,
            item.source,
            &header_text,
            &file,
            &list.route,
            parent_outcome.body.trim(),
        )?;
        let child_tokens = unescape_execution_tokens(
            child_tokens,
            &child_line_dependencies.escapes,
        );
        let outcome = resolve_line(child_tokens, false, true)
            .map_err(CaptureItemParseError::generic)?;
        declarations.extend(global_declarations_from_tokens(
            outcome.declarations,
            line_number,
        ));
        if outcome.body.is_empty() {
            if child_line_dependencies.stripped_tokens > 0 {
                continue;
            }
            return Err(coded(
                "empty_child_after_markers",
                empty_child_after_markers_error(line_number),
            ));
        }
        if let Some(marker_text) = outcome
            .route
            .as_ref()
            .map(|route| route.marker_text.clone())
        {
            let suggestion = list_bullet_marker_suggestion(
                &marker_text,
                parent_outcome.body.trim(),
                &list.route,
            );
            return Err(coded(
                "list_bullet_marker",
                list_bullet_marker_error(
                    &header_text,
                    &file,
                    &marker_text,
                    &suggestion,
                ),
            ));
        }
        aggregate
            .absorb(outcome.markers, None)
            .map_err(CaptureItemParseError::generic)?;
        let body = outcome.body;
        if body.is_empty() {
            if child_line_dependencies.stripped_tokens > 0 {
                continue;
            }
            return Err(coded(
                "empty_child_after_markers",
                empty_child_after_markers_error(line_number),
            ));
        }
        // The bullet line is this item's text line, so its nested lines sit
        // one level higher than their lexical depth: exactly as `<text>
        // @route` plus those lines as direct children would parse (the
        // equivalence invariant). A deeper line cannot exist here; four
        // spaces classify `Invalid` and refuse above.
        let depth = match authored.depth {
            AuthoredDepth::Nested => AuthoredDepth::First,
            AuthoredDepth::First => AuthoredDepth::First,
        };
        sub_bullets.push(AuthoredSubBullet {
            body,
            depth,
            task_id: None,
            move_task: None,
            move_choice: None,
            drawer: None,
        });
    }
    let has_creation_modifiers = aggregate.clip.is_some()
        || aggregate.scheduled_offset.is_some()
        || aggregate.priority_level.is_some();
    let (route, kind) = match (notes, stripped_id) {
        (true, _) => (
            Some(list.route.clone()),
            CaptureKind::Bullet {
                section_prefix: list.section.clone(),
                exact: list.exact,
            },
        ),
        (false, Some(id)) if stripped_sigil == Some(':') => (
            Some(list.route.clone()),
            CaptureKind::Pomodoro {
                block_id: id,
                pomodoro_name: None,
                pomodoro_name_is_literal: false,
                start: None,
                close: None,
            },
        ),
        (false, Some(id)) => (
            Some(list.route.clone()),
            CaptureKind::TaskWithBlockId { block_id: id },
        ),
        (false, None) => (Some(list.route.clone()), CaptureKind::Task),
    };
    super::drawer_bullet::finish_execution_drawer_bullets(
        &kind,
        &parent_outcome.body,
        &mut sub_bullets,
    )
    .map_err(CaptureItemParseError::generic)?;
    let (dependencies, dependency_target) = execution_item_dependencies(
        &item_dependencies,
        &kind,
        route.as_deref(),
        parent_outcome.body.is_empty(),
        has_creation_modifiers,
        Some(&header_text),
    )
    .map_err(CaptureItemParseError::generic)?;
    Ok(parsed_capture_item_outcome(
        item,
        ParsedCaptureText {
            body: parent_outcome.body,
            clip: aggregate.clip,
            route,
            kind,
            scheduled_offset: aggregate.scheduled_offset,
            priority_level: aggregate.priority_level,
            sub_bullets,
            dependencies,
            blocked_tasks: execution_blocked_tasks(&item_dependencies),
            relations: execution_relations(&item_dependencies),
            dependency_target,
            type_marker: None,
        },
        declarations,
        Some(header_text),
    ))
}

/// Refuse malformed or partial dependency modifiers on a list bullet before
/// any other list rule, mirroring the ordinary item's dependency gate.
fn list_bullet_dependency_errors(
    found: &ItemDependencySet,
) -> Result<(), CaptureItemParseError> {
    let invalid = [
        found
            .invalid
            .first()
            .map(|invalid| (invalid.start, invalid.message.clone())),
        found
            .blocked_invalid
            .first()
            .map(|invalid| (invalid.start, invalid.message.clone())),
        found
            .relation_invalid
            .first()
            .map(|invalid| (invalid.start, invalid.message.clone())),
    ]
    .into_iter()
    .flatten()
    .min_by_key(|(start, _)| *start)
    .map(|(_, message)| message);
    if let Some(invalid) = invalid {
        return Err(CaptureItemParseError::generic(invalid));
    }
    if found.has_partial_tokens() {
        let mut partials: Vec<(usize, String)> = Vec::new();
        if let Some(partial) = found.partials.first() {
            partials.push((partial.0, incomplete_dependency_error()));
        }
        if let Some(partial) = found.blocked_partials.first() {
            partials.push((partial.0, incomplete_blocked_task_error()));
        }
        if let Some(partial) = found.relation_target_partials.first() {
            partials.push((partial.0, incomplete_relation_target_error()));
        }
        if let Some(partial) = found.relation_label_partials.first() {
            partials.push((partial.0, incomplete_relation_label_error()));
        }
        partials.sort_by_key(|(start, _)| *start);
        if let Some((_, message)) = partials.first() {
            return Err(CaptureItemParseError::generic(message.clone()));
        }
    }
    Ok(())
}

/// Refuse affixes that would route a list bullet elsewhere: a trailing
/// parent selector (`+`/`+query`) or a task completion (`!note:id`).
fn refuse_list_bullet_affixes(
    tokens: &[Token<'_>],
    source: &str,
    header: &str,
    file: &str,
    route: &str,
    body: &str,
) -> Result<(), CaptureItemParseError> {
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
            let suggestion =
                list_bullet_marker_suggestion(token.text, body, route);
            return Err(CaptureItemParseError::coded(
                "list_bullet_marker",
                list_bullet_marker_error(header, file, token.text, &suggestion),
            ));
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
        let suggestion = list_bullet_marker_suggestion(token.text, body, route);
        return Err(CaptureItemParseError::coded(
            "list_bullet_marker",
            list_bullet_marker_error(header, file, token.text, &suggestion),
        ));
    }
    Ok(())
}

/// A resolved bare-Task header with only item-wide markers and bullets
/// below: the header must stand alone. The bare `@route` form stays literal
/// through resolve, so this runs on the success path; the `missing_text`
/// path (sealed `@route#section` markers) is handled separately.
fn list_header_marker_from_resolved(
    item: &CaptureItem<'_>,
    occurrences: &[super::type_marker::TypeMarkerOccurrence],
    outcome: &ParsedCaptureItemOutcome<'_>,
) -> Option<String> {
    if outcome.parsed.route.is_some() {
        return None;
    }
    if super::dependencies::scan_item_dependencies(item).has_entries() {
        return None;
    }
    if !outcome
        .parsed
        .sub_bullets
        .iter()
        .any(|sub| sub.depth == AuthoredDepth::First)
    {
        return None;
    }
    let (parent, _) = item.lines.split_first()?;
    let body = outcome.parsed.body.trim();
    if body.is_empty() {
        return None;
    }
    let mut tokens = parent.raw.text.split_whitespace();
    let first = tokens.next()?;
    if body != first {
        return None;
    }
    let route = super::tokens::parse_terminal_route_token(first)
        .ok()
        .flatten()?;
    if !matches!(route.kind, CaptureKind::Task | CaptureKind::Bullet { .. }) {
        return None;
    }
    let mut rest = tokens.peekable();
    let stray = if let Some(marker) = rest.next() {
        if !is_list_header_marker_token(marker) {
            return None;
        }
        marker.to_string()
    } else {
        occurrences
            .iter()
            .filter(|occurrence| {
                occurrence.is_marker()
                    && occurrence.span().start >= parent.raw.start
                    && occurrence.span().end <= parent.raw.end
            })
            .find_map(|occurrence| match occurrence {
                super::type_marker::TypeMarkerOccurrence::Complete(marker) => {
                    Some(format!("#{}", marker.name))
                }
                super::type_marker::TypeMarkerOccurrence::Incomplete {
                    ..
                } => Some("#".to_string()),
                super::type_marker::TypeMarkerOccurrence::Invalid {
                    raw,
                    ..
                } => Some(raw.clone()),
                super::type_marker::TypeMarkerOccurrence::Escaped {
                    ..
                } => None,
            })?
    };
    for marker in rest {
        if !is_list_header_marker_token(marker) {
            return None;
        }
    }
    let bullet = outcome
        .parsed
        .sub_bullets
        .iter()
        .find(|sub| sub.depth == AuthoredDepth::First)
        .map(|sub| sub.body.trim())?;
    Some(list_header_marker_error(first, &stray, bullet))
}

/// A bare `@` header with list bullets below: the route was never
/// finished, so the draft needs a route instead of filing literally.
/// Column-zero is required; indented lines keep their literal meaning,
/// and forced/template modes bypass this refusal.
fn is_bare_at_list_attempt(item: &CaptureItem<'_>) -> bool {
    let Some((first, rest)) = item.lines.split_first() else {
        return false;
    };
    if first.raw.text.len() != first.raw.text.trim_start().len() {
        return false;
    }
    if first.raw.text.trim() != "@" {
        return false;
    }
    rest.iter().any(|line| {
        matches!(
            super::draft::classify_authored_line(line.raw),
            super::draft::AuthoredLineClass::Item(authored)
                if authored.depth == super::model::AuthoredDepth::First
        )
    })
}

/// Whether a token may follow a list header on its own line: schedule,
/// priority, `%` directives, and `#` type markers. Anything else (prose,
/// routes, selectors, completions) keeps the ordinary reading.
fn is_list_header_marker_token(token: &str) -> bool {
    super::markers::parse_schedule_token(token).is_some()
        || super::markers::parse_priority_token(token).is_some()
        || super::markers::parse_retired_priority_token(token).is_some()
        || token.starts_with('%')
        || token.starts_with('#')
}

fn remap_type_marker_declarations<'a>(
    declarations: Vec<GlobalDeclarationToken<'_>>,
    source: &'a str,
) -> Vec<GlobalDeclarationToken<'a>> {
    declarations
        .into_iter()
        .filter_map(|declaration| {
            let start = declaration.token.start;
            let end = declaration.token.end;
            Some(GlobalDeclarationToken {
                token: Token {
                    text: source.get(start..end)?,
                    start,
                    end,
                },
                line_number: declaration.line_number,
            })
        })
        .collect()
}

fn apply_directives(
    parsed: &mut ParsedCaptureText,
    occurrences: &[super::directive::DirectiveOccurrence],
) -> Result<(), CaptureItemParseError> {
    match super::directive::resolve(occurrences) {
        Ok(directives) => {
            // Template drafts are framed before items are parsed, so a
            // `%t` that reaches an item is not on the draft's first item
            // (or came from a row value).
            if directives.template {
                return Err(CaptureItemParseError::coded(
                    "template_directive_misplaced",
                    super::directive::template_directive_misplaced_error(),
                ));
            }
            if parsed.clip.is_none() {
                parsed.clip = directives.clip;
            }
            Ok(())
        }
        Err((message, code, _)) => {
            Err(CaptureItemParseError::coded(code, message))
        }
    }
}

fn refuse_clip_on_incompatible_kind(
    parsed: &ParsedCaptureText,
) -> Result<(), CaptureItemParseError> {
    if parsed.clip.is_none() {
        return Ok(());
    }
    let message = match &parsed.kind {
        CaptureKind::PomodoroLink { .. } => {
            "Pomodoro link capture cannot be combined with %c"
        }
        CaptureKind::PomodoroAdjust { .. } => {
            "Pomodoro adjustment `+N`/`-N` cannot be combined with %c; capture the adjustment alone"
        }
        CaptureKind::PomodoroShift { .. } => {
            "Pomodoro shift `++N`/`--N` cannot be combined with %c; capture the shift alone"
        }
        CaptureKind::PomodoroSplit { .. } => {
            "Pomodoro split `=/` cannot be combined with %c; capture the split alone"
        }
        CaptureKind::PomodoroMerge { .. } => {
            "Pomodoro merge `=*` cannot be combined with %c; capture the merge alone"
        }
        CaptureKind::PomodoroClose { .. } => {
            "Pomodoro close `x` cannot be combined with %c; capture the close alone"
        }
        CaptureKind::PomodoroPast { .. } => {
            "Pomodoro past `x=` cannot be combined with %c; capture the past alone"
        }
        CaptureKind::PomodoroStart { .. } => {
            "Pomodoro start `=<X>` cannot be combined with %c; capture the start alone"
        }
        CaptureKind::TaskComplete { .. } => {
            "task complete cannot be combined with %c"
        }
        CaptureKind::ProjectNote { .. } => {
            "project-note capture cannot be combined with %c"
        }
        _ => return Ok(()),
    };
    Err(CaptureItemParseError::generic(message.to_string()))
}

fn directive_only_error(
    occurrences: &[super::directive::DirectiveOccurrence],
) -> CaptureItemParseError {
    match super::directive::resolve(occurrences) {
        Err((message, code, _)) => CaptureItemParseError::coded(code, message),
        Ok(directives) if directives.clip.is_some() => {
            CaptureItemParseError::generic(missing_text_error())
        }
        Ok(_) => CaptureItemParseError::generic(missing_text_error()),
    }
}

fn type_marker_only_error(
    occurrences: &[super::type_marker::TypeMarkerOccurrence],
) -> CaptureItemParseError {
    let markers = occurrences
        .iter()
        .filter(|occurrence| occurrence.is_marker())
        .collect::<Vec<_>>();
    if markers.len() > 1 {
        return CaptureItemParseError::coded(
            "duplicate_capture_marker",
            duplicate_marker_error("type marker (#<type>)"),
        );
    }
    match markers.first().copied() {
        Some(super::type_marker::TypeMarkerOccurrence::Complete(marker)) => {
            CaptureItemParseError::coded(
                "type_marker_needs_task",
                super::type_marker::needs_task_error(Some(&marker.name)),
            )
        }
        Some(super::type_marker::TypeMarkerOccurrence::Incomplete {
            ..
        }) => CaptureItemParseError::coded(
            "type_marker_needs_task",
            super::type_marker::needs_task_error(None),
        ),
        Some(super::type_marker::TypeMarkerOccurrence::Invalid {
            raw, ..
        }) => CaptureItemParseError::coded(
            "invalid_type_marker",
            super::type_marker::invalid_marker_error(raw),
        ),
        _ => CaptureItemParseError::generic(missing_text_error()),
    }
}

fn type_marker_not_applicable_error(name: &str, kind: &CaptureKind) -> String {
    super::type_marker::type_marker_not_applicable_error(
        name,
        super::type_marker::refusal_subject_for_kind(kind),
    )
}

/// One accepted move-bullet post-pass outcome.
#[derive(Debug, Clone, PartialEq, Eq)]
enum ExecutionMoveOutcome {
    Move(MoveBullet),
    Choice(MoveChoiceBullet),
}

/// Interpret one move-bullet lexer hit in the project-note post-pass, in
/// source order. Returns the recorded move or choice on success and the
/// refusal message otherwise. The caller clears the bullet body and records
/// the outcome.
#[allow(clippy::too_many_arguments)]
fn interpret_execution_move(
    hit: MoveBulletHit,
    pass: &mut ProjectTaskPass,
    sub_bullets: &[AuthoredSubBullet],
    index: usize,
    line_number: usize,
    route: &str,
    new_id: &str,
) -> Result<ExecutionMoveOutcome, String> {
    let invalid = |kind: &MoveBulletInvalid| -> String {
        match kind {
            MoveBulletInvalid::Text { sigil, id } => {
                super::move_bullet::move_bullet_text_error(*sigil, id, route)
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
        }
    };
    match hit {
        MoveBulletHit::Invalid { kind, .. } => Err(invalid(&kind)),
        MoveBulletHit::Choice { span } => {
            if sub_bullets
                .get(index + 1)
                .is_some_and(|next| next.depth == AuthoredDepth::Nested)
            {
                return Err(super::move_bullet::move_choice_nested_error());
            }
            if let Some(first) = pass.register_choice(line_number) {
                return Err(super::move_bullet::move_choice_duplicate_error(
                    first,
                ));
            }
            Ok(ExecutionMoveOutcome::Choice(MoveChoiceBullet {
                range: (span.start, span.end),
            }))
        }
        MoveBulletHit::Unfinished { sigil, .. } => Err(
            super::move_bullet::move_bullet_unfinished_error(sigil, route),
        ),
        MoveBulletHit::Complete {
            sigil,
            id,
            sigil_span,
            id_span,
            ..
        } => {
            if sub_bullets
                .get(index + 1)
                .is_some_and(|next| next.depth == AuthoredDepth::Nested)
            {
                let basename =
                    crate::native::capture_project_note::project_note_basename(
                        route, new_id,
                    );
                let stem = basename.strip_suffix(".md").unwrap_or(&basename);
                return Err(super::move_bullet::move_bullet_nested_error(
                    stem, &id,
                ));
            }
            match pass.register_move(sigil, &id, line_number) {
                MoveIdOutcome::Accept => {
                    Ok(ExecutionMoveOutcome::Move(MoveBullet {
                        sigil,
                        block_id: Some(id),
                        range: (sigil_span.0, id_span.1),
                        via: super::model::MoveVia::Bullet,
                    }))
                }
                MoveIdOutcome::Duplicate { first_line } => {
                    Err(super::move_bullet::move_bullet_duplicate_error(
                        sigil, &id, first_line,
                    ))
                }
                MoveIdOutcome::Collision { authored_line } => {
                    Err(super::move_bullet::move_bullet_id_collision_error(
                        &id,
                        route,
                        line_number,
                        authored_line,
                    ))
                }
            }
        }
    }
}

fn parse_capture_item_masked<'a>(
    item: &CaptureItem<'a>,
    forced_route: Option<&str>,
    forced_section: Option<&str>,
    options: &CaptureParseOptions<'_>,
    clip: Option<ClipRequest>,
    note: bool,
    listen: bool,
    target_run: Option<&TargetRunContext>,
) -> Result<ParsedCaptureItemOutcome<'a>, String> {
    let Some((parent_line, child_lines)) = item.lines.split_first() else {
        return Err(missing_text_error());
    };
    let detect_route = forced_route.is_none();
    let mut declarations = Vec::new();

    let parent_normalized = normalize_task_text(parent_line.raw.text);
    if parent_normalized.is_empty() {
        return Err(missing_text_error());
    }
    // A whole-item `!` token is claimed before dependency scanning, so
    // a claimed item gets the `!` teaching error rather than a
    // dependency error, matching the editor: queries and padded items
    // fail with teaching errors, and complete tokens become
    // `TaskComplete`. `@@` inheritance skips these items: the kind below
    // is never `Task`, so draft resolution leaves the global declaration
    // alone.
    if let Some(claim) = claim_bang_item(item) {
        match claim {
            BangClaim::Query { raw, .. } => {
                return Err(bang_picker_error(&raw));
            }
            BangClaim::Invalid(invalid) => {
                return Err(invalid.message);
            }
            BangClaim::Complete(dependency) => {
                if forced_route.is_some() || forced_section.is_some() {
                    return Err(bang_invalid_error(
                        &dependency.raw,
                        "its forced destination flags",
                    ));
                }
                return Ok(parsed_capture_item_outcome(
                    item,
                    ParsedCaptureText {
                        body: String::new(),
                        clip: None,
                        route: None,
                        kind: CaptureKind::TaskComplete {
                            raw: dependency.raw.clone(),
                            note: dependency.note.clone(),
                            block_id: dependency.block_id.clone(),
                            quoted: dependency.quoted,
                        },
                        scheduled_offset: None,
                        priority_level: None,
                        sub_bullets: Vec::new(),
                        dependencies: Vec::new(),
                        blocked_tasks: Vec::new(),
                        relations: Vec::new(),
                        dependency_target: None,
                        type_marker: None,
                    },
                    Vec::new(),
                    Some(dependency.raw.clone()),
                ));
            }
        }
    }
    // A whole-draft `/query` Go To search is never executable: it
    // rejects with the T3 teaching error before any other item parser --
    // before the `&` dependency scan, and so before the `:` refusal.
    // Forced flags do not change this.
    if let Some(token) = task_goto_query_token(item) {
        return Err(task_goto_picker_error(token.text));
    }
    // `&note:block-id` prerequisites and `*note:block-id` blocked tasks
    // are staged for the writer:
    // malformed or half-typed modifiers fail here, operator items fail
    // with a targeted diagnostic, and complete modifiers strip from the
    // marker runs below so the established pipeline resolves the
    // remaining tokens unchanged. Ownership is decided once the finished
    // item kind is known; ownerless items retry after `@@` inheritance in
    // draft resolution.
    let item_dependencies = scan_item_dependencies(item);
    let invalid = [
        item_dependencies
            .invalid
            .first()
            .map(|invalid| (invalid.start, invalid.message.clone())),
        item_dependencies
            .blocked_invalid
            .first()
            .map(|invalid| (invalid.start, invalid.message.clone())),
        item_dependencies
            .relation_invalid
            .first()
            .map(|invalid| (invalid.start, invalid.message.clone())),
    ]
    .into_iter()
    .flatten()
    .min_by_key(|(start, _)| *start)
    .map(|(_, message)| message);
    if let Some(invalid) = invalid {
        return Err(invalid);
    }
    if item_dependencies.has_partial_tokens() {
        let mut partials: Vec<(usize, String)> = Vec::new();
        if let Some(partial) = item_dependencies.partials.first() {
            partials.push((partial.0, incomplete_dependency_error()));
        }
        if let Some(partial) = item_dependencies.blocked_partials.first() {
            partials.push((partial.0, incomplete_blocked_task_error()));
        }
        if let Some(partial) =
            item_dependencies.relation_target_partials.first()
        {
            partials.push((partial.0, incomplete_relation_target_error()));
        }
        if let Some(partial) = item_dependencies.relation_label_partials.first()
        {
            partials.push((partial.0, incomplete_relation_label_error()));
        }
        partials.sort_by_key(|(start, _)| *start);
        if let Some((_, message)) = partials.first() {
            return Err(message.clone());
        }
    }
    let has_dependencies = item_dependencies.has_entries();
    let relationship_sigil = item_dependencies.sigil();
    if has_dependencies {
        reject_operator_dependencies(
            item,
            parent_line,
            forced_route,
            forced_section,
            relationship_sigil,
        )?;
    }
    // A `:` picker query is never executable: it rejects with a teaching
    // error before any other item parser, so the whole batch rolls back.
    // Forced flags do not change this.
    if let Some(token) = task_link_query_token(item) {
        let query = token.text.strip_prefix(':').unwrap_or_default();
        if parse_caret_link_token(&format!("^{query}")).is_ok() {
            return Err(task_link_picker_at_error(token.text, query));
        }
        return Err(task_link_picker_error(token.text));
    }
    if let Some(outcome) = parse_pomodoro_equals_item(
        item,
        parent_line,
        forced_route,
        forced_section,
    )? {
        return Ok(outcome);
    }
    // The lone whole-item `+` remains its existing five-minute adjustment.
    // Every other eligible bare-plus selector requires an explicit task
    // choice before capture, so it cannot fall through as inbox prose.
    if let Some(selector) = parent_task_selector_tokens(item)
        .into_iter()
        .find(|selector| !selector.dual_use_adjustment)
    {
        return Err(if has_dependencies {
            parent_task_relate_error(selector.token.text)
        } else {
            parent_task_picker_error(selector.token.text)
        });
    }
    if let Some(outcome) = parse_pomodoro_adjust_item(
        item,
        parent_line,
        forced_route,
        forced_section,
    )? {
        return Ok(outcome);
    }
    if let Some(outcome) = parse_pomodoro_link_item(
        item,
        parent_line,
        forced_route,
        forced_section,
    )? {
        return Ok(outcome);
    }
    if listen && clip.is_some() {
        return Err(super::directive::directive_not_applicable_error());
    }
    if listen && note {
        return Err(super::directive::directive_not_applicable_error());
    }
    if listen {
        if let Some(outcome) = claim_reference_item(
            item,
            options,
            has_dependencies,
            forced_route,
            true,
        ) {
            if clip.is_some() {
                return Err(super::directive::directive_not_applicable_error());
            }
            return Ok(outcome);
        }
        return Err(super::directive::directive_not_applicable_error());
    }
    if note && forced_route.is_some() {
        return Err(pomodoro_note_forced_route_conflict_error());
    }
    if clip.is_none()
        && let Some(outcome) = claim_reference_item(
            item,
            options,
            has_dependencies,
            forced_route,
            false,
        )
    {
        return Ok(outcome);
    }
    // Dependency modifiers leave the token stream before terminal-marker
    // extraction and route selection see it, exactly like the editor
    // line pass: the remaining tokens resolve like a draft without them.
    let parent_tokens = tokenize_line_with_spans(&parent_line.raw);
    let parent_tokens = super::pomodoro_literal::merge_leading_literal_token(
        &parent_line.raw,
        parent_tokens,
    );
    let (parent_tokens, parent_line_dependencies) = extract_line_dependencies(
        parent_tokens,
        parent_line.raw.text,
        parent_line.raw.start,
        true,
    );
    let parent_tokens = unescape_execution_tokens(
        parent_tokens,
        &parent_line_dependencies.escapes,
    );
    let mut parent_outcome = if detect_route {
        match try_resolve_parent_target_run(parent_tokens, detect_route) {
            Some(result) => result.map_err(|error| error.message)?.0,
            None => {
                // Re-tokenize for the singleton fallback: the probe above
                // consumed the original vector.
                let retry = tokenize_line_with_spans(&parent_line.raw);
                let retry =
                    super::pomodoro_literal::merge_leading_literal_token(
                        &parent_line.raw,
                        retry,
                    );
                let (retry, _) = extract_line_dependencies(
                    retry,
                    parent_line.raw.text,
                    parent_line.raw.start,
                    true,
                );
                let retry = unescape_execution_tokens(
                    retry,
                    &parent_line_dependencies.escapes,
                );
                resolve_line(retry, true, detect_route)?
            }
        }
    } else {
        resolve_line(parent_tokens, true, detect_route)?
    };
    if let Some(target_run) = target_run
        && target_run.line == parent_line.line_number
    {
        apply_target_run_terminal_markers(
            &mut parent_outcome.markers,
            &target_run.terminal_markers,
        )?;
    }
    declarations.extend(global_declarations_from_tokens(
        parent_outcome.declarations,
        parent_line.line_number,
    ));
    let parent_body_is_empty = parent_outcome.body.is_empty();
    let parent_is_toggle_candidate = matches!(
        parent_outcome.route.as_ref().map(|route| &route.token.kind),
        Some(
            CaptureKind::SubBullet {
                target: SubBulletTarget::BlockId(_),
                ..
            } | CaptureKind::TaskToggle { .. }
        )
    );
    let parent_is_pomodoro_candidate = matches!(
        parent_outcome.route.as_ref().map(|route| &route.token.kind),
        Some(CaptureKind::Pomodoro { .. })
    );
    // An ownerless dependency item (`&foo:bar` with no local marker)
    // defers its missing-dependent diagnostic until draft resolution
    // retries after `@@` inheritance.
    let defers_dependency_owner = has_dependencies
        && parent_body_is_empty
        && parent_outcome.route.is_none();
    if parent_body_is_empty
        && !parent_is_toggle_candidate
        && !parent_is_pomodoro_candidate
        && !defers_dependency_owner
    {
        return Err(missing_text_error());
    }

    let parent_is_project_note = matches!(
        parent_outcome.route.as_ref().map(|route| &route.token.kind),
        Some(CaptureKind::ProjectNote { .. })
    );
    // The project-note marker may sit on a child line: a sealed marker
    // there routes the whole item, so every sibling takes the ID rule.
    let child_seals_note = child_lines.iter().any(|line| {
        matches!(
            classify_authored_line(line.raw),
            AuthoredLineClass::Item(authored)
                if child_line_seals_project_note(authored.body)
        )
    });
    let parent_takes_ids = parent_is_project_note || child_seals_note;
    let mut aggregate = AggregateMarkers::default();
    aggregate.clip = clip;
    aggregate.absorb(parent_outcome.markers, parent_outcome.route)?;

    let mut sub_bullets = Vec::new();
    let mut sub_bullet_lines = Vec::new();
    let mut sub_bullet_stripped = Vec::new();
    let mut sub_bullet_moves: Vec<Option<MoveBulletHit>> = Vec::new();
    let mut has_first_level_owner = false;
    for line in child_lines {
        let line_number = line.line_number;
        let authored = match classify_authored_line(line.raw) {
            AuthoredLineClass::EmptyOrPlaceholder => continue,
            AuthoredLineClass::Invalid => {
                return Err(invalid_child_line_error(line_number));
            }
            AuthoredLineClass::Item(authored) => authored,
        };
        if authored.depth == AuthoredDepth::Nested && !has_first_level_owner {
            return Err(orphaned_nested_bullet_error(line_number));
        }
        let child_line = RawLine {
            text: authored.body,
            start: authored.body_start,
            end: line.raw.end,
        };
        let tokens = tokenize_line_with_spans(&child_line);
        let (mut tokens, child_line_dependencies) = extract_line_dependencies(
            tokens,
            child_line.text,
            child_line.start,
            false,
        );
        // Project-note task bullets take the list ID rule: the ID may sit
        // anywhere in the trailing marker run, so it leaves before marker
        // extraction and schedule/priority markers behind it still resolve.
        let child_stripped = if parent_takes_ids {
            strip_trailing_task_id(&mut tokens)
        } else {
            None
        };
        let tokens =
            unescape_execution_tokens(tokens, &child_line_dependencies.escapes);
        let mut outcome = if detect_route {
            match try_resolve_child_target_run(tokens, detect_route) {
                Some(result) => result.map_err(|error| error.message)?.0,
                None => {
                    let retry = tokenize_line_with_spans(&child_line);
                    let (mut retry, _) = extract_line_dependencies(
                        retry,
                        child_line.text,
                        child_line.start,
                        false,
                    );
                    if parent_takes_ids {
                        let _ = strip_trailing_task_id(&mut retry);
                    }
                    let retry = unescape_execution_tokens(
                        retry,
                        &child_line_dependencies.escapes,
                    );
                    resolve_line(retry, false, detect_route)?
                }
            }
        } else {
            resolve_line(tokens, false, detect_route)?
        };
        if let Some(target_run) = target_run
            && target_run.line == line_number
        {
            apply_target_run_terminal_markers(
                &mut outcome.markers,
                &target_run.terminal_markers,
            )?;
        }
        declarations.extend(global_declarations_from_tokens(
            outcome.declarations,
            line_number,
        ));
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
        let drawer_hit = (authored.depth == AuthoredDepth::First)
            .then(|| super::drawer_bullet::scan_leading(authored.body, 0))
            .flatten();
        if let Some(hit) = drawer_hit.as_ref() {
            use super::drawer_bullet::DrawerBulletHit;
            match hit {
                DrawerBulletHit::Incomplete { .. } => {
                    return Err(super::drawer_bullet::incomplete_drawer_error());
                }
                DrawerBulletHit::Invalid { raw, .. } => {
                    if raw.starts_with('"')
                        || raw.starts_with('“')
                        || !raw.starts_with('#')
                    {
                        return Err(
                            super::drawer_bullet::invalid_quoted_drawer_error(
                                raw,
                            ),
                        );
                    }
                    return Err(INVALID_DRAWER_ERROR.to_string());
                }
                DrawerBulletHit::Complete { leftover, .. }
                    if !leftover.is_empty() =>
                {
                    return Err(
                        super::drawer_bullet::drawer_bullet_text_error(),
                    );
                }
                DrawerBulletHit::Complete { .. }
                | DrawerBulletHit::Escaped { .. } => {}
            }
        }
        // A stripped task ID defers to the post-pass, which reports the
        // ID refusal in step order (marker, parent, children in source
        // order, then unused `#pomodoro`). The loop below only refuses
        // ID-less emptied children.
        let id_pending = child_stripped.is_some();
        if outcome.body.is_empty() && drawer_hit.is_none() && !id_pending {
            // A child holding only dependency modifiers contributes no
            // empty bullet; any other emptied child stays an error.
            if child_line_dependencies.stripped_tokens > 0 {
                continue;
            }
            return Err(empty_child_after_markers_error(line_number));
        }
        aggregate.absorb(outcome.markers, outcome.route)?;
        let (body, drawer) = match drawer_hit {
            Some(super::drawer_bullet::DrawerBulletHit::Escaped { .. }) => (
                super::drawer_bullet::unescape_escaped_body(&outcome.body),
                None,
            ),
            Some(super::drawer_bullet::DrawerBulletHit::Complete {
                selector,
                title,
                ..
            }) => (title, Some(selector)),
            Some(_) => (outcome.body, None),
            None => (outcome.body, None),
        };
        if body.is_empty() && drawer.is_none() && !id_pending {
            if child_line_dependencies.stripped_tokens > 0 {
                continue;
            }
            return Err(empty_child_after_markers_error(line_number));
        }
        sub_bullets.push(AuthoredSubBullet {
            body,
            depth: authored.depth,
            task_id: None,
            move_task: None,
            move_choice: None,
            drawer,
        });
        sub_bullet_lines.push(line_number);
        sub_bullet_stripped.push(child_stripped);
        sub_bullet_moves.push(move_hit);
        if authored.depth == AuthoredDepth::First {
            has_first_level_owner = true;
        }
    }

    if (forced_route.is_some() || forced_section.is_some())
        && !parent_is_pomodoro_candidate
    {
        let trimmed = parent_line.raw.text.trim();
        if trimmed.split_whitespace().count() == 1
            && is_pomodoro_marker_candidate(trimmed)
            && parse_pomodoro_route_token(trimmed).is_ok()
            && item.lines.len() == 1
        {
            return Err(
                "Pomodoro link capture cannot be combined with --route, --section, --task, --task-ref, --drawer, or --clip; capture the link alone".to_string(),
            );
        }
    }

    if parent_is_pomodoro_candidate && parent_body_is_empty {
        let marker_text = aggregate
            .route
            .as_ref()
            .map(|route| route.marker_text.clone());
        let extras = aggregate.clip.is_some()
            || aggregate.scheduled_offset.is_some()
            || aggregate.priority_level.is_some()
            || !sub_bullets.is_empty();
        if has_dependencies
            && extras
            && !owner_marker_is_suffixed(marker_text.as_deref().unwrap_or(""))
        {
            let parent_tokens = tokenize_line_with_spans(&parent_line.raw);
            return Err(refuse_relationship_needs_text(
                &item_dependencies,
                marker_text.as_deref().unwrap_or("@route:id"),
                &parent_tokens,
                aggregate.clip.is_some(),
                !sub_bullets.is_empty(),
            ));
        }
        if forced_route.is_some() || forced_section.is_some() {
            return Err(
                "Pomodoro link capture cannot be combined with --route, --section, --task, --task-ref, --drawer, or --clip; capture the link alone".to_string(),
            );
        }
        if !has_dependencies {
            if aggregate.clip.is_some() {
                return Err("Pomodoro link capture cannot be combined with %c"
                    .to_string());
            }
            if aggregate.scheduled_offset.is_some() {
                return Err(
                    "Pomodoro link capture cannot be combined with s:<N>"
                        .to_string(),
                );
            }
            if aggregate.priority_level.is_some() {
                return Err(
                    "Pomodoro link capture cannot be combined with $<N>"
                        .to_string(),
                );
            }
            if !sub_bullets.is_empty() {
                return Err(
                    "Pomodoro link capture cannot be combined with authored child bullets".to_string(),
                );
            }
        }
        let (route, kind) = match aggregate.route {
            Some(line_route) => (line_route.token.route, line_route.token.kind),
            None => (None, CaptureKind::Task),
        };
        let CaptureKind::Pomodoro {
            block_id,
            pomodoro_name,
            pomodoro_name_is_literal,
            start,
            close,
        } = kind
        else {
            return Err(missing_text_error());
        };
        let spelling = if marker_text
            .as_deref()
            .is_some_and(|marker| marker.starts_with('^'))
        {
            PomodoroLinkSpelling::Caret
        } else {
            PomodoroLinkSpelling::At
        };
        let link_kind = CaptureKind::PomodoroLink {
            block_id,
            pomodoro_name,
            pomodoro_name_is_literal,
            start,
            close,
            spelling,
        };
        // Bare `@route:id` / `^route:id` aliases select the same existing
        // dependent as `@route+id`; a suffixed link rejects.
        let (dependencies, dependency_target) = execution_item_dependencies(
            &item_dependencies,
            &link_kind,
            route.as_deref(),
            true,
            false,
            marker_text.as_deref(),
        )?;
        return Ok(parsed_capture_item_outcome(
            item,
            ParsedCaptureText {
                body: String::new(),
                clip: None,
                route,
                kind: link_kind,
                scheduled_offset: None,
                priority_level: None,
                sub_bullets: Vec::new(),
                dependencies,
                blocked_tasks: execution_blocked_tasks(&item_dependencies),
                relations: execution_relations(&item_dependencies),
                dependency_target,
                type_marker: None,
            },
            declarations,
            marker_text,
        ));
    }

    if let Some(section) = forced_section {
        let Some(route) = forced_route else {
            return Err("--section requires --route".to_string());
        };
        if section.trim().is_empty() {
            return Err("--section must not be empty".to_string());
        }
        let route = normalize_forced_route(route)?;
        if has_dependencies {
            return Err(unsupported_target_relation_error(
                "section-bullet",
                relationship_sigil,
            ));
        }
        let kind = CaptureKind::Bullet {
            section_prefix: Some(section.to_string()),
            exact: true,
        };
        super::drawer_bullet::finish_execution_drawer_bullets(
            &kind,
            &parent_outcome.body,
            &mut sub_bullets,
        )?;
        return Ok(parsed_capture_item_outcome(
            item,
            ParsedCaptureText {
                body: parent_outcome.body,
                clip: aggregate.clip,
                route: Some(route),
                kind,
                scheduled_offset: aggregate.scheduled_offset,
                priority_level: aggregate.priority_level,
                sub_bullets,
                dependencies: Vec::new(),
                blocked_tasks: Vec::new(),
                relations: Vec::new(),
                dependency_target: None,
                type_marker: None,
            },
            declarations,
            aggregate
                .route
                .as_ref()
                .map(|route| route.marker_text.clone()),
        ));
    }

    if let Some(route) = forced_route {
        let route = normalize_forced_route(route)?;
        // `--route` routes a new task with dependencies; it never turns
        // an ownerless modifier into an existing-task update.
        let (dependencies, dependency_target) = if has_dependencies {
            if parent_outcome.body.is_empty() && sub_bullets.is_empty() {
                return Err(ownerless_relation_error(relationship_sigil));
            }
            (
                execution_dependencies(&item_dependencies),
                Some(ParsedDependencyTarget {
                    kind: ParsedDependencyTargetKind::NewTask,
                    route: Some(route.clone()),
                    block_id: None,
                    inherited: false,
                }),
            )
        } else {
            (Vec::new(), None)
        };
        super::drawer_bullet::finish_execution_drawer_bullets(
            &CaptureKind::Task,
            &parent_outcome.body,
            &mut sub_bullets,
        )?;
        return Ok(parsed_capture_item_outcome(
            item,
            ParsedCaptureText {
                body: parent_outcome.body,
                clip: aggregate.clip,
                route: Some(route),
                kind: CaptureKind::Task,
                scheduled_offset: aggregate.scheduled_offset,
                priority_level: aggregate.priority_level,
                sub_bullets,
                dependencies,
                blocked_tasks: execution_blocked_tasks(&item_dependencies),
                relations: execution_relations(&item_dependencies),
                dependency_target,
                type_marker: None,
            },
            declarations,
            aggregate
                .route
                .as_ref()
                .map(|route| route.marker_text.clone()),
        ));
    }

    let local_destination_marker = aggregate
        .route
        .as_ref()
        .map(|route| route.marker_text.clone());
    if note && aggregate.route.is_some() {
        return Err(pomodoro_note_route_conflict_error());
    }
    let (route, kind) = match aggregate.route {
        Some(line_route) => (line_route.token.route, line_route.token.kind),
        None => (None, CaptureKind::Task),
    };
    let kind = if note {
        CaptureKind::PomodoroNote
    } else {
        kind
    };
    let route = if note { None } else { route };
    // Schedule, priority, and `%c` stay task-creation modifiers: a
    // dependency-only item carrying them mixes a second
    // action into an existing-task update instead of failing silently.
    let has_creation_modifiers = aggregate.clip.is_some()
        || aggregate.scheduled_offset.is_some()
        || aggregate.priority_level.is_some();
    // A note-less `@route+id` item with first-level bullets is the
    // multi-note form: its bullets are notes, not children requiring
    // parent text, and `&`/`*`/`~` keep the target as owner.
    let is_multi_note_candidate = matches!(kind, CaptureKind::SubBullet { .. })
        && parent_body_is_empty
        && sub_bullets
            .iter()
            .any(|bullet| bullet.depth == AuthoredDepth::First);
    if has_dependencies
        && parent_body_is_empty
        && (has_creation_modifiers || !sub_bullets.is_empty())
        && !owner_marker_is_suffixed(
            local_destination_marker.as_deref().unwrap_or(""),
        )
        && !is_multi_note_candidate
    {
        let parent_tokens = tokenize_line_with_spans(&parent_line.raw);
        return Err(refuse_relationship_needs_text(
            &item_dependencies,
            local_destination_marker.as_deref().unwrap_or("@route+id"),
            &parent_tokens,
            aggregate.clip.is_some(),
            !sub_bullets.is_empty(),
        ));
    }
    let kind = resolve_sub_bullet_kind(
        kind,
        parent_body_is_empty,
        sub_bullets.is_empty(),
        aggregate.scheduled_offset,
        aggregate.priority_level,
        aggregate.clip.is_some(),
    )?;
    if matches!(kind, CaptureKind::PomodoroNote) {
        if aggregate.scheduled_offset.is_some() {
            return Err(pomodoro_note_schedule_conflict_error());
        }
        if aggregate.priority_level.is_some() {
            return Err(pomodoro_note_priority_conflict_error());
        }
    }
    if let CaptureKind::Pomodoro { start: Some(_), .. } = &kind {
        if aggregate.scheduled_offset.is_some() {
            return Err(POMODORO_START_SCHEDULE_CONFLICT_ERROR.to_string());
        }
        if aggregate.priority_level.is_some() {
            return Err(POMODORO_START_PRIORITY_CONFLICT_ERROR.to_string());
        }
    }
    if let CaptureKind::Pomodoro { close: Some(_), .. } = &kind {
        if aggregate.scheduled_offset.is_some() {
            return Err(POMODORO_CLOSE_SCHEDULE_CONFLICT_ERROR.to_string());
        }
        if aggregate.priority_level.is_some() {
            return Err(POMODORO_CLOSE_PRIORITY_CONFLICT_ERROR.to_string());
        }
    }
    // A project-note `#pomodoro` name picks the Pomodoro that ` :<id>`
    // Task Links go under. The trailing-ID post-pass below strips accepted
    // IDs from bodies and records them on the sub-bullets; a name with no
    // ` :` task stays unused. Move bullets (`- ^<id>` / `- :<id>`) are
    // interpreted first from their lexer hits, in the same source order, so
    // a claimed line never reaches the new-task path.
    let mut has_link_task = false;
    if matches!(kind, CaptureKind::ProjectNote { .. }) {
        let mut pass = ProjectTaskPass::new();
        if let Some(message) = pass.check_parent(&parent_outcome.body) {
            return Err(message);
        }
        let route_name = route.as_deref();
        let new_id = match &kind {
            CaptureKind::ProjectNote { block_id, .. } => {
                Some(block_id.as_str())
            }
            _ => None,
        };
        let mut owner_body: Option<String> = None;
        for index in 0..sub_bullets.len() {
            let line_number = sub_bullet_lines[index];
            if sub_bullets[index].depth == AuthoredDepth::First {
                let (_, body) = split_leading_checkbox(
                    sub_bullets[index].body.trim_start(),
                );
                owner_body = Some(body.trim().to_string());
            }
            if route_name.is_some()
                && let Some(hit) = sub_bullet_moves[index].clone()
            {
                let route_name =
                    route_name.expect("gated on route presence above");
                match interpret_execution_move(
                    hit,
                    &mut pass,
                    &sub_bullets,
                    index,
                    line_number,
                    route_name,
                    new_id.unwrap_or_default(),
                ) {
                    Ok(ExecutionMoveOutcome::Move(move_bullet)) => {
                        sub_bullets[index].body.clear();
                        sub_bullets[index].move_task = Some(move_bullet);
                    }
                    Ok(ExecutionMoveOutcome::Choice(choice)) => {
                        sub_bullets[index].body.clear();
                        sub_bullets[index].move_choice = Some(choice);
                    }
                    Err(message) => return Err(message),
                }
                continue;
            }
            let stripped = sub_bullet_stripped.get(index).cloned().flatten();
            match pass.check_child(
                stripped,
                &sub_bullets[index].body,
                sub_bullets[index].depth,
                line_number,
            ) {
                ChildTaskOutcome::Ignore => {}
                ChildTaskOutcome::Unfinished { sigil } => {
                    return Err(unfinished_project_task_id_error(
                        sigil,
                        owner_body
                            .as_deref()
                            .unwrap_or(&sub_bullets[index].body),
                    ));
                }
                ChildTaskOutcome::Error { message, .. } => {
                    return Err(message);
                }
                ChildTaskOutcome::Accept { sigil, id } => {
                    if let Some(move_line) = pass.move_line_for(&id) {
                        return Err(
                            super::move_bullet::move_bullet_id_collision_error(
                                &id,
                                route_name.unwrap_or_default(),
                                move_line,
                                line_number,
                            ),
                        );
                    }
                    sub_bullets[index].task_id = Some(ProjectTaskId {
                        block_id: id,
                        link: sigil == ':',
                    });
                }
            }
        }
        has_link_task = pass.has_link;
    }
    if let CaptureKind::ProjectNote {
        pomodoro_name: Some(name),
        form,
        ..
    } = &kind
        && !has_link_task
        && *form == super::model::ProjectNoteForm::Create
    {
        return Err(unused_project_note_pomodoro_error(name));
    }
    super::drawer_bullet::finish_execution_drawer_bullets(
        &kind,
        &parent_outcome.body,
        &mut sub_bullets,
    )?;
    let (dependencies, dependency_target) = execution_item_dependencies(
        &item_dependencies,
        &kind,
        route.as_deref(),
        parent_outcome.body.is_empty(),
        has_creation_modifiers,
        local_destination_marker.as_deref(),
    )?;
    Ok(parsed_capture_item_outcome(
        item,
        ParsedCaptureText {
            body: parent_outcome.body,
            clip: aggregate.clip,
            route,
            kind,
            scheduled_offset: aggregate.scheduled_offset,
            priority_level: aggregate.priority_level,
            sub_bullets,
            dependencies,
            blocked_tasks: execution_blocked_tasks(&item_dependencies),
            relations: execution_relations(&item_dependencies),
            dependency_target,
            type_marker: None,
        },
        declarations,
        local_destination_marker,
    ))
}

/// Finish dependency ownership after `@@` inheritance: a locally owned
/// new task keeps its target but adopts an inherited route, and an
/// ownerless item retries against the inherited parent (or reports the
/// ownerless diagnostic). `inherited` is true when a global destination
/// was eligible to apply to this item.
pub(super) fn finish_item_dependency_target(
    parsed: &mut ParsedCaptureText,
    inherited: bool,
) -> Result<(), String> {
    if parsed.dependencies.is_empty()
        && parsed.blocked_tasks.is_empty()
        && parsed.relations.is_empty()
    {
        return Ok(());
    }
    if let Some(target) = parsed.dependency_target.as_mut() {
        if target.kind == ParsedDependencyTargetKind::NewTask
            && !target.inherited
            && target.route.is_none()
            && let Some(route) = parsed.route.clone()
        {
            target.route = Some(route);
            target.inherited = inherited;
        }
        return Ok(());
    }
    // Ownerless sigil prefers `&`, then `*`, then `~` by earliest start.
    // `ParsedCaptureText` has no positions, so prefer `~` when only
    // relations name an owner.
    let ownerless_sigil = if !parsed.dependencies.is_empty() {
        b'&'
    } else if !parsed.blocked_tasks.is_empty() {
        b'*'
    } else {
        b'~'
    };
    if let Some(target) = finish_inherited_dependency_target(
        parsed.route.as_deref(),
        &parsed.kind,
        parsed.body.is_empty(),
        false,
        true,
        inherited,
        ownerless_sigil,
    )? {
        parsed.dependency_target = Some(target);
        return Ok(());
    }
    Err(ownerless_relation_error(ownerless_sigil))
}

/// Complete modifiers plus ownership for an execution item: empty when
/// the item carries none, otherwise the typed modifiers with the target
/// resolved from the finished kind (or deferred for `@@` inheritance).
pub(super) fn execution_item_dependencies(
    found: &ItemDependencySet,
    kind: &CaptureKind,
    route: Option<&str>,
    body_empty: bool,
    has_creation_modifiers: bool,
    local_marker_text: Option<&str>,
) -> Result<(Vec<ParsedDependency>, Option<ParsedDependencyTarget>), String> {
    if !found.has_entries() {
        return Ok((Vec::new(), None));
    }
    if matches!(kind, CaptureKind::Pomodoro { close: Some(_), .. })
        && !found.blocked_entries.is_empty()
    {
        return Err(close_blocked_task_error(
            local_marker_text.unwrap_or("@route:id=x"),
        ));
    }
    let target = resolve_execution_ownership(
        kind,
        route,
        body_empty,
        has_creation_modifiers,
        local_marker_text,
        found.sigil(),
    )?;
    Ok((execution_dependencies(found), target))
}

/// Decide whether a resolved `@route+block-id[#<type>]` marker stays an
/// ordinary sub-bullet capture or becomes a task toggle, once the finished
/// item -- not just the marker's own token -- is known. A toggle needs an
/// empty parent body, no authored children, and no other item-wide marker;
/// an item that looks like a toggle but fails one of those still needs text,
/// exactly like it always has. Every other kind passes through unchanged.
pub(super) fn resolve_sub_bullet_kind(
    kind: CaptureKind,
    parent_body_is_empty: bool,
    sub_bullets_is_empty: bool,
    scheduled_offset: Option<u64>,
    priority_level: Option<u64>,
    clip_present: bool,
) -> Result<CaptureKind, String> {
    let no_other_item_markers =
        scheduled_offset.is_none() && priority_level.is_none() && !clip_present;
    // A multi-note item decorates one note per bullet, so item-wide
    // single-note markers are refused with a teaching error.
    let multi_note_marker_error = || {
        if let Some(offset) = scheduled_offset {
            super::drawer_bullet::multi_note_schedule_error(offset)
        } else if let Some(level) = priority_level {
            super::drawer_bullet::multi_note_priority_error(level)
        } else {
            super::drawer_bullet::multi_note_clip_error()
        }
    };
    if matches!(kind, CaptureKind::TaskToggle { .. }) {
        if parent_body_is_empty && sub_bullets_is_empty && no_other_item_markers
        {
            return Ok(kind);
        }
        return Err(EXPLICIT_TOGGLE_ONLY_MARKER_ERROR.to_string());
    }
    let CaptureKind::SubBullet { target, section } = kind else {
        return Ok(kind);
    };
    // Clipboard notes: a note-less `@route+id` (or `@route+id#drawer`)
    // whose only extra is `%c` files the clipboard as notes directly
    // under that task. Item-wide single-note markers still refuse with
    // the multi-note teaching error.
    if parent_body_is_empty && sub_bullets_is_empty && clip_present {
        if let Some(drawer) = section.as_ref()
            && drawer.text.is_empty()
        {
            return Err(
                "`#` is incomplete: add a drawer (`#requirements`), or write `\\#` to keep a literal `#`"
                    .to_string(),
            );
        }
        if scheduled_offset.is_some() || priority_level.is_some() {
            return Err(multi_note_marker_error());
        }
        return Ok(CaptureKind::SubBullet { target, section });
    }
    // A glued `#drawer` suffix always stays a drawer capture, never a
    // toggle. Plain `@route+id` without a drawer keeps the old
    // Ensure-Next toggle when the body is empty. A note-less item with
    // first-level bullets is the multi-note form: each bullet becomes
    // its own note.
    if let Some(drawer) = section {
        if drawer.text.is_empty() {
            return Err(
                "`#` is incomplete: add a drawer (`#requirements`), or write `\\#` to keep a literal `#`"
                    .to_string(),
            );
        }
        if parent_body_is_empty {
            if sub_bullets_is_empty {
                return Err(
                    "add the note text (`Postgres 17 minimum @foo+bar#requirements`)"
                        .to_string(),
                );
            }
            if !no_other_item_markers {
                return Err(multi_note_marker_error());
            }
            return Ok(CaptureKind::SubBullet {
                target,
                section: Some(drawer),
            });
        }
        return Ok(CaptureKind::SubBullet {
            target,
            section: Some(drawer),
        });
    }
    // No drawer: plain child bullet. Empty bodies become Ensure-Next
    // toggles for plain block IDs; Ref targets stay sub-bullets.
    if parent_body_is_empty && sub_bullets_is_empty && no_other_item_markers {
        if let SubBulletTarget::BlockId(block_id) = target {
            return Ok(CaptureKind::TaskToggle {
                block_id,
                intent: TaskToggleIntent::EnsureNext,
            });
        }
        return Ok(CaptureKind::SubBullet {
            target,
            section: None,
        });
    }
    if parent_body_is_empty {
        if !sub_bullets_is_empty {
            if !no_other_item_markers {
                return Err(multi_note_marker_error());
            }
            return Ok(CaptureKind::SubBullet {
                target,
                section: None,
            });
        }
        return Err(missing_text_error());
    }
    Ok(CaptureKind::SubBullet {
        target,
        section: None,
    })
}

pub(super) fn parsed_capture_item_outcome<'a>(
    item: &CaptureItem<'a>,
    parsed: ParsedCaptureText,
    declarations: Vec<GlobalDeclarationToken<'a>>,
    local_destination_marker: Option<String>,
) -> ParsedCaptureItemOutcome<'a> {
    ParsedCaptureItemOutcome {
        index: item.index,
        start: item.start,
        end: item.end,
        line_start: item.line_start,
        line_end: item.line_end,
        parsed,
        declarations,
        local_destination_marker,
        list: item.list.clone(),
        fanout: None,
    }
}

/// One small lexer for whole-item Pomodoro session operators: one sign
/// resizes, two identical signs shift, and the ASCII-digit count is
/// optional. Returns the operator, the digit text, and the token length in
/// bytes, or `None` when `text` does not start with an operator token.
///
/// A token is a sign run of exactly `+`, `-`, `++`, or `--` followed by
/// zero or more ASCII digits. Longer or mixed runs (`+++`, `---`, `+-`,
/// `-+3`) are not tokens, so they stay ordinary prose.
pub(crate) fn session_operator_token(
    text: &str,
) -> Option<(SessionOperator, &str, usize)> {
    let bytes = text.as_bytes();
    if bytes.len() >= 2 && text.starts_with("++") {
        if bytes
            .get(2)
            .is_some_and(|byte| *byte == b'+' || *byte == b'-')
        {
            return None;
        }
        let mut len = 2;
        while len < bytes.len() && bytes[len].is_ascii_digit() {
            len += 1;
        }
        return Some((
            SessionOperator::Shift { later: true },
            &text[2..len],
            len,
        ));
    }
    if bytes.len() >= 2 && text.starts_with("--") {
        if bytes
            .get(2)
            .is_some_and(|byte| *byte == b'+' || *byte == b'-')
        {
            return None;
        }
        let mut len = 2;
        while len < bytes.len() && bytes[len].is_ascii_digit() {
            len += 1;
        }
        return Some((
            SessionOperator::Shift { later: false },
            &text[2..len],
            len,
        ));
    }
    if bytes
        .first()
        .is_some_and(|byte| *byte == b'+' || *byte == b'-')
    {
        if bytes
            .get(1)
            .is_some_and(|byte| *byte == b'+' || *byte == b'-')
        {
            return None;
        }
        let plus = bytes[0] == b'+';
        let mut len = 1;
        while len < bytes.len() && bytes[len].is_ascii_digit() {
            len += 1;
        }
        return Some((SessionOperator::Resize { plus }, &text[1..len], len));
    }
    None
}

/// Byte length of a past unquoted name part: ends at whitespace, `!`,
/// `=`, `@`, or `~`. Unlike [`start_name_extent`], `!` and `=` also end
/// the name so `x=fix!2` keeps `fix` as the name and `!2` as the glued
/// suffix.
pub(crate) fn past_name_extent(text: &str) -> usize {
    text.find(|character: char| {
        character.is_ascii_whitespace()
            || character == '!'
            || character == '='
            || character == '@'
            || character == '~'
    })
    .unwrap_or(text.len())
}

/// Byte length of the `x=` past token at the start of `text`, including a
/// spaced exact-name literal through its closing quote plus glued `@D`,
/// `!`, and `=X` suffixes. Returns `None` when `text` does not start with
/// lowercase `x=`. Otherwise claims the token up to whitespace (or through
/// the literal), even when malformed, so every `x=` prefix becomes a past
/// token instead of prose.
pub(crate) fn past_token_len(text: &str) -> Option<usize> {
    if !text.starts_with("x=") {
        return None;
    }
    let first_len = text
        .split_whitespace()
        .next()
        .map(str::len)
        .unwrap_or(text.len());
    let first = &text[..first_len.min(text.len())];
    if let Some(hash_pos) = first.find('#') {
        let after_hash_in_first = &first[hash_pos + 1..];
        if opens_exact_name(after_hash_in_first) {
            let after_hash_full = &text[hash_pos + 1..];
            if let Some(lex) =
                lex_exact_name_after_hash(after_hash_full, after_hash_full.len())
            {
                let literal_end = hash_pos + 1 + lex.literal_len;
                let after_literal = &text[literal_end..];
                let suffix_len = after_literal
                    .find(|c: char| c.is_ascii_whitespace())
                    .unwrap_or(after_literal.len());
                return Some(literal_end + suffix_len);
            }
        }
    }
    Some(first_len)
}

/// A whole-item session token: either a close (a bare `x`, or a retired
/// `=x`/`=*`/`=!` spelling that teaches it), a split (`=/` plus an
/// optional bare base plus `#name`/`#"Literal"` selectors), a past
/// (`x=` plus its `E` part, optional repair `!`, and optional `=X` close
/// part), or a start (`=` plus an `<X>`-shaped suffix and optional `#name`
/// or bare `name` part).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum EqualsToken {
    /// A whole-item `=/` Pomodoro split: `base` is the optional bare base
    /// name after `=/`, `selectors` are the ordered `#type` parts in typed
    /// order, and `len` is the token's byte length including the sigil.
    Split {
        base: Option<String>,
        selectors: Vec<PomodoroSplitSelector>,
        len: usize,
    },
    /// A whole-item `=*` Pomodoro merge: `src` is the optional bare or
    /// quoted source after `=*`, `dests` are the ordered `#name` parts,
    /// and `len` is the token's byte length including the sigil.
    Merge {
        src: Option<String>,
        dests: Vec<PomodoroSplitSelector>,
        len: usize,
    },
    /// A leading bare lowercase `x`, one of its claimed selection/hash near
    /// misses, or a retired `=x`/`=X`/`=*`/`=!` spelling. The retired
    /// spellings stay claimed so they fail with a teaching error instead of
    /// becoming a task.
    Close,
    /// A leading lowercase `x=` past token. `len` is the token's byte length
    /// including `x=`, the `E` part, an optional repair `!`, and an optional
    /// second `=` plus its close selection. The length runs through a quoted
    /// exact-name literal's closing quote (spaces inside included) plus any
    /// glued `@D`, `!`, and `=X` suffixes. Every `x=` prefix is claimed,
    /// valid or not, so the whole-item parser reports `invalid_pomodoro_past`
    /// instead of falling through to prose. Detailed `E` validation lives in
    /// the shared past lexer used by both execution and editor adapters.
    Past { len: usize },
    /// `=` plus the longest `[0-9]*(-[0-9]*)?` run, plus an optional
    /// `#name` or bare `name` part, plus optional `@<X>` timing and `~<K>`
    /// drop parts in either order. `suffix` excludes the sigil (the leading
    /// `<X>`), `counted` is true when the effective `<X>` (leading or `@`)
    /// holds at least one digit, `name` is `Some` when a `#` immediately
    /// follows the suffix (possibly empty, as in `=#`) or a bare name
    /// follows an empty suffix, `name_spelling` records Hash vs Bare so
    /// callers do not recompute `1 + suffix.len() + 1`, `drop` holds the
    /// `~` offset inside the token plus the text after it, `timing` holds
    /// the `@` offset plus the text after it, `len` is the token's byte
    /// length including the sigil, the name, and both optional parts, and
    /// `doubled` is true when the token used the doubled `==` sigil, which
    /// is now retired. A name ends at the first ASCII whitespace, `~`, or
    /// `@`. A `#"NAME"` / `#“NAME”` exact-name literal instead runs through
    /// its closing quote (spaces inside included); `name_is_literal` is
    /// true then and `name` holds the canonical name (or the decoded text
    /// when it names nothing valid).
    Start {
        suffix: String,
        counted: bool,
        len: usize,
        name: Option<String>,
        name_spelling: Option<NameSpelling>,
        name_is_literal: bool,
        drop: Option<(usize, String)>,
        timing: Option<(usize, String)>,
        doubled: bool,
    },
}

/// Whether the text after a leading `=` is itself a claimed retired
/// whole-item close token (`=x`, `=x<N>…`, `=x#…`, `=*…`, `=!…`): only then
/// does a `==`-prefixed token become the retired `==x` close error. Other
/// doubled starts stay ordinary prose unless their token claims its item,
/// which protects Obsidian `==highlight==` prose.
fn double_close_tail_is_claimed(after_first: &str) -> bool {
    let Some(first) = after_first.split_whitespace().next() else {
        return false;
    };
    !first.starts_with("==")
        && retired_whole_item_close_rewrite(first).is_some()
}

/// Byte length of a plain `=/` split `#name` part: ends at whitespace,
/// `#`, `~`, or `@`, so adjacent `#` selectors never swallow into one
/// name. Empty when the selector has no text (a trailing or doubled `#`).
fn split_selector_extent(after_hash: &str) -> usize {
    after_hash
        .find(|character: char| {
            character.is_ascii_whitespace()
                || character == '#'
                || character == '~'
                || character == '@'
        })
        .unwrap_or(after_hash.len())
}

/// Lex the text after `=/` (the `/` already stripped) into an optional bare
/// base plus ordered `#name`/`#"Literal"` selectors. `sigil_len` is 1 for
/// the single `=` sigil. The returned length includes the sigil, the slash,
/// the base, and every `#` part (including a trailing incomplete `#` or
/// literal extent). Malformed or incomplete selectors are kept as empty or
/// partial entries so the whole-item parser can teach precisely instead of
/// falling through as a task.
fn split_base_len(after_slash: &str) -> Option<usize> {
    let end = after_slash
        .find(|character: char| {
            character.is_ascii_whitespace()
                || character == '#'
                || character == '~'
                || character == '@'
        })
        .unwrap_or(after_slash.len());
    if end == 0 {
        return None;
    }
    let candidate = &after_slash[..end];
    bare_start_name_len(candidate).filter(|len| *len == end)
}

fn lex_split_after_slash(
    after_slash: &str,
    sigil_len: usize,
) -> (Option<String>, Vec<PomodoroSplitSelector>, usize) {
    let mut token_len = sigil_len + 1;
    let mut base: Option<String> = None;
    let mut pos = 0;
    if let Some(name_len) = split_base_len(after_slash) {
        base = Some(after_slash[..name_len].to_string());
        pos += name_len;
        token_len += name_len;
    }
    let mut selectors = Vec::new();
    let bytes_len = after_slash.len();
    while pos < bytes_len && after_slash.as_bytes()[pos] == b'#' {
        let after_hash = &after_slash[pos + 1..];
        if after_hash.is_empty() {
            selectors.push(PomodoroSplitSelector {
                text: String::new(),
                literal: false,
            });
            token_len += 1;
            break;
        }
        if opens_exact_name(after_hash)
            && let Some(lex) =
                lex_exact_name_after_hash(after_hash, after_hash.len())
        {
            if lex.complete {
                selectors.push(PomodoroSplitSelector {
                    text: canonicalize_exact_name(&lex.decoded)
                        .unwrap_or(lex.decoded),
                    literal: true,
                });
                token_len += 1 + lex.literal_len;
                pos += 1 + lex.literal_len;
            } else {
                selectors.push(PomodoroSplitSelector {
                    text: String::new(),
                    literal: true,
                });
                token_len += 1 + lex.literal_len;
                pos += 1 + lex.literal_len;
            }
            continue;
        }
        let name_len = split_selector_extent(after_hash);
        if name_len == 0 {
            selectors.push(PomodoroSplitSelector {
                text: String::new(),
                literal: false,
            });
            token_len += 1;
            pos += 1;
            continue;
        }
        selectors.push(PomodoroSplitSelector {
            text: after_hash[..name_len].to_string(),
            literal: false,
        });
        token_len += 1 + name_len;
        pos += 1 + name_len;
    }
    (base, selectors, token_len)
}

/// Whether the text after `=*` is a merge (letter, `#`, quote, or empty)
/// rather than a retired close (`=*1`, `=*!2`, `=**`, `=*@`, `=*~`, `=*,`).
fn merge_star_rest_is_merge(after_star: &str) -> bool {
    after_star.is_empty()
        || after_star.starts_with('“')
        || matches!(
            after_star.as_bytes().first(),
            Some(byte) if byte.is_ascii_alphabetic() || *byte == b'#' || *byte == b'"'
        )
}

/// Lex the text after `=*` (the `*` already stripped) into an optional
/// source plus ordered `#dest` selectors. Quoted sources (`=*"NAME"#fix`)
/// run through the exact-name lexer. `sigil_len` is 1 for the single `=`
/// sigil. The returned length includes `=*`, the source, and every `#`
/// part.
fn lex_merge_after_star(
    after_star: &str,
    sigil_len: usize,
) -> (Option<String>, Vec<PomodoroSplitSelector>, usize) {
    let mut token_len = sigil_len + 1;
    let mut src: Option<String> = None;
    let mut pos = 0;
    if opens_exact_name(after_star)
        && let Some(lex) =
            lex_exact_name_after_hash(after_star, after_star.len())
    {
        if lex.complete {
            src = Some(
                canonicalize_exact_name(&lex.decoded).unwrap_or(lex.decoded),
            );
        } else {
            src = Some(String::new());
        }
        pos += lex.literal_len;
        token_len += lex.literal_len;
    } else if let Some(name_len) = split_base_len(after_star) {
        src = Some(after_star[..name_len].to_string());
        pos += name_len;
        token_len += name_len;
    }
    let mut dests = Vec::new();
    let bytes_len = after_star.len();
    while pos < bytes_len && after_star.as_bytes()[pos] == b'#' {
        let after_hash = &after_star[pos + 1..];
        if after_hash.is_empty() {
            dests.push(PomodoroSplitSelector {
                text: String::new(),
                literal: false,
            });
            token_len += 1;
            break;
        }
        if opens_exact_name(after_hash)
            && let Some(lex) =
                lex_exact_name_after_hash(after_hash, after_hash.len())
        {
            if lex.complete {
                dests.push(PomodoroSplitSelector {
                    text: canonicalize_exact_name(&lex.decoded)
                        .unwrap_or(lex.decoded),
                    literal: true,
                });
                token_len += 1 + lex.literal_len;
                pos += 1 + lex.literal_len;
            } else {
                dests.push(PomodoroSplitSelector {
                    text: String::new(),
                    literal: true,
                });
                token_len += 1 + lex.literal_len;
                pos += 1 + lex.literal_len;
            }
            continue;
        }
        let name_len = split_selector_extent(after_hash);
        if name_len == 0 {
            dests.push(PomodoroSplitSelector {
                text: String::new(),
                literal: false,
            });
            token_len += 1;
            pos += 1;
            continue;
        }
        dests.push(PomodoroSplitSelector {
            text: after_hash[..name_len].to_string(),
            literal: false,
        });
        token_len += 1 + name_len;
        pos += 1 + name_len;
    }
    (src, dests, token_len)
}

/// Claim a bare lowercase close at the first whitespace-delimited token, or
/// lex the `=`-family token at the start of trimmed item text, mirroring
/// [`session_operator_token`]. Returns `None` for ordinary prose. A retired
/// `=x`/`=*`/`=!` close lexes as `Close` too, so it reports its teaching
/// error. A bare start token (`=`, `=-`) has
/// an empty or digit-free suffix; a counted token (`=3`, `=-2`, `=3-`,
/// `=2-1`, `=0`) carries at least one digit. An optional `#name` part
/// immediately after the suffix, or a bare `name` after an empty suffix,
/// ends at the first ASCII whitespace, `~`, or `@`; the `#` must come
/// immediately after the suffix. After the name, two optional parts may
/// follow, each at most once and in either order: an `@<X>` timing part
/// and a `~<K>` drop part. An unnamed token whose `~<K>` part is present
/// may also take an `@<X>` part after it. A token with no name and no
/// `~<K>` never reads an `@`, so `=@3` stays prose and `=3@2` keeps its
/// shape error. Whichever optional part comes first ends at whitespace or
/// at the other part's sigil; the second runs to whitespace. `len` covers
/// every part.
pub(crate) fn session_equals_token(text: &str) -> Option<EqualsToken> {
    // Bare `x` closes only at the start of a whole-item token. Keep this
    // strict classifier ahead of the `=`-family lexer so starts and all
    // existing `=` spellings retain their current tokenization.
    let first = text.split_whitespace().next().unwrap_or(text);
    if classify_whole_item_close(first).is_some() || first.starts_with("x#") {
        return Some(EqualsToken::Close);
    }
    // Past `x=` claims every lowercase prefix at an item start or chain
    // token boundary, including malformed tokens, before merge and retired
    // handling. Never reparse `E` by prepending `=` and invoking this
    // function: that would allow start, merge, split, or retired-close
    // semantics. `X=3` and mid-body `Plan x=3` never reach here as `x=`.
    if let Some(len) = past_token_len(text) {
        return Some(EqualsToken::Past { len });
    }
    // Merge-shaped `=*` (letter, `#`, quote, or bare) claims the token
    // before retired close, so `=*build#fix` merges and `=*1` still
    // teaches `x*1`. `retired_whole_item_close_rewrite("=*")` stays Some
    // so `==*` remains the doubled-close teaching error.
    // Unlike other selectors, a quoted merge source can contain spaces
    // (`=*"DEEP WORK"`). Lex the full parent text so the exact-name lexer
    // can consume through its closing quote before the whole-item shape
    // check decides whether trailing text remains.
    let merge_text = text
        .strip_prefix("=*")
        .filter(|after_star| merge_star_rest_is_merge(after_star))
        .map(|_| text)
        .unwrap_or(first);
    if let Some(after_star) = merge_text.strip_prefix("=*")
        && merge_star_rest_is_merge(after_star)
    {
        let (src, dests, merge_len) = lex_merge_after_star(after_star, 1);
        return Some(EqualsToken::Merge {
            src,
            dests,
            len: merge_len,
        });
    }
    // Retired `=x`/`=X`/`=*`/`=!`/`=x#…` closes claim the token; other
    // `=x…` shapes (`=xx`, `=xmas`) fall through to start lexing.
    if retired_whole_item_close_rewrite(first).is_some() {
        return Some(EqualsToken::Close);
    }
    let rest = text.strip_prefix('=')?;
    // Keep lexing the retired doubled sigil so it remains claimed with the
    // same spans. Claimed starts fail below with a teaching error; close
    // near misses (`==x`, `==X1`, `==*`, `==!2`) keep their own message.
    let (rest, sigil_len, doubled) = match rest.strip_prefix('=') {
        Some(after_second) => {
            if double_close_tail_is_claimed(&text[1..]) {
                return Some(EqualsToken::Close);
            }
            (after_second, 2, true)
        }
        None => (rest, 1, false),
    };
    // A leading `=/` claims the whole-item split before generic
    // start/prose handling. A mid-body `=/` never reaches here because the
    // text does not start with `=`.
    if !doubled && let Some(after_slash) = rest.strip_prefix('/') {
        let (base, selectors, split_len) =
            lex_split_after_slash(after_slash, sigil_len);
        return Some(EqualsToken::Split {
            base,
            selectors,
            len: split_len,
        });
    }
    let bytes = rest.as_bytes();
    let mut len = 0;
    while len < bytes.len() && bytes[len].is_ascii_digit() {
        len += 1;
    }
    if bytes.get(len) == Some(&b'-') {
        len += 1;
        while len < bytes.len() && bytes[len].is_ascii_digit() {
            len += 1;
        }
    }
    let suffix = rest[..len].to_string();
    let mut token_len = len + sigil_len;
    let mut name: Option<String> = None;
    let mut name_spelling: Option<NameSpelling> = None;
    let mut name_is_literal = false;
    if rest.as_bytes().get(len) == Some(&b'#') {
        let after_hash = &rest[len + 1..];
        if opens_exact_name(after_hash)
            && let Some(lex) =
                lex_exact_name_after_hash(after_hash, after_hash.len())
        {
            // An exact-name literal: the token extent runs through the
            // closing quote. Unterminated is an incomplete editing state:
            // claim an empty name so the incomplete diagnostic wins, never
            // a shape error.
            name_is_literal = true;
            name_spelling = Some(NameSpelling::Hash);
            if lex.complete {
                name = Some(
                    canonicalize_exact_name(&lex.decoded)
                        .unwrap_or(lex.decoded),
                );
                token_len += 1 + lex.literal_len;
            } else {
                name = Some(String::new());
                token_len += 1 + lex.literal_len;
            }
        } else {
            let name_len = start_name_extent(after_hash);
            name = Some(after_hash[..name_len].to_string());
            name_spelling = Some(NameSpelling::Hash);
            token_len += 1 + name_len;
        }
    } else if !doubled
        && suffix.is_empty()
        && let Some(name_len) = bare_start_name_len(rest)
    {
        name = Some(rest[..name_len].to_string());
        name_spelling = Some(NameSpelling::Bare);
        token_len += name_len;
    }
    let mut drop: Option<(usize, String)> = None;
    let mut timing: Option<(usize, String)> = None;
    // After the suffix/name, an optional `@<X>` timing part and an optional
    // `~<K>` drop part may follow in either order (named), or a `~<K>` part
    // followed by an `@<X>` part (unnamed with a drop). A token with no
    // `#name` and no `~<K>` never reads an `@`.
    let mut rest_pos = token_len - sigil_len;
    let first = rest.as_bytes().get(rest_pos).copied();
    match (name.is_some(), first) {
        (true, Some(b'~')) | (true, Some(b'@')) | (false, Some(b'~')) => {
            let first_sigil = first.expect("sigil") as char;
            let after_first = &rest[rest_pos + 1..];
            let other = if first_sigil == '~' { '@' } else { '~' };
            // Named: first part ends at whitespace or the other sigil.
            // Unnamed (no name): only `~` can come first, ending at `@`.
            let first_len = if name.is_some() {
                after_first
                    .find(|c: char| c.is_ascii_whitespace() || c == other)
                    .unwrap_or(after_first.len())
            } else {
                // Unnamed: `~` part ends at whitespace or `@`.
                after_first
                    .find(|c: char| c.is_ascii_whitespace() || c == '@')
                    .unwrap_or(after_first.len())
            };
            let first_text = after_first[..first_len].to_string();
            let first_offset = token_len;
            token_len += 1 + first_len;
            rest_pos += 1 + first_len;
            if first_sigil == '~' {
                drop = Some((first_offset, first_text));
            } else {
                timing = Some((first_offset, first_text));
            }
            // Second part, if present, runs to whitespace.
            let second = rest.as_bytes().get(rest_pos).copied();
            let want_second = match (name.is_some(), first_sigil, second) {
                (true, '~', Some(b'@')) | (true, '@', Some(b'~')) => true,
                (false, '~', Some(b'@')) => true,
                _ => false,
            };
            if want_second {
                let after_second = &rest[rest_pos + 1..];
                let second_len = after_second
                    .find(|c: char| c.is_ascii_whitespace())
                    .unwrap_or(after_second.len());
                let second_text = after_second[..second_len].to_string();
                let second_offset = token_len;
                token_len += 1 + second_len;
                if first_sigil == '~' {
                    timing = Some((second_offset, second_text));
                } else {
                    drop = Some((second_offset, second_text));
                }
            }
        }
        _ => {}
    }
    let effective = if let Some((_, text)) = timing.as_ref() {
        text.clone()
    } else {
        suffix.clone()
    };
    // `counted` reflects the effective `<X>`.
    let counted = effective.bytes().any(|byte| byte.is_ascii_digit());
    Some(EqualsToken::Start {
        suffix,
        counted,
        len: token_len,
        name,
        name_spelling,
        name_is_literal,
        drop,
        timing,
        doubled,
    })
}

/// Absolute byte offset of a named start's name inside the token: after
/// `=<X>#` for Hash, after `=<X>` for Bare.
pub(crate) fn named_start_name_offset(
    suffix_len: usize,
    spelling: NameSpelling,
) -> usize {
    let prefix = 1 + suffix_len;
    match spelling {
        NameSpelling::Hash => prefix + 1,
        NameSpelling::Bare => prefix,
    }
}

/// The effective `<X>` timing text: the `@` text when a timing part was
/// typed, otherwise the leading suffix.
pub(crate) fn effective_start_timing(
    suffix: &str,
    timing: &Option<(usize, String)>,
) -> String {
    timing
        .as_ref()
        .map(|(_, text)| text.clone())
        .unwrap_or_else(|| suffix.to_string())
}

/// Whether a whitespace-separated token can take part in a same-line
/// session-operator chain. A token qualifies exactly when the whole-item
/// session parsers would claim it as a standalone one-line item: either
/// [`parse_pomodoro_equals_item`] or [`parse_pomodoro_adjust_item`] returns
/// something other than `Ok(None)` for it. "Claimed" therefore includes
/// both valid tokens and near misses (zero magnitudes, overflows, counted
/// tokens with extra text, selection-shaped close fragments), so a broken
/// token inside a chain gets its own precise family diagnostic on its own
/// range instead of a vague shape error. A same-line chain has already been
/// split into single-token items upstream in `draft.rs`, so these parsers
/// only ever see exact single-token items.
/// [`parse_pomodoro_equals_item`]: parse_pomodoro_equals_item
/// [`parse_pomodoro_adjust_item`]: parse_pomodoro_adjust_item
pub(super) fn is_session_chain_token(token: &str) -> bool {
    if let Some((_, digits, len)) = session_operator_token(token) {
        return len == token.len() || !digits.is_empty();
    }
    match session_equals_token(token) {
        Some(EqualsToken::Start {
            counted,
            len,
            name,
            drop,
            timing,
            ..
        }) => {
            if name.is_some() || drop.is_some() || timing.is_some() {
                return true;
            }
            len == token.len() || counted
        }
        Some(EqualsToken::Split { len, .. }) => len == token.len(),
        Some(EqualsToken::Merge { len, .. }) => len == token.len(),
        Some(EqualsToken::Past { len }) => len == token.len(),
        Some(EqualsToken::Close) => {
            // A `==`-prefixed close tail (`==x`, `==X1`, `==*`, `==!2`) is
            // a claimed teaching near miss, so it chains like any other
            // claimed token and reports its own diagnostic per item.
            token.starts_with("==")
                || is_plain_whole_item_close(token)
                || whole_item_close_after_x(token).is_some()
                || is_close_hash_token(token)
                || retired_whole_item_close_rewrite(token).is_some()
        }
        None => false,
    }
}

/// Whether a whitespace-free token is a close-hash near miss: bare lowercase
/// `x` immediately followed by `#`. The retired `=x#…` spelling is a
/// retired-form teaching error instead.
pub(super) fn is_close_hash_token(token: &str) -> bool {
    token.starts_with("x#")
}

/// Whether a close token is the plain whole-item `x`, with no selection
/// body.
pub(super) fn is_plain_whole_item_close(token: &str) -> bool {
    classify_whole_item_close(token).is_some_and(str::is_empty)
}

/// Whole-item session-operator grammar: one sign resizes, two signs shift.
///
/// A same-line chain has already been split into single-token items
/// upstream in `draft.rs`, so this parser only ever sees an exact
/// single-token item here.
///
/// Returns `Ok(None)` when the item does not start with an operator token
/// and ordinary parsing should continue. Returns `Ok(Some(outcome))` for
/// an exact single-token operator (an omitted count means 1). Returns
/// `Err` for every operator-shaped near miss: a zero magnitude, an
/// overflow, a counted token with extra text, or a bare-or-counted token
/// with child lines. Near misses are never allowed to fall through as
/// ordinary tasks. A bare sign run followed by more text on the same line
/// (`- foo`, `++ plan`), a longer or mixed run (`+++`, `+-`), and any
/// mid-body token (`Plan ++3`, `C++`) stay ordinary prose.
pub(super) fn parse_pomodoro_adjust_item<'a>(
    item: &CaptureItem<'a>,
    parent_line: &ItemLine<'a>,
    forced_route: Option<&str>,
    forced_section: Option<&str>,
) -> Result<Option<ParsedCaptureItemOutcome<'a>>, String> {
    let parent_trimmed = parent_line.raw.text.trim();
    let Some((operator, digits, prefix_len)) =
        session_operator_token(parent_trimmed)
    else {
        return Ok(None);
    };
    let exact = parent_trimmed.len() == prefix_len && item.lines.len() == 1;
    if !exact {
        // A counted token claims the item even with extra text: `+5 more`,
        // `++3@work`, `--2 s:1`, `++3++`. A bare token claims only a child
        // line; bare text on the same line (`- foo`, `-- aside`) stays
        // prose.
        if !digits.is_empty() {
            return Err(match operator {
                SessionOperator::Resize { .. } => {
                    POMODORO_ADJUST_SHAPE_ERROR.to_string()
                }
                SessionOperator::Shift { .. } => {
                    POMODORO_SHIFT_SHAPE_ERROR.to_string()
                }
            });
        }
        if parent_trimmed.len() == prefix_len && item.lines.len() > 1 {
            return Err(match operator {
                SessionOperator::Resize { .. } => {
                    POMODORO_ADJUST_SHAPE_ERROR.to_string()
                }
                SessionOperator::Shift { .. } => {
                    POMODORO_SHIFT_SHAPE_ERROR.to_string()
                }
            });
        }
        return Ok(None);
    }
    let units = if digits.is_empty() {
        1
    } else {
        digits.parse::<u64>().map_err(|_| match operator {
            SessionOperator::Resize { .. } => {
                POMODORO_ADJUST_OVERFLOW_ERROR.to_string()
            }
            SessionOperator::Shift { .. } => {
                POMODORO_SHIFT_OVERFLOW_ERROR.to_string()
            }
        })?
    };
    if units == 0 {
        return Err(match operator {
            SessionOperator::Resize { .. } => {
                POMODORO_ADJUST_ZERO_ERROR.to_string()
            }
            SessionOperator::Shift { .. } => {
                POMODORO_SHIFT_ZERO_ERROR.to_string()
            }
        });
    }
    if forced_route.is_some() || forced_section.is_some() {
        return Err(match operator {
            SessionOperator::Resize { .. } => {
                POMODORO_ADJUST_FORCED_ERROR.to_string()
            }
            SessionOperator::Shift { .. } => {
                POMODORO_SHIFT_FORCED_ERROR.to_string()
            }
        });
    }
    let raw = parent_trimmed.to_string();
    let kind = match operator {
        SessionOperator::Resize { plus } => CaptureKind::PomodoroAdjust {
            spec: PomodoroAdjustSpec {
                raw: raw.clone(),
                plus,
                units,
            },
        },
        SessionOperator::Shift { later } => CaptureKind::PomodoroShift {
            spec: PomodoroShiftSpec {
                raw: raw.clone(),
                later,
                units,
            },
        },
    };
    Ok(Some(parsed_capture_item_outcome(
        item,
        ParsedCaptureText {
            body: raw,
            clip: None,
            route: None,
            kind,
            scheduled_offset: None,
            priority_level: None,
            sub_bullets: Vec::new(),
            dependencies: Vec::new(),
            blocked_tasks: Vec::new(),
            relations: Vec::new(),
            dependency_target: None,
            type_marker: None,
        },
        Vec::new(),
        None,
    )))
}

/// A named start token's own error, in E1/E3/E2/overflow order. `None`
/// when the token itself is well-formed (callers then report E4 for extra
/// text or accept the exact token). `token` is the token without its
/// `~<K>` drop and `@<X>` timing parts; `drop_text`/`timing_text` keep the
/// typed parts in the order-error suggestion. Suggestions that rewrite the
/// typed token keep `spelling`.
fn named_token_error(
    token: &str,
    suffix: &str,
    name: &str,
    drop_text: Option<&str>,
    timing_text: Option<&str>,
    spelling: NameSpelling,
    is_literal: bool,
) -> Option<String> {
    if name.is_empty() {
        return Some(pomodoro_named_start_incomplete_error(token));
    }
    if is_literal {
        // A literal's spaces are the point: validate the canonical name,
        // never the slug charset.
        if canonicalize_exact_name(name).is_none() {
            return Some(pomodoro_named_start_name_error(name, token));
        }
    } else if let Some((before, after)) = name.split_once('=')
        && suffix.is_empty()
        && timing_text.is_none()
        && parse_pomodoro_start_suffix(after).is_ok()
    {
        let drop_suffix =
            drop_text.map(|text| format!("~{text}")).unwrap_or_default();
        let at_form = format!(
            "{}@{after}{drop_suffix}",
            echo_named_start(spelling, suffix, before)
        );
        let hash_form = format!("={after}#{before}{drop_suffix}");
        return Some(format!(
            "write the duration after `@` or before the name: `{at_form}` or `{hash_form}` instead of `{token}`"
        ));
    } else if !is_pomodoro_selector_component(name) {
        return Some(pomodoro_named_start_name_error(name, token));
    }
    let effective = timing_text.unwrap_or(suffix);
    if let Err(message) = parse_pomodoro_start_suffix(effective) {
        return Some(message);
    }
    None
}

/// Both-timings error for a named token with a leading `<X>` and an `@<X>`
/// part. `None` when not both. Wins over the dangling-`@` incomplete state.
fn named_timing_twice_error(
    token: &str,
    suffix: &str,
    name: &str,
    timing_text: Option<&str>,
) -> Option<String> {
    let timing = timing_text?;
    if suffix.is_empty() {
        return None;
    }
    let sigil = "=";
    let effective = if !timing.is_empty() { timing } else { suffix };
    Some(start_timing_twice_error(token, sigil, effective, name))
}

/// E4 for a well-formed named token with extra text or child lines.
/// `token` is the token without its drop part; a multi-word hint keeps the
/// typed `~<K>` and the typed `#` or bare spelling.
pub(super) fn named_shape_error_with_drop(
    token: &str,
    suffix: &str,
    name: &str,
    parent_trimmed: &str,
    len: usize,
    drop_text: Option<&str>,
    spelling: NameSpelling,
    is_literal: bool,
) -> String {
    if name.is_empty()
        && let Some(nospace) = named_nospace_error(suffix, parent_trimmed, len)
    {
        return nospace;
    }
    let mut message = pomodoro_named_start_shape_error(token, suffix, name);
    // A literal's spaces are inside the token: trailing words are extra
    // text, never a join-with-`-` hint.
    if !is_literal && !name.is_empty() && parent_trimmed.len() > len {
        let extra = parent_trimmed[len..].trim_start();
        let words: Vec<&str> = extra.split_whitespace().collect();
        if !words.is_empty()
            && words
                .iter()
                .all(|word| is_pomodoro_selector_component(word))
        {
            let joined = words.join("-");
            let drop_suffix =
                drop_text.map(|text| format!("~{text}")).unwrap_or_default();
            let suggestion = format!(
                "{}{drop_suffix}",
                echo_named_start(spelling, suffix, &format!("{name}-{joined}"))
            );
            message.push_str(&format!(
                "; to name a multi-word Pomodoro, join the words with `-`: `{suggestion}`"
            ));
        }
    }
    message
}

/// Teaching error when a counted unnamed start's `<X>` runs into a following
/// letter (`=3bugs`, `=2fa`). `None` when the byte after the suffix is not
/// an ASCII letter. The range is token-relative over the glued name.
pub(super) fn glued_timing_diagnostic(
    parent_trimmed: &str,
    len: usize,
    suffix: &str,
) -> Option<(String, (usize, usize))> {
    if suffix.is_empty() || !suffix.bytes().any(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let rest = parent_trimmed.get(len..)?;
    if !rest.as_bytes().first().is_some_and(u8::is_ascii_alphabetic) {
        return None;
    }
    let name_len = start_name_extent(rest);
    let name = rest.get(..name_len)?;
    if name.is_empty() || !is_pomodoro_selector_component(name) {
        return None;
    }
    let token = parent_trimmed
        .get(..len + name_len)
        .unwrap_or(parent_trimmed);
    let at_form = format!("{}@{suffix}", named_start_token(name));
    let hash_form = format!("={suffix}#{name}");
    let combined = format!("{suffix}{name}");
    let digit_name = if suffix.bytes().all(|byte| byte.is_ascii_digit())
        && is_pomodoro_selector_component(&combined)
    {
        Some(named_start_token(&combined))
    } else {
        None
    };
    Some((
        pomodoro_glued_timing_error(
            token,
            &at_form,
            &hash_form,
            digit_name.as_deref(),
        ),
        (len, len + name_len),
    ))
}

/// Absolute byte offset of a whole-item start token's `after_tilde` text
/// (the drop list after the first `~`), so drop diagnostics point at the
/// original input. `drop_offset` is the token-relative offset of the `~`.
fn start_drop_offset(parent_line: &ItemLine<'_>, drop_offset: usize) -> usize {
    let leading =
        parent_line.raw.text.len() - parent_line.raw.text.trim_start().len();
    parent_line.raw.start + leading + drop_offset + 1
}

/// Lex a start drop part for execution: a valid list, an incomplete
/// separator error, or the list's own diagnostic. `token_text` is the
/// display token interpolated into diagnostics.
fn lex_start_drop_for_execution(
    parent_line: &ItemLine<'_>,
    token_text: &str,
    drop_offset: usize,
    after_tilde: &str,
) -> Result<Vec<u32>, String> {
    let offset = start_drop_offset(parent_line, drop_offset);
    match lex_start_drop(after_tilde, offset, token_text) {
        Ok(StartDropOutcome::Valid(lex)) => Ok(lex.drop),
        Ok(StartDropOutcome::Incomplete(incomplete)) => Err(
            close_selection_incomplete_error(token_text, incomplete.separator),
        ),
        Err(error) => Err(error.message),
    }
}

/// Whether the spaceless join of a start line lexes as a valid whole-item
/// start with a drop or timing list, in which case the no-spaces hint wins
/// over the shape error.
pub(super) fn spaceless_start_drop_is_valid(nospace: &str) -> bool {
    let Some(EqualsToken::Start {
        suffix,
        name,
        drop,
        timing,
        len,
        ..
    }) = session_equals_token(nospace)
    else {
        return false;
    };
    if len != nospace.len() {
        return false;
    }
    if drop.is_none() && timing.is_none() {
        return false;
    }
    let effective = effective_start_timing(&suffix, &timing);
    if parse_pomodoro_start_suffix(&effective).is_err() {
        return false;
    }
    if let Some(selector) = name
        && (selector.is_empty() || !is_pomodoro_selector_component(&selector))
    {
        return false;
    }
    if let Some((_, after_tilde)) = drop
        && !lex_start_drop(&after_tilde, 0, nospace)
            .is_ok_and(|outcome| matches!(outcome, StartDropOutcome::Valid(_)))
    {
        return false;
    }
    // A timing part, when present, must itself be a valid `<X>`.
    if let Some((_, after_at)) = timing
        && parse_pomodoro_start_suffix(&after_at).is_err()
    {
        return false;
    }
    true
}

/// Whether the spaceless join's first extra word starts with `@`, in which
/// case the timing no-space hint wins over the drop hint.
pub(super) fn spaceless_extra_is_timing(
    parent_trimmed: &str,
    len: usize,
) -> bool {
    let extra = parent_trimmed.get(len..).unwrap_or("").trim_start();
    extra
        .split_whitespace()
        .next()
        .is_some_and(|word| word.starts_with('@'))
}

/// E4 no-space variant for `=# <word>`: the name must follow `#` directly.
pub(super) fn named_nospace_error(
    suffix: &str,
    parent_trimmed: &str,
    len: usize,
) -> Option<String> {
    let extra = parent_trimmed.get(len..)?.trim_start();
    let word = extra.split_whitespace().next()?;
    if word.is_empty() {
        return None;
    }
    let sigil = "=";
    Some(pomodoro_named_start_nospace_error(sigil, suffix, word))
}

/// Absolute byte offset of a whole-item close token's `after_x` text (the
/// lists after `x`), so selection diagnostics point at the original input.
/// `first` is the parent line's first whitespace-delimited token.
fn close_after_x_offset(
    parent_line: &ItemLine<'_>,
    first: &str,
    after_x: &str,
) -> usize {
    let leading =
        parent_line.raw.text.len() - parent_line.raw.text.trim_start().len();
    parent_line.raw.start + leading + (first.len() - after_x.len())
}

/// Whole-item session grammar: `x[<N>][!<M>]` closes, `=`/`=<X>` starts.
///
/// A same-line chain has already been split upstream in `draft.rs`: chain
/// siblings are exact single-token items, while an inline-close owner
/// carries exactly `x… <entry>` on its parent line. Starts still only ever
/// see exact single-token items here.
///
/// Runs after the `:` task-link query check (before session operators,
/// caret links, and ordinary parsing). Returns `Ok(None)` when the item is
/// not a session token and ordinary parsing should continue. Returns
/// `Ok(Some(outcome))` for an exact single-token close or start. Returns
/// `Err` for every near miss: a leading `x` token with markers or child
/// lines; a retired `=x`/`=X`/`=*`/`=!` close (it names the bare rewrite);
/// a counted start token (`=3`, `=-2`, `=3-`, `=2-1`, `=0`) with anything
/// else; or an exact start token with child lines. Near misses never fall
/// through as ordinary tasks. A bare token followed by more text on the same
/// line (`= foo`, `=- foo`, `== foo`, `=-)`), every other `x` shape (`xx`,
/// `xa`, `x.`, `x-ray`) and capitalized `X…`, doubled lookalikes that are no
/// claimed start or close tail (`==xyz`, `==foo`, `===`), and mid-body tokens (`Plan =3`,
/// `Plan ==3`, `a=3`, `Plan x1`) stay ordinary prose: a bare sign run
/// followed by prose stays prose while a counted token claims its item.
///
/// A selection-shaped first token (`x` followed by a digit, `,`, `*`, `!`,
/// `~`, or `@`) claims the item the same way: an exact single-token item
/// lexes its lists (a dangling separator is an incomplete error here),
/// while trailing text on the parent line lexes as the inline Work Log
/// entry through the shared inline lexer.
pub(super) fn parse_pomodoro_equals_item<'a>(
    item: &CaptureItem<'a>,
    parent_line: &ItemLine<'a>,
    forced_route: Option<&str>,
    forced_section: Option<&str>,
) -> Result<Option<ParsedCaptureItemOutcome<'a>>, String> {
    let parent_trimmed = parent_line.raw.text.trim();
    let Some(token) = session_equals_token(parent_trimmed) else {
        return Ok(None);
    };
    match token {
        EqualsToken::Split {
            base,
            selectors,
            len,
        } => {
            let Some(first) = parent_trimmed.split_whitespace().next() else {
                return Ok(None);
            };
            // A mid-body `=/` never reaches here, but guard anyway: the
            // token must start the trimmed line.
            if !parent_trimmed.starts_with("=/") {
                return Ok(None);
            }
            // Validate selectors before shape: empty text is an incomplete
            // or malformed selector, never a task.
            for selector in &selectors {
                if selector.text.is_empty() {
                    return Err(pomodoro_split_selector_error(first, ""));
                }
                if !selector.literal
                    && !is_pomodoro_selector_component(&selector.text)
                {
                    return Err(pomodoro_split_selector_error(
                        first,
                        &selector.text,
                    ));
                }
            }
            if let Some(base_name) = base.as_deref()
                && !is_pomodoro_selector_component(base_name)
            {
                return Err(pomodoro_split_selector_error(first, base_name));
            }
            let single_token_parent =
                parent_trimmed == first && first.len() == len;
            if !single_token_parent || item.lines.len() != 1 {
                return Err(pomodoro_split_shape_error(first));
            }
            if forced_route.is_some() || forced_section.is_some() {
                return Err(POMODORO_SPLIT_FORCED_ERROR.to_string());
            }
            let spec = PomodoroSplitSpec {
                raw: first.to_string(),
                base,
                selectors,
            };
            Ok(Some(parsed_capture_item_outcome(
                item,
                ParsedCaptureText {
                    body: first.to_string(),
                    clip: None,
                    route: None,
                    kind: CaptureKind::PomodoroSplit { spec },
                    scheduled_offset: None,
                    priority_level: None,
                    sub_bullets: Vec::new(),
                    dependencies: Vec::new(),
                    blocked_tasks: Vec::new(),
                    relations: Vec::new(),
                    dependency_target: None,
                    type_marker: None,
                },
                Vec::new(),
                None,
            )))
        }
        EqualsToken::Merge { src, dests, len } => {
            let Some(first_token) = parent_trimmed.split_whitespace().next()
            else {
                return Ok(None);
            };
            if !parent_trimmed.starts_with("=*") {
                return Ok(None);
            }
            let first = parent_trimmed.get(..len).unwrap_or(first_token);
            for dest in &dests {
                if dest.text.is_empty() {
                    return Err(pomodoro_merge_selector_error(first, ""));
                }
                if !dest.literal && !is_pomodoro_selector_component(&dest.text)
                {
                    return Err(pomodoro_merge_selector_error(
                        first, &dest.text,
                    ));
                }
            }
            if let Some(src_name) = src.as_deref()
                && (src_name.is_empty()
                    || (!src_name.contains(char::is_whitespace)
                        && !is_pomodoro_selector_component(src_name)))
            {
                return Err(pomodoro_merge_selector_error(first, src_name));
            }
            let single_token_parent =
                parent_trimmed == first && first.len() == len;
            if !single_token_parent || item.lines.len() != 1 {
                return Err(pomodoro_merge_shape_error(first));
            }
            if forced_route.is_some() || forced_section.is_some() {
                return Err(POMODORO_MERGE_FORCED_ERROR.to_string());
            }
            let spec = PomodoroMergeSpec {
                raw: first.to_string(),
                src,
                dests,
            };
            Ok(Some(parsed_capture_item_outcome(
                item,
                ParsedCaptureText {
                    body: first.to_string(),
                    clip: None,
                    route: None,
                    kind: CaptureKind::PomodoroMerge { spec },
                    scheduled_offset: None,
                    priority_level: None,
                    sub_bullets: Vec::new(),
                    dependencies: Vec::new(),
                    blocked_tasks: Vec::new(),
                    relations: Vec::new(),
                    dependency_target: None,
                    type_marker: None,
                },
                Vec::new(),
                None,
            )))
        }
        EqualsToken::Past { len } => {
            use crate::native::capture_language::past::{
                split_past_token, validate_past_e, PastEOutcome,
            };
            const PAST_FORCED_ERROR: &str = "Pomodoro past `x=` cannot be combined with --route, --section, --task, --task-ref, --drawer, or --clip; capture the past alone";
            let raw_token =
                parent_trimmed.get(..len).unwrap_or(parent_trimmed);
            let Some(split) = split_past_token(raw_token) else {
                return Ok(None);
            };
            let valid = match validate_past_e(raw_token, &split) {
                PastEOutcome::Invalid(message) => {
                    return Err(message);
                }
                PastEOutcome::Incomplete { message, .. } => {
                    return Err(message);
                }
                PastEOutcome::Valid(valid) => valid,
            };
            if forced_route.is_some() || forced_section.is_some() {
                return Err(PAST_FORCED_ERROR.to_string());
            }
            let leading = parent_line.raw.text.len()
                - parent_line.raw.text.trim_start().len();
            let token_abs_start = parent_line.raw.start + leading;
            let close_base = token_abs_start + split.close_offset;
            let lexed_selection = match lex_close_selection(
                &split.close_text,
                close_base,
                raw_token,
            ) {
                Ok(CloseSelectionOutcome::Valid(lex)) => lex,
                Ok(CloseSelectionOutcome::Incomplete(incomplete)) => {
                    return Err(close_selection_incomplete_error(
                        raw_token,
                        incomplete.separator,
                    ));
                }
                Err(error) => return Err(error.message),
            };
            let single_token_parent = parent_trimmed == raw_token;
            if single_token_parent && item.lines.len() == 1 {
                let mut close =
                    close_spec_from_lex(split.close_text.clone(), &lexed_selection);
                close.log = Vec::new();
                let spec = PomodoroPastSpec {
                    raw: raw_token.to_string(),
                    name: valid.name.clone(),
                    name_is_literal: valid.name_is_literal,
                    blocks: valid.blocks,
                    blocks_typed: valid.blocks_typed,
                    repair: split.repair,
                    close,
                    stack: PastStackPos {
                        index: 1,
                        size: 1,
                        offset_blocks: 0,
                    },
                };
                let body = parent_trimmed.to_string();
                return Ok(Some(parsed_capture_item_outcome(
                    item,
                    ParsedCaptureText {
                        body,
                        clip: None,
                        route: None,
                        kind: CaptureKind::PomodoroPast { spec },
                        scheduled_offset: None,
                        priority_level: None,
                        sub_bullets: Vec::new(),
                        dependencies: Vec::new(),
                        blocked_tasks: Vec::new(),
                        relations: Vec::new(),
                        dependency_target: None,
                        type_marker: None,
                    },
                    Vec::new(),
                    None,
                )));
            }
            if single_token_parent {
                let lexed = match lex_close_selection(
                    &split.close_text,
                    close_base,
                    raw_token,
                ) {
                    Ok(CloseSelectionOutcome::Valid(lex)) => lex,
                    Ok(CloseSelectionOutcome::Incomplete(incomplete)) => {
                        return Err(close_selection_incomplete_error(
                            raw_token,
                            incomplete.separator,
                        ));
                    }
                    Err(error) => return Err(error.message),
                };
                match lex_close_log_bullets_with_all(
                    &item.lines[1..],
                    raw_token,
                    lexed.in_progress.as_deref(),
                    &lexed.park,
                    &lexed.complete,
                    &lexed.drop,
                    lexed.park_all,
                    lexed.complete_all,
                ) {
                    Err(error) => return Err(error.message),
                    Ok(lexed_bullets) => {
                        if let Some(dangling) = lexed_bullets.dangling.first() {
                            return Err(match dangling.index {
                                Some(index) => close_log_dangling_error(
                                    &format!("- {index}"),
                                    index,
                                ),
                                None => close_log_star_dangling_error(),
                            });
                        }
                        let mut close = close_spec_from_lex(
                            split.close_text.clone(),
                            &lexed,
                        );
                        close.log =
                            log_entries_from_lex(&lexed_bullets.entries);
                        let spec = PomodoroPastSpec {
                            raw: raw_token.to_string(),
                            name: valid.name.clone(),
                            name_is_literal: valid.name_is_literal,
                            blocks: valid.blocks,
                            blocks_typed: valid.blocks_typed,
                            repair: split.repair,
                            close,
                            stack: PastStackPos {
                                index: 1,
                                size: 1,
                                offset_blocks: 0,
                            },
                        };
                        let body = parent_trimmed.to_string();
                        return Ok(Some(parsed_capture_item_outcome(
                            item,
                            ParsedCaptureText {
                                body,
                                clip: None,
                                route: None,
                                kind: CaptureKind::PomodoroPast { spec },
                                scheduled_offset: None,
                                priority_level: None,
                                sub_bullets: Vec::new(),
                                dependencies: Vec::new(),
                                blocked_tasks: Vec::new(),
                                relations: Vec::new(),
                                dependency_target: None,
                                type_marker: None,
                            },
                            Vec::new(),
                            None,
                        )));
                    }
                }
            }
            let default_index = default_log_index_with_all(
                lexed_selection.in_progress.as_deref(),
                &lexed_selection.park,
                &lexed_selection.complete,
                &lexed_selection.drop,
                lexed_selection.park_all,
                lexed_selection.complete_all,
            );
            let has_bullet_child = item.lines[1..].iter().any(|line| {
                !matches!(
                    classify_authored_line(line.raw),
                    AuthoredLineClass::EmptyOrPlaceholder
                )
            });
            let tail_text = parent_trimmed
                .get(raw_token.len()..)
                .unwrap_or("");
            let tail_start = token_abs_start + raw_token.len();
            let tail_raw = RawLine {
                text: tail_text,
                start: tail_start,
                end: tail_start + tail_text.len(),
            };
            let tail_tokens = tokenize_line_with_spans(&tail_raw);
            match lex_close_inline_entry_with_all(
                &tail_tokens,
                raw_token,
                lexed_selection.in_progress.as_deref(),
                &lexed_selection.park,
                &lexed_selection.complete,
                &lexed_selection.drop,
                lexed_selection.park_all,
                lexed_selection.complete_all,
            ) {
                Err(error) => Err(error.message),
                Ok(CloseInlineLex::Dangling { index, .. }) => {
                    if has_bullet_child {
                        let bullet = format!("- {index}");
                        return Err(close_inline_mixing_error(&bullet));
                    }
                    let head = format!("{raw_token} {index}");
                    Err(close_inline_dangling_error(&head, index, false))
                }
                Ok(entry @ CloseInlineLex::Entry { .. }) => {
                    let (index, index_range, text) = match &entry {
                        CloseInlineLex::Entry {
                            index,
                            index_range,
                            text,
                            ..
                        } => (*index, *index_range, text.clone()),
                        CloseInlineLex::Dangling { .. } => unreachable!(),
                    };
                    if has_bullet_child {
                        let bullet = if index_range.is_none()
                            && close_first_bullet_is_unnumbered(
                                &item.lines[1..],
                            ) {
                            format!("- {text}")
                        } else {
                            format!("- {} {text}", index.unwrap_or(1))
                        };
                        return Err(close_inline_mixing_error(&bullet));
                    }
                    let mut close = close_spec_from_lex(
                        split.close_text.clone(),
                        &lexed_selection,
                    );
                    close.log = vec![log_entry_from_inline(
                        &entry,
                        raw_token,
                        default_index,
                    )];
                    let spec = PomodoroPastSpec {
                        raw: raw_token.to_string(),
                        name: valid.name.clone(),
                        name_is_literal: valid.name_is_literal,
                        blocks: valid.blocks,
                        blocks_typed: valid.blocks_typed,
                        repair: split.repair,
                        close,
                        stack: PastStackPos {
                            index: 1,
                            size: 1,
                            offset_blocks: 0,
                        },
                    };
                    let body = raw_token.to_string();
                    Ok(Some(parsed_capture_item_outcome(
                        item,
                        ParsedCaptureText {
                            body,
                            clip: None,
                            route: None,
                            kind: CaptureKind::PomodoroPast { spec },
                            scheduled_offset: None,
                            priority_level: None,
                            sub_bullets: Vec::new(),
                            dependencies: Vec::new(),
                            blocked_tasks: Vec::new(),
                            relations: Vec::new(),
                            dependency_target: None,
                            type_marker: None,
                        },
                        Vec::new(),
                        None,
                    )))
                }
            }
        }
        EqualsToken::Close => {
            let Some(first) = parent_trimmed.split_whitespace().next() else {
                return Ok(None);
            };
            // A `==x`-family close shape is retired. The lexer only yields
            // `Close` here for a claimed close tail, so it gets the close
            // migration message below.
            if first.starts_with("==") {
                let close_spelling =
                    retired_whole_item_close_rewrite(&first[1..])
                        .unwrap_or_default();
                return Err(pomodoro_double_close_error(
                    first,
                    &close_spelling,
                ));
            }
            // A retired `=x`/`=*`/`=!` close fails with the bare rewrite of
            // the whole line, so an inline entry survives the correction.
            if let Some(rewritten) = retired_whole_item_close_rewrite(first) {
                let rewrite =
                    format!("{rewritten}{}", &parent_trimmed[first.len()..]);
                return Err(retired_whole_item_close_error(first, &rewrite));
            }
            if is_close_hash_token(first) {
                let name = decode_close_hash_name(first, parent_trimmed);
                return Err(pomodoro_close_hash_error(&name));
            }
            let selection_after_x = whole_item_close_after_x(first);
            if selection_after_x.is_none() && !is_plain_whole_item_close(first)
            {
                return Ok(None);
            }
            let single_token_parent = parent_trimmed == first;
            if single_token_parent && item.lines.len() == 1 {
                if forced_route.is_some() || forced_section.is_some() {
                    return Err(POMODORO_CLOSE_FORCED_ERROR.to_string());
                }
                let raw = parent_trimmed.to_string();
                let spec = match selection_after_x {
                    None => PomodoroCloseSpec::plain(raw.clone()),
                    Some(after_x) => {
                        let offset =
                            close_after_x_offset(parent_line, first, after_x);
                        match lex_close_selection(after_x, offset, first) {
                            Ok(CloseSelectionOutcome::Valid(lex)) => {
                                close_spec_from_lex(raw.clone(), &lex)
                            }
                            Ok(CloseSelectionOutcome::Incomplete(
                                incomplete,
                            )) => {
                                return Err(close_selection_incomplete_error(
                                    first,
                                    incomplete.separator,
                                ));
                            }
                            Err(error) => return Err(error.message),
                        }
                    }
                };
                return Ok(Some(parsed_capture_item_outcome(
                    item,
                    ParsedCaptureText {
                        body: raw,
                        clip: None,
                        route: None,
                        kind: CaptureKind::PomodoroClose { spec },
                        scheduled_offset: None,
                        priority_level: None,
                        sub_bullets: Vec::new(),
                        dependencies: Vec::new(),
                        blocked_tasks: Vec::new(),
                        relations: Vec::new(),
                        dependency_target: None,
                        type_marker: None,
                    },
                    Vec::new(),
                    None,
                )));
            }
            if single_token_parent {
                // A single-token parent with Work Log bullet child lines:
                // lex the selection once (a broken list reports its own
                // diagnostic, a dangling separator stays incomplete), then
                // the shared bullet lexer. Bullet text never reaches
                // `resolve_line`, so markers stay literal.
                if forced_route.is_some() || forced_section.is_some() {
                    return Err(POMODORO_CLOSE_FORCED_ERROR.to_string());
                }
                let lexed_selection = match selection_after_x {
                    None => CloseSelectionLex {
                        in_progress: None,
                        park: Vec::new(),
                        complete: Vec::new(),
                        drop: Vec::new(),
                        park_all: false,
                        complete_all: false,
                        in_progress_range: None,
                        park_range: None,
                        complete_range: None,
                        drop_range: None,
                    },
                    Some(after_x) => {
                        let offset =
                            close_after_x_offset(parent_line, first, after_x);
                        match lex_close_selection(after_x, offset, first) {
                            Ok(CloseSelectionOutcome::Valid(lex)) => lex,
                            Ok(CloseSelectionOutcome::Incomplete(
                                incomplete,
                            )) => {
                                return Err(close_selection_incomplete_error(
                                    first,
                                    incomplete.separator,
                                ));
                            }
                            Err(error) => return Err(error.message),
                        }
                    }
                };
                match lex_close_log_bullets_with_all(
                    &item.lines[1..],
                    first,
                    lexed_selection.in_progress.as_deref(),
                    &lexed_selection.park,
                    &lexed_selection.complete,
                    &lexed_selection.drop,
                    lexed_selection.park_all,
                    lexed_selection.complete_all,
                ) {
                    Err(error) => return Err(error.message),
                    Ok(lexed) => {
                        if let Some(dangling) = lexed.dangling.first() {
                            return Err(match dangling.index {
                                Some(index) => close_log_dangling_error(
                                    &format!("- {index}"),
                                    index,
                                ),
                                None => close_log_star_dangling_error(),
                            });
                        }
                        let mut spec = close_spec_from_lex(
                            first.to_string(),
                            &lexed_selection,
                        );
                        spec.log = log_entries_from_lex(&lexed.entries);
                        let body = parent_trimmed.to_string();
                        return Ok(Some(parsed_capture_item_outcome(
                            item,
                            ParsedCaptureText {
                                body,
                                clip: None,
                                route: None,
                                kind: CaptureKind::PomodoroClose { spec },
                                scheduled_offset: None,
                                priority_level: None,
                                sub_bullets: Vec::new(),
                                dependencies: Vec::new(),
                                blocked_tasks: Vec::new(),
                                relations: Vec::new(),
                                dependency_target: None,
                                type_marker: None,
                            },
                            Vec::new(),
                            None,
                        )));
                    }
                }
            }
            // A parent line with an inline Work Log entry: the text after
            // the close token lexes through the shared inline lexer, so
            // `x<sel> [<n>] <text>` executes byte for byte like its bullet
            // form. The draft split already separated any trailing session
            // chain, so the tail here is exactly the entry.
            if forced_route.is_some() || forced_section.is_some() {
                return Err(POMODORO_CLOSE_FORCED_ERROR.to_string());
            }
            let lexed_selection = match selection_after_x {
                None => CloseSelectionLex {
                    in_progress: None,
                    park: Vec::new(),
                    complete: Vec::new(),
                    drop: Vec::new(),
                    park_all: false,
                    complete_all: false,
                    in_progress_range: None,
                    park_range: None,
                    complete_range: None,
                    drop_range: None,
                },
                Some(after_x) => {
                    let offset =
                        close_after_x_offset(parent_line, first, after_x);
                    match lex_close_selection(after_x, offset, first) {
                        Ok(CloseSelectionOutcome::Valid(lex)) => lex,
                        Ok(CloseSelectionOutcome::Incomplete(incomplete)) => {
                            return Err(close_selection_incomplete_error(
                                first,
                                incomplete.separator,
                            ));
                        }
                        Err(error) => return Err(error.message),
                    }
                }
            };
            let line_tokens = tokenize_line_with_spans(&parent_line.raw);
            let tail_tokens: Vec<Token<'_>> =
                if line_tokens.first().is_some_and(|token| token.text == first)
                {
                    line_tokens[1..].to_vec()
                } else {
                    // Fallback for leading-whitespace quirks: skip the close
                    // token by text match.
                    let mut skipped = false;
                    let mut tail = Vec::new();
                    for token in line_tokens {
                        if !skipped && token.text == first {
                            skipped = true;
                            continue;
                        }
                        if skipped {
                            tail.push(token);
                        }
                    }
                    tail
                };
            let default_index = default_log_index_with_all(
                lexed_selection.in_progress.as_deref(),
                &lexed_selection.park,
                &lexed_selection.complete,
                &lexed_selection.drop,
                lexed_selection.park_all,
                lexed_selection.complete_all,
            );
            let has_bullet_child = item.lines[1..].iter().any(|line| {
                !matches!(
                    classify_authored_line(line.raw),
                    AuthoredLineClass::EmptyOrPlaceholder
                )
            });
            match lex_close_inline_entry_with_all(
                &tail_tokens,
                first,
                lexed_selection.in_progress.as_deref(),
                &lexed_selection.park,
                &lexed_selection.complete,
                &lexed_selection.drop,
                lexed_selection.park_all,
                lexed_selection.complete_all,
            ) {
                Err(error) => Err(error.message),
                Ok(CloseInlineLex::Dangling { index, .. }) => {
                    if has_bullet_child {
                        let bullet = format!("- {index}");
                        return Err(close_inline_mixing_error(&bullet));
                    }
                    let head = format!("{first} {index}");
                    let plain = is_plain_whole_item_close(first);
                    Err(close_inline_dangling_error(&head, index, plain))
                }
                Ok(entry @ CloseInlineLex::Entry { .. }) => {
                    let (index, index_range, text) = match &entry {
                        CloseInlineLex::Entry {
                            index,
                            index_range,
                            text,
                            ..
                        } => (*index, *index_range, text.clone()),
                        CloseInlineLex::Dangling { .. } => unreachable!(),
                    };
                    if has_bullet_child {
                        // A defaulted inline entry beside unnumbered
                        // bullets suggests the unnumbered form: a numbered
                        // suggestion would itself fail as mixed numbering.
                        let bullet = if index_range.is_none()
                            && close_first_bullet_is_unnumbered(
                                &item.lines[1..],
                            ) {
                            format!("- {text}")
                        } else {
                            format!("- {} {text}", index.unwrap_or(1))
                        };
                        return Err(close_inline_mixing_error(&bullet));
                    }
                    let mut spec = close_spec_from_lex(
                        first.to_string(),
                        &lexed_selection,
                    );
                    spec.log = vec![log_entry_from_inline(
                        &entry,
                        first,
                        default_index,
                    )];
                    let body = first.to_string();
                    Ok(Some(parsed_capture_item_outcome(
                        item,
                        ParsedCaptureText {
                            body,
                            clip: None,
                            route: None,
                            kind: CaptureKind::PomodoroClose { spec },
                            scheduled_offset: None,
                            priority_level: None,
                            sub_bullets: Vec::new(),
                            dependencies: Vec::new(),
                            blocked_tasks: Vec::new(),
                            relations: Vec::new(),
                            dependency_target: None,
                            type_marker: None,
                        },
                        Vec::new(),
                        None,
                    )))
                }
            }
        }
        EqualsToken::Start {
            suffix,
            counted,
            len,
            name,
            name_spelling,
            name_is_literal,
            drop,
            timing,
            doubled,
        } => {
            let claimed = counted
                || name.is_some()
                || drop.is_some()
                || timing.is_some()
                || parent_trimmed.len() == len;
            if doubled && claimed {
                let token_text =
                    parent_trimmed.get(..len).unwrap_or(parent_trimmed);
                return Err(pomodoro_double_start_error(token_text));
            }
            if let Some(selector) = name {
                let spelling = name_spelling.unwrap_or(NameSpelling::Hash);
                let token_text =
                    parent_trimmed.get(..len).unwrap_or(parent_trimmed);
                let drop_text = drop.as_ref().map(|(_, text)| text.as_str());
                let timing_text =
                    timing.as_ref().map(|(_, text)| text.as_str());
                // Named diagnostics interpolate the token without the
                // optional parts; `=#~2` reports the empty-name error on
                // `=#`, and `=#@3` the same way.
                let first_opt = drop
                    .as_ref()
                    .map(|(offset, _)| *offset)
                    .into_iter()
                    .chain(timing.as_ref().map(|(offset, _)| *offset))
                    .min();
                let token_without_parts = match first_opt {
                    Some(offset) => {
                        token_text.get(..offset).unwrap_or(token_text)
                    }
                    None => token_text,
                };
                // Suffix and name errors win over the drop/timing lists.
                if let Some(error) = named_token_error(
                    token_without_parts,
                    &suffix,
                    &selector,
                    drop_text,
                    timing_text,
                    spelling,
                    name_is_literal,
                ) {
                    let exact =
                        parent_trimmed.len() == len && item.lines.len() == 1;
                    if exact {
                        return Err(error);
                    }
                    let has_extra = parent_trimmed.len() > len;
                    if selector.is_empty()
                        && has_extra
                        && let Some(nospace) =
                            named_nospace_error(&suffix, parent_trimmed, len)
                    {
                        return Err(nospace);
                    }
                    return Err(error);
                }
                // Both timings win over the dangling-`@` incomplete state.
                if let Some(error) = named_timing_twice_error(
                    token_text,
                    &suffix,
                    &selector,
                    timing_text,
                ) {
                    return Err(error);
                }
                // Dangling `@` is an incomplete error on execution.
                if let Some(text) = timing_text
                    && text.is_empty()
                {
                    return Err(start_timing_incomplete_error(token_text));
                }
                // A drop or timing part claims the item, like a counted token.
                let exact =
                    parent_trimmed.len() == len && item.lines.len() == 1;
                if !exact {
                    let has_extra = parent_trimmed.len() > len;
                    let has_children = item.lines.len() > 1;
                    if has_extra || has_children {
                        // A broken drop list reports its own error first; a
                        // dangling separator with extra text falls through
                        // to the no-spaces hint or the shape error.
                        if let Some((drop_offset, after_tilde)) = drop.as_ref()
                        {
                            let offset =
                                start_drop_offset(parent_line, *drop_offset);
                            if let Err(error) =
                                lex_start_drop(after_tilde, offset, token_text)
                            {
                                return Err(error.message);
                            }
                        }
                        if has_extra {
                            let nospace: String =
                                parent_trimmed.split_whitespace().collect();
                            if spaceless_start_drop_is_valid(&nospace) {
                                if spaceless_extra_is_timing(
                                    parent_trimmed,
                                    len,
                                ) {
                                    return Err(start_timing_no_spaces_error(
                                        &nospace,
                                    ));
                                }
                                return Err(start_drop_no_spaces_error());
                            }
                        }
                        return Err(named_shape_error_with_drop(
                            token_without_parts,
                            &suffix,
                            &selector,
                            parent_trimmed,
                            len,
                            drop_text,
                            spelling,
                            name_is_literal,
                        ));
                    }
                    return Ok(None);
                }
                let effective = effective_start_timing(&suffix, &timing);
                let mut spec = parse_pomodoro_start_suffix(&effective)?;
                if let Some((drop_offset, after_tilde)) = drop.as_ref() {
                    spec.drop = lex_start_drop_for_execution(
                        parent_line,
                        token_text,
                        *drop_offset,
                        after_tilde,
                    )?;
                }
                if forced_route.is_some() || forced_section.is_some() {
                    return Err(POMODORO_START_FORCED_ERROR.to_string());
                }
                let raw = parent_trimmed.to_string();
                return Ok(Some(parsed_capture_item_outcome(
                    item,
                    ParsedCaptureText {
                        body: raw,
                        clip: None,
                        route: None,
                        kind: CaptureKind::PomodoroStart {
                            spec,
                            pomodoro_name: Some(selector),
                            pomodoro_name_is_literal: name_is_literal,
                        },
                        scheduled_offset: None,
                        priority_level: None,
                        sub_bullets: Vec::new(),
                        dependencies: Vec::new(),
                        blocked_tasks: Vec::new(),
                        relations: Vec::new(),
                        dependency_target: None,
                        type_marker: None,
                    },
                    Vec::new(),
                    None,
                )));
            }
            // A drop or timing part claims the item, the same way a counted
            // token does. It is never prose, near misses included. A bare `=`
            // followed by separate text (`= ~2`, `= foo`) stays prose.
            let claims = counted || drop.is_some() || timing.is_some();
            let exact = parent_trimmed.len() == len && item.lines.len() == 1;
            if !exact {
                if claims
                    || (parent_trimmed.len() == len && item.lines.len() > 1)
                {
                    if let Some((message, _)) =
                        glued_timing_diagnostic(parent_trimmed, len, &suffix)
                    {
                        return Err(message);
                    }
                    let token_text =
                        parent_trimmed.get(..len).unwrap_or(parent_trimmed);
                    // Both timings win over dangling-`@`.
                    if !suffix.is_empty()
                        && let Some((_, timing_text)) = timing.as_ref()
                    {
                        let sigil = "=";
                        let drop_text = drop
                            .as_ref()
                            .map(|(_, text)| text.as_str())
                            .unwrap_or("");
                        let effective = if !timing_text.is_empty() {
                            timing_text.as_str()
                        } else {
                            suffix.as_str()
                        };
                        return Err(start_timing_twice_unnamed_error(
                            token_text, sigil, effective, drop_text,
                        ));
                    }
                    if let Some((_, timing_text)) = timing.as_ref()
                        && timing_text.is_empty()
                    {
                        return Err(start_timing_incomplete_error(token_text));
                    }
                    // A broken drop list reports its own error first; a
                    // dangling separator with extra text falls through to
                    // the no-spaces hint or the shape error, like closes.
                    if let Some((drop_offset, after_tilde)) = drop.as_ref() {
                        let offset =
                            start_drop_offset(parent_line, *drop_offset);
                        if let Err(error) =
                            lex_start_drop(after_tilde, offset, token_text)
                        {
                            return Err(error.message);
                        }
                        // Valid or dangling list with extra text or child
                        // lines: the no-spaces hint wins when the spaceless
                        // join lexes, else the existing shape error.
                        let nospace: String =
                            parent_trimmed.split_whitespace().collect();
                        if spaceless_start_drop_is_valid(&nospace) {
                            return Err(start_drop_no_spaces_error());
                        }
                    } else if parent_trimmed.len() > len {
                        let nospace: String =
                            parent_trimmed.split_whitespace().collect();
                        if spaceless_start_drop_is_valid(&nospace) {
                            return Err(start_drop_no_spaces_error());
                        }
                    }
                    return Err(pomodoro_start_shape_error(
                        token_text, &suffix,
                    ));
                }
                return Ok(None);
            }
            // Both timings (`=3~2@4`) are the timing-twice error.
            if !suffix.is_empty()
                && let Some((_, timing_text)) = timing.as_ref()
            {
                let sigil = "=";
                let drop_text =
                    drop.as_ref().map(|(_, text)| text.as_str()).unwrap_or("");
                let effective = if !timing_text.is_empty() {
                    timing_text.as_str()
                } else {
                    suffix.as_str()
                };
                let token_text = parent_trimmed;
                return Err(start_timing_twice_unnamed_error(
                    token_text, sigil, effective, drop_text,
                ));
            }
            if let Some((_, timing_text)) = timing.as_ref()
                && timing_text.is_empty()
            {
                let token_text = parent_trimmed;
                return Err(start_timing_incomplete_error(token_text));
            }
            // Suffix errors win over the drop list; the effective `<X>`
            // (leading or `@`) is validated.
            let effective = effective_start_timing(&suffix, &timing);
            let mut spec = parse_pomodoro_start_suffix(&effective)?;
            if let Some((drop_offset, after_tilde)) = drop.as_ref() {
                let token_text = parent_trimmed;
                spec.drop = lex_start_drop_for_execution(
                    parent_line,
                    token_text,
                    *drop_offset,
                    after_tilde,
                )?;
            }
            if forced_route.is_some() || forced_section.is_some() {
                return Err(POMODORO_START_FORCED_ERROR.to_string());
            }
            let raw = parent_trimmed.to_string();
            Ok(Some(parsed_capture_item_outcome(
                item,
                ParsedCaptureText {
                    body: raw,
                    clip: None,
                    route: None,
                    kind: CaptureKind::PomodoroStart {
                        spec,
                        pomodoro_name: None,
                        pomodoro_name_is_literal: false,
                    },
                    scheduled_offset: None,
                    priority_level: None,
                    sub_bullets: Vec::new(),
                    dependencies: Vec::new(),
                    blocked_tasks: Vec::new(),
                    relations: Vec::new(),
                    dependency_target: None,
                    type_marker: None,
                },
                Vec::new(),
                None,
            )))
        }
    }
}
