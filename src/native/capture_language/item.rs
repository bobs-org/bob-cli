//! Capture-item and session-operator parsing.

use super::close_log::*;
use super::close_selection::*;
use super::dependencies::*;
use super::draft::*;
use super::editor_parse::*;
use super::line::*;
use super::markers::*;
use super::model::*;
use super::project_tasks::*;
use super::start_selection::*;
use super::tokens::*;
use crate::native::url_routing::{
    classify_token, is_standalone_listen_marker, RefIntent, RoutingEntry,
};

pub(super) struct ParsedCaptureItemOutcome<'a> {
    pub(super) index: usize,
    pub(super) start: usize,
    pub(super) end: usize,
    pub(super) line_start: usize,
    pub(super) line_end: usize,
    pub(super) parsed: ParsedCaptureText,
    pub(super) declarations: Vec<GlobalDeclarationToken<'a>>,
    pub(super) local_destination_marker: Option<String>,
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
/// a known route token. A standalone `%` listen marker (or `@` alias) never resolves.
fn plain_ref_route(text: &str) -> Option<String> {
    if is_standalone_listen_marker(text) {
        return None;
    }
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
/// global parent during draft resolution), plus an optional standalone
/// `%` listen marker (or compatible `@` alias) immediately after the URL.
/// `URL %` requests companion audio; `URL % @route` and `@route URL %`
/// request it with an explicit parent. The compatible alias works in the
/// same positions. The body/URL intent excludes the route and listen
/// tokens. Policy-gated markers (`-R`, excluded hosts, disabled
/// routing, child lines, extra words/tags, schedule/priority/clipboard,
/// dependencies, operators, task refs, destination flags beyond `-r`)
/// retain existing task or diagnostic behavior. `URL @route %`,
/// `URL % %`, attached `path@`, queries/fragments, and encoded forms
/// stay unsupported and fall through.
fn claim_reference_item<'a>(
    item: &CaptureItem<'a>,
    options: &CaptureParseOptions<'_>,
    has_dependencies: bool,
    forced_route: Option<&str>,
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
        if let Some(forced) = forced_route {
            let route = normalize_forced_route(forced).ok()?;
            return Some(parsed_capture_item_outcome(
                item,
                ParsedCaptureText {
                    body: token.text.to_string(),
                    clip: None,
                    route: Some(route),
                    kind: CaptureKind::Ref(RefIntent::without_listen(intent)),
                    scheduled_offset: None,
                    priority_level: None,
                    sub_bullets: Vec::new(),
                    dependencies: Vec::new(),
                    blocked_tasks: Vec::new(),
                    relations: Vec::new(),
                    dependency_target: None,
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
                kind: CaptureKind::Ref(RefIntent::without_listen(intent)),
                scheduled_offset: None,
                priority_level: None,
                sub_bullets: Vec::new(),
                dependencies: Vec::new(),
                blocked_tasks: Vec::new(),
                relations: Vec::new(),
                dependency_target: None,
            },
            Vec::new(),
            None,
        ));
    }
    if tokens.len() == 2 {
        // `URL %` / `URL @`: listen with default/global/forced parent. The marker
        // is item-local and whitespace-delimited; attached `@` stays part
        // of the URL and never claims here.
        if is_standalone_listen_marker(tokens[1].text) {
            if let Some(intent) = classify_token(tokens[0].text) {
                if !policy.admits(&intent, RoutingEntry::Capture) {
                    return None;
                }
                if let Some(forced) = forced_route {
                    let route = normalize_forced_route(forced).ok()?;
                    return Some(parsed_capture_item_outcome(
                        item,
                        ParsedCaptureText {
                            body: tokens[0].text.to_string(),
                            clip: None,
                            route: Some(route),
                            kind: CaptureKind::Ref(RefIntent::with_listen(
                                intent,
                            )),
                            scheduled_offset: None,
                            priority_level: None,
                            sub_bullets: Vec::new(),
                            dependencies: Vec::new(),
                            blocked_tasks: Vec::new(),
                            relations: Vec::new(),
                            dependency_target: None,
                        },
                        Vec::new(),
                        Some(format!("--route {forced}")),
                    ));
                }
                return Some(parsed_capture_item_outcome(
                    item,
                    ParsedCaptureText {
                        body: tokens[0].text.to_string(),
                        clip: None,
                        route: None,
                        kind: CaptureKind::Ref(RefIntent::with_listen(intent)),
                        scheduled_offset: None,
                        priority_level: None,
                        sub_bullets: Vec::new(),
                        dependencies: Vec::new(),
                        blocked_tasks: Vec::new(),
                        relations: Vec::new(),
                        dependency_target: None,
                    },
                    Vec::new(),
                    None,
                ));
            }
        }
        if forced_route.is_some() {
            return None;
        }
        // Exactly one URL plus exactly one ordinary `@route`, in either
        // order. Anything else (extra words, tags, markers, `+`/`:`/`#`
        // shapes, a leading `@` listen marker) falls through to the
        // generic task path.
        if tokens
            .iter()
            .any(|token| is_standalone_listen_marker(token.text))
        {
            return None;
        }
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
        return Some(parsed_capture_item_outcome(
            item,
            ParsedCaptureText {
                body: url_token.text.to_string(),
                clip: None,
                route: Some(route),
                kind: CaptureKind::Ref(RefIntent::without_listen(intent)),
                scheduled_offset: None,
                priority_level: None,
                sub_bullets: Vec::new(),
                dependencies: Vec::new(),
                blocked_tasks: Vec::new(),
                relations: Vec::new(),
                dependency_target: None,
            },
            Vec::new(),
            Some(route_token.text.to_string()),
        ));
    }
    if tokens.len() == 3 && forced_route.is_none() {
        // `URL % @route` (trailing explicit parent with listen) and
        // `@route URL %` (leading explicit parent with listen). Every
        // other three-token shape (`URL @route %`, `URL % %`, extra
        // words) stays an ordinary task/diagnostic.
        if is_standalone_listen_marker(tokens[1].text) {
            let url_token = tokens[0];
            let route_token = tokens[2];
            let intent = classify_token(url_token.text)?;
            if !policy.admits(&intent, RoutingEntry::Capture) {
                return None;
            }
            let route = plain_ref_route(route_token.text)?;
            return Some(parsed_capture_item_outcome(
                item,
                ParsedCaptureText {
                    body: url_token.text.to_string(),
                    clip: None,
                    route: Some(route),
                    kind: CaptureKind::Ref(RefIntent::with_listen(intent)),
                    scheduled_offset: None,
                    priority_level: None,
                    sub_bullets: Vec::new(),
                    dependencies: Vec::new(),
                    blocked_tasks: Vec::new(),
                    relations: Vec::new(),
                    dependency_target: None,
                },
                Vec::new(),
                Some(route_token.text.to_string()),
            ));
        }
        if is_standalone_listen_marker(tokens[2].text) {
            let route_token = tokens[0];
            let url_token = tokens[1];
            // The middle token must be the URL and the last token the
            // standalone marker; a leading listen-marker shape never claims.
            if is_standalone_listen_marker(route_token.text) {
                return None;
            }
            let intent = classify_token(url_token.text)?;
            if !policy.admits(&intent, RoutingEntry::Capture) {
                return None;
            }
            let route = plain_ref_route(route_token.text)?;
            return Some(parsed_capture_item_outcome(
                item,
                ParsedCaptureText {
                    body: url_token.text.to_string(),
                    clip: None,
                    route: Some(route),
                    kind: CaptureKind::Ref(RefIntent::with_listen(intent)),
                    scheduled_offset: None,
                    priority_level: None,
                    sub_bullets: Vec::new(),
                    dependencies: Vec::new(),
                    blocked_tasks: Vec::new(),
                    relations: Vec::new(),
                    dependency_target: None,
                },
                Vec::new(),
                Some(route_token.text.to_string()),
            ));
        }
    }
    None
}

pub(super) fn parse_capture_item<'a>(
    item: &CaptureItem<'a>,
    forced_route: Option<&str>,
    forced_section: Option<&str>,
    options: &CaptureParseOptions<'_>,
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
    let item_dependencies =
        scan_item_dependencies(item, options.parse_clip_markers);
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
    if let Some(outcome) =
        claim_reference_item(item, options, has_dependencies, forced_route)
    {
        return Ok(outcome);
    }
    // Dependency modifiers leave the token stream before terminal-marker
    // extraction and route selection see it, exactly like the editor
    // line pass: the remaining tokens resolve like a draft without them.
    let parent_tokens = tokenize_line_with_spans(&parent_line.raw);
    let (parent_tokens, parent_line_dependencies) = extract_line_dependencies(
        parent_tokens,
        parent_line.raw.text,
        parent_line.raw.start,
        true,
        options.parse_clip_markers,
    );
    let parent_tokens = unescape_execution_tokens(
        parent_tokens,
        &parent_line_dependencies.escapes,
    );
    let parent_outcome = resolve_line(
        parent_tokens,
        true,
        detect_route,
        options.parse_clip_markers,
    )?;
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

    let mut aggregate = AggregateMarkers::default();
    aggregate.absorb(parent_outcome.markers, parent_outcome.route)?;

    let mut sub_bullets = Vec::new();
    let mut sub_bullet_lines = Vec::new();
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
        let (tokens, child_line_dependencies) = extract_line_dependencies(
            tokens,
            child_line.text,
            child_line.start,
            false,
            options.parse_clip_markers,
        );
        let tokens =
            unescape_execution_tokens(tokens, &child_line_dependencies.escapes);
        let outcome = resolve_line(
            tokens,
            false,
            detect_route,
            options.parse_clip_markers,
        )?;
        declarations.extend(global_declarations_from_tokens(
            outcome.declarations,
            line_number,
        ));
        if outcome.body.is_empty() {
            // A child holding only dependency modifiers contributes no
            // empty bullet; any other emptied child stays an error.
            if child_line_dependencies.stripped_tokens > 0 {
                continue;
            }
            return Err(empty_child_after_markers_error(line_number));
        }
        aggregate.absorb(outcome.markers, outcome.route)?;
        sub_bullets.push(AuthoredSubBullet {
            body: outcome.body,
            depth: authored.depth,
            task_id: None,
        });
        sub_bullet_lines.push(line_number);
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
                "Pomodoro link capture cannot be combined with --route, --section, --task, --task-ref, --task-section, or --clip; capture the link alone".to_string(),
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
                options.parse_clip_markers,
                !sub_bullets.is_empty(),
            ));
        }
        if forced_route.is_some() || forced_section.is_some() {
            return Err(
                "Pomodoro link capture cannot be combined with --route, --section, --task, --task-ref, --task-section, or --clip; capture the link alone".to_string(),
            );
        }
        if !has_dependencies {
            if aggregate.clip.is_some() {
                return Err(
                    "Pomodoro link capture cannot be combined with % clipboard markers".to_string(),
                );
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
        return Ok(parsed_capture_item_outcome(
            item,
            ParsedCaptureText {
                body: parent_outcome.body,
                clip: aggregate.clip,
                route: Some(route),
                kind: CaptureKind::Bullet {
                    section_prefix: Some(section.to_string()),
                    exact: true,
                },
                scheduled_offset: aggregate.scheduled_offset,
                priority_level: aggregate.priority_level,
                sub_bullets,
                dependencies: Vec::new(),
                blocked_tasks: Vec::new(),
                relations: Vec::new(),
                dependency_target: None,
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
    let (route, kind) = match aggregate.route {
        Some(line_route) => (line_route.token.route, line_route.token.kind),
        None => (None, CaptureKind::Task),
    };
    // Schedule, priority, and clipboard markers stay task-creation
    // modifiers: a dependency-only item carrying them mixes a second
    // action into an existing-task update instead of failing silently.
    let has_creation_modifiers = aggregate.clip.is_some()
        || aggregate.scheduled_offset.is_some()
        || aggregate.priority_level.is_some();
    if has_dependencies
        && parent_body_is_empty
        && (has_creation_modifiers || !sub_bullets.is_empty())
        && !owner_marker_is_suffixed(
            local_destination_marker.as_deref().unwrap_or(""),
        )
    {
        let parent_tokens = tokenize_line_with_spans(&parent_line.raw);
        return Err(refuse_relationship_needs_text(
            &item_dependencies,
            local_destination_marker.as_deref().unwrap_or("@route+id"),
            &parent_tokens,
            options.parse_clip_markers,
            !sub_bullets.is_empty(),
        ));
    }
    let kind = resolve_sub_bullet_kind(
        kind,
        parent_body_is_empty,
        sub_bullets.is_empty(),
        aggregate.clip.is_none()
            && aggregate.scheduled_offset.is_none()
            && aggregate.priority_level.is_none(),
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
    // ` :` task stays unused.
    let mut has_link_task = false;
    if matches!(kind, CaptureKind::ProjectNote { .. }) {
        let mut pass = ProjectTaskPass::new();
        if let Some(message) = pass.check_parent(&parent_outcome.body) {
            return Err(message);
        }
        for (index, child) in sub_bullets.iter_mut().enumerate() {
            let line_number = sub_bullet_lines[index];
            match pass.check_child(&child.body, child.depth, line_number) {
                ChildTaskOutcome::Ignore => {}
                ChildTaskOutcome::Unfinished { sigil } => {
                    return Err(unfinished_project_task_id_error(sigil));
                }
                ChildTaskOutcome::Error { message, .. } => {
                    return Err(message);
                }
                ChildTaskOutcome::Accept {
                    sigil,
                    id,
                    stripped,
                } => {
                    child.body = stripped;
                    child.task_id = Some(ProjectTaskId {
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
        ..
    } = &kind
        && !has_link_task
    {
        return Err(unused_project_note_pomodoro_error(name));
    }
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

/// Decide whether a resolved `@route+block-id[#name]` marker stays an
/// ordinary sub-bullet capture or becomes a task toggle, once the finished
/// item -- not just the marker's own token -- is known. A toggle needs an
/// empty parent body, no authored children, and no other item-wide marker;
/// an item that looks like a toggle but fails one of those still needs text,
/// exactly like it always has. Every other kind passes through unchanged.
pub(super) fn resolve_sub_bullet_kind(
    kind: CaptureKind,
    parent_body_is_empty: bool,
    sub_bullets_is_empty: bool,
    no_other_item_markers: bool,
) -> Result<CaptureKind, String> {
    if matches!(kind, CaptureKind::TaskToggle { .. }) {
        if parent_body_is_empty && sub_bullets_is_empty && no_other_item_markers
        {
            return Ok(kind);
        }
        return Err(EXPLICIT_TOGGLE_ONLY_MARKER_ERROR.to_string());
    }
    let CaptureKind::SubBullet {
        target: SubBulletTarget::BlockId(block_id),
        section,
    } = kind
    else {
        return Ok(kind);
    };
    if !parent_body_is_empty {
        if let Some(section) = &section
            && !is_selector_component(&section.text)
        {
            return Err(SUB_BULLET_SECTION_ERROR.to_string());
        }
        return Ok(CaptureKind::SubBullet {
            target: SubBulletTarget::BlockId(block_id),
            section,
        });
    }
    if sub_bullets_is_empty && no_other_item_markers {
        return Ok(CaptureKind::TaskToggle {
            block_id,
            pomodoro_name: section.map(|selector| selector.text),
            intent: TaskToggleIntent::EnsureNext,
        });
    }
    Err(missing_text_error())
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

/// A whole-item session token: either a close (a bare `x`, or a retired
/// `=x`/`=*`/`=!` spelling that teaches it) or a start (`=` plus an
/// `<X>`-shaped suffix and optional `#name` part).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum EqualsToken {
    /// A leading bare lowercase `x`, one of its claimed selection/hash near
    /// misses, or a retired `=x`/`=X`/`=*`/`=!` spelling. The retired
    /// spellings stay claimed so they fail with a teaching error instead of
    /// becoming a task.
    Close,
    /// `=` plus the longest `[0-9]*(-[0-9]*)?` run, plus an optional
    /// `#name` part, plus optional `@<X>` timing and `~<K>` drop parts in
    /// either order. `suffix` excludes the sigil (the leading `<X>`),
    /// `counted` is true when the effective `<X>` (leading or `@`) holds at
    /// least one digit, `name` is `Some` when a `#` immediately follows the
    /// suffix (possibly empty, as in `=#`; it ends at the first ASCII
    /// whitespace, `~`, or `@`), `drop` holds the `~` offset inside the
    /// token plus the text after it, `timing` holds the `@` offset plus the
    /// text after it, `len` is the token's byte length including the sigil,
    /// the name, and both optional parts, and `doubled` is true when the
    /// token used the doubled `==` sigil, which is now retired.
    Start {
        suffix: String,
        counted: bool,
        len: usize,
        name: Option<String>,
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

/// Claim a bare lowercase close at the first whitespace-delimited token, or
/// lex the `=`-family token at the start of trimmed item text, mirroring
/// [`session_operator_token`]. Returns `None` for ordinary prose. A retired
/// `=x`/`=*`/`=!` close lexes as `Close` too, so it reports its teaching
/// error. A bare start token (`=`, `=-`) has
/// an empty or digit-free suffix; a counted token (`=3`, `=-2`, `=3-`,
/// `=2-1`, `=0`) carries at least one digit. An optional `#name` part
/// immediately after the suffix ends at the first ASCII whitespace, `~`, or
/// `@`; the `#` must come immediately after the suffix. After the name, two
/// optional parts may follow, each at most once and in either order: an
/// `@<X>` timing part and a `~<K>` drop part. An unnamed token whose `~<K>`
/// part is present may also take an `@<X>` part after it. A token with no
/// `#name` and no `~<K>` never reads an `@`, so `=@3` stays prose and
/// `=3@2` keeps its shape error. Whichever optional part comes first ends
/// at whitespace or at the other part's sigil; the second runs to
/// whitespace. `len` covers every part.
pub(crate) fn session_equals_token(text: &str) -> Option<EqualsToken> {
    // Bare `x` closes only at the start of a whole-item token. Keep this
    // strict classifier ahead of the `=`-family lexer so starts and all
    // existing `=` spellings retain their current tokenization.
    let first = text.split_whitespace().next().unwrap_or(text);
    if classify_whole_item_close(first).is_some() || first.starts_with("x#") {
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
    if !doubled
        && rest
            .as_bytes()
            .first()
            .is_some_and(|byte| byte.eq_ignore_ascii_case(&b'x'))
    {
        return Some(EqualsToken::Close);
    }
    // The retired `=*`/`=!` shorthands claim the token even when their list
    // is malformed, so `=*abc` reports a retired-close error, never a task.
    if !doubled && (rest.starts_with('*') || rest.starts_with('!')) {
        return Some(EqualsToken::Close);
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
    if rest.as_bytes().get(len) == Some(&b'#') {
        let after_hash = &rest[len + 1..];
        let name_len = after_hash
            .find(|character: char| {
                character.is_ascii_whitespace()
                    || character == '~'
                    || character == '@'
            })
            .unwrap_or(after_hash.len());
        name = Some(after_hash[..name_len].to_string());
        token_len += 1 + name_len;
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
        drop,
        timing,
        doubled,
    })
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
        },
        Vec::new(),
        None,
    )))
}

/// A named start token's own error, in E1/E3/E2/overflow order. `None`
/// when the token itself is well-formed (callers then report E4 for extra
/// text or accept the exact token). `token` is the token without its
/// `~<K>` drop and `@<X>` timing parts; `drop_text`/`timing_text` keep the
/// typed parts in the order-error suggestion.
fn named_token_error(
    token: &str,
    suffix: &str,
    name: &str,
    drop_text: Option<&str>,
    timing_text: Option<&str>,
) -> Option<String> {
    if name.is_empty() {
        return Some(pomodoro_named_start_incomplete_error(token));
    }
    if let Some((before, after)) = name.split_once('=')
        && suffix.is_empty()
        && timing_text.is_none()
        && parse_pomodoro_start_suffix(after).is_ok()
    {
        let drop_suffix =
            drop_text.map(|text| format!("~{text}")).unwrap_or_default();
        let sigil = "=";
        return Some(format!(
            "write the duration after `@` or before the name: `{sigil}#{before}@{after}{drop_suffix}` or `{sigil}{after}#{before}{drop_suffix}` instead of `{token}`"
        ));
    }
    if !is_pomodoro_selector_component(name) {
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
/// typed `~<K>`.
pub(super) fn named_shape_error_with_drop(
    token: &str,
    suffix: &str,
    name: &str,
    parent_trimmed: &str,
    len: usize,
    drop_text: Option<&str>,
) -> String {
    if name.is_empty()
        && let Some(nospace) = named_nospace_error(suffix, parent_trimmed, len)
    {
        return nospace;
    }
    let mut message = pomodoro_named_start_shape_error(token, suffix, name);
    if !name.is_empty() && parent_trimmed.len() > len {
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
            let sigil = "=";
            message.push_str(&format!(
                "; to name a multi-word Pomodoro, join the words with `-`: `{sigil}{suffix}#{name}-{joined}{drop_suffix}`"
            ));
        }
    }
    message
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
    if let Some((_, after_tilde)) = drop {
        if lex_start_drop(&after_tilde, 0, nospace)
            .is_ok_and(|outcome| matches!(outcome, StartDropOutcome::Valid(_)))
            == false
        {
            return false;
        }
    }
    // A timing part, when present, must itself be a valid `<X>`.
    if let Some((_, after_at)) = timing {
        if parse_pomodoro_start_suffix(&after_at).is_err() {
            return false;
        }
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
                let name = first
                    .find('#')
                    .map(|hash| first[hash + 1..].to_string())
                    .unwrap_or_default();
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
                            return Err(close_log_dangling_error(
                                &format!("- {}", dangling.index),
                                dangling.index,
                            ));
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
                Err(error) => return Err(error.message),
                Ok(CloseInlineLex::Dangling { index, .. }) => {
                    if has_bullet_child {
                        let bullet = format!("- {index}");
                        return Err(close_inline_mixing_error(&bullet));
                    }
                    let head = format!("{first} {index}");
                    let plain = is_plain_whole_item_close(first);
                    return Err(close_inline_dangling_error(
                        &head, index, plain,
                    ));
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
                        },
                        Vec::new(),
                        None,
                    )));
                }
            }
        }
        EqualsToken::Start {
            suffix,
            counted,
            len,
            name,
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
                        },
                        scheduled_offset: None,
                        priority_level: None,
                        sub_bullets: Vec::new(),
                        dependencies: Vec::new(),
                        blocked_tasks: Vec::new(),
                        relations: Vec::new(),
                        dependency_target: None,
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
                    },
                    scheduled_offset: None,
                    priority_level: None,
                    sub_bullets: Vec::new(),
                    dependencies: Vec::new(),
                    blocked_tasks: Vec::new(),
                    relations: Vec::new(),
                    dependency_target: None,
                },
                Vec::new(),
                None,
            )))
        }
    }
}
