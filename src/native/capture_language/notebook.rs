//! Lexical notebook-route classification shared by capture and its editor.

use std::collections::HashMap;

use super::{
    editor_model::{Diagnostic, EditorMode, Need, Severity, Span, SpanKind},
    item::{CaptureItemParseError, ParsedCaptureItemOutcome},
    model::{
        AuthoredSubBullet, CaptureItem, CaptureKind, CaptureParseOptions,
        EditorParseOptions, FanoutContext, ParsedCaptureText,
    },
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NotebookRouteInfo {
    pub(crate) title: String,
    pub(crate) summary: Option<String>,
    pub(crate) owner: Option<String>,
    pub(crate) breadcrumb: Vec<String>,
}

/// Immutable notebook route lookup for a single parse/capture invocation.
/// The native adapter builds it only after finding route-shaped text.
#[derive(Debug, Clone, Default)]
pub(crate) struct NotebookRoutePolicy {
    routes: HashMap<String, NotebookRouteInfo>,
    /// Exact file bytes read while building the policy, keyed by
    /// lower-cased route. The writer compares these preimages against the
    /// planner's staged bytes so a retype between classification and
    /// planning aborts instead of writing the new version silently.
    sources: HashMap<String, String>,
}

impl NotebookRoutePolicy {
    pub(crate) fn absent() -> Self {
        Self::default()
    }

    #[cfg(test)]
    pub(crate) fn insert(&mut self, route: String, info: NotebookRouteInfo) {
        self.routes.insert(route.to_ascii_lowercase(), info);
    }

    pub(crate) fn insert_with_source(
        &mut self,
        route: String,
        info: NotebookRouteInfo,
        source: String,
    ) {
        let key = route.to_ascii_lowercase();
        self.routes.insert(key.clone(), info);
        self.sources.insert(key, source);
    }

    pub(crate) fn get(&self, route: &str) -> Option<&NotebookRouteInfo> {
        self.routes.get(&route.to_ascii_lowercase())
    }

    pub(crate) fn source(&self, route: &str) -> Option<&str> {
        self.sources
            .get(&route.to_ascii_lowercase())
            .map(String::as_str)
    }
}

/// Routes mentioned by capture syntax, in first-seen order. This is a
/// deliberately cheap lexical preflight: native callers use it to avoid
/// opening any note for an unrouted draft, and deduplicate repeated routes
/// before resolving notebook frontmatter.
fn collect_candidate_routes(
    raw_text: &str,
    seen: &mut std::collections::HashSet<String>,
    routes: &mut Vec<String>,
) {
    for token in raw_text.split_whitespace() {
        let Some(rest) = token.strip_prefix('@') else {
            continue;
        };
        let rest = rest.strip_prefix('@').unwrap_or(rest);
        let route: String = rest
            .chars()
            .take_while(|ch| {
                ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-')
            })
            .collect();
        if route.is_empty() || !super::is_route_token(&route) {
            continue;
        }
        let route = route.to_ascii_lowercase();
        if seen.insert(route.clone()) {
            routes.push(route);
        }
    }
}

pub(crate) fn candidate_notebook_routes(raw_text: &str) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    let mut routes = Vec::new();
    collect_candidate_routes(raw_text, &mut seen, &mut routes);
    let analysis = super::template::analyze(raw_text);
    if let Ok(Some(expansion)) =
        super::template::expand_with_analysis(raw_text, &analysis)
    {
        for instance in expansion.instances {
            collect_candidate_routes(&instance.text, &mut seen, &mut routes);
        }
    }
    routes
}

/// Forced `--route` destinations are lexical candidates too, even though
/// they make ordinary `@` tokens literal for this invocation.
pub(crate) fn candidate_notebook_route(route: &str) -> Option<String> {
    let route = route.trim();
    (super::is_route_token(route) && !route.starts_with('@'))
        .then(|| route.to_ascii_lowercase())
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum NotebookForm {
    Note {
        section: Option<String>,
        exact_section: bool,
        block_id: Option<String>,
        section_range: Option<(usize, usize)>,
        block_id_range: Option<(usize, usize)>,
    },
    SubNote {
        parent_block_id: String,
        block_id_range: Option<(usize, usize)>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RouteMarker {
    route: String,
    route_range: (usize, usize),
    token_range: (usize, usize),
    form: NotebookForm,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum RouteScan {
    NotNotebookRoute,
    Supported(RouteMarker),
    Unsupported {
        route: String,
        range: (usize, usize),
        code: &'static str,
        message: String,
    },
}

fn route_prefix(text: &str, start: usize) -> Option<(String, usize)> {
    let tail = text.get(start..)?.strip_prefix('@')?;
    if tail.starts_with('@') {
        return None;
    }
    let length = tail
        .char_indices()
        .take_while(|(_, ch)| {
            ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-')
        })
        .map(|(index, ch)| index + ch.len_utf8())
        .last()?;
    let route = &tail[..length];
    if !super::is_route_token(route) {
        return None;
    }
    Some((route.to_ascii_lowercase(), start + 1 + length))
}

fn scan_route_marker(text: &str, start: usize, end: usize) -> RouteScan {
    let Some((route, route_end)) = route_prefix(text, start) else {
        return RouteScan::NotNotebookRoute;
    };
    let Some(suffix) = text.get(route_end..end) else {
        return RouteScan::NotNotebookRoute;
    };
    let unsupported =
        |code: &'static str, message: &str| RouteScan::Unsupported {
            route: route.clone(),
            range: (start, end),
            code,
            message: message.to_string(),
        };
    if suffix.is_empty() {
        return RouteScan::Supported(RouteMarker {
            route,
            route_range: (start, route_end),
            token_range: (start, end),
            form: NotebookForm::Note {
                section: None,
                exact_section: false,
                block_id: None,
                section_range: None,
                block_id_range: None,
            },
        });
    }
    if suffix.starts_with('/') {
        // Record routes keep their established grammar even when the
        // folder name collides with a notebook.
        return RouteScan::NotNotebookRoute;
    }
    if suffix.starts_with(':') {
        return unsupported(
            "notebook_holds_no_tasks",
            &super::line::notebook_holds_no_tasks_error(&route, None),
        );
    }
    if suffix.starts_with('#') {
        let parsed = super::tokens::parse_hash_section_and_id(&suffix[1..]);
        let consumed = 1
            + parsed.section_len
            + if parsed.has_caret {
                1 + parsed.id_len
            } else {
                0
            };
        if consumed < suffix.len() {
            let rest = &suffix[consumed..];
            if rest.starts_with('#') {
                return unsupported(
                    "notebook_marker_unsupported",
                    &super::line::notebook_no_drawers_error(&route),
                );
            }
            return unsupported(
                "notebook_marker_unsupported",
                &super::line::notebook_marker_unsupported_error(
                    rest, &route, None,
                ),
            );
        }
        let section_end_abs = route_end + 1 + parsed.section_len;
        let block_id_range = parsed.has_caret.then_some((
            section_end_abs + 1,
            section_end_abs + 1 + parsed.id_len,
        ));
        let section_range = (parsed.section.is_some()
            || parsed.incomplete_section
            || parsed.quoted_incomplete)
            .then_some((route_end, section_end_abs));
        return RouteScan::Supported(RouteMarker {
            route,
            route_range: (start, route_end),
            token_range: (start, end),
            form: NotebookForm::Note {
                section: parsed.section,
                exact_section: parsed.exact,
                block_id: parsed.block_id,
                section_range,
                block_id_range,
            },
        });
    }
    if let Some(block_id) = suffix.strip_prefix('^') {
        if let Some(id) = block_id.strip_suffix('@') {
            return unsupported(
                "notebook_project_seal",
                &super::line::notebook_project_seal_error(&route, id),
            );
        }
        if block_id.contains('#') {
            return unsupported(
                "notebook_marker_unsupported",
                &super::line::notebook_no_drawers_error(&route),
            );
        }
        if block_id.contains(['+', ':', '=', '!']) {
            let marker = if block_id.contains('=') {
                "="
            } else if block_id.contains('!') {
                "!"
            } else if block_id.contains(':') {
                ":"
            } else {
                "+"
            };
            return unsupported(
                "notebook_marker_unsupported",
                &super::line::notebook_marker_unsupported_error(
                    marker, &route, None,
                ),
            );
        }
        let id_range = (route_end + 1, end);
        if !block_id.is_empty() && !super::tokens::is_block_id(block_id) {
            return unsupported("notebook_block_id_required", "notebook note block IDs must contain only A-Z, a-z, 0-9, or `-`");
        }
        return RouteScan::Supported(RouteMarker {
            route,
            route_range: (start, route_end),
            token_range: (start, end),
            form: NotebookForm::Note {
                section: None,
                exact_section: false,
                block_id: (!block_id.is_empty()).then(|| block_id.to_string()),
                section_range: None,
                block_id_range: Some(id_range),
            },
        });
    }
    if let Some(parent_block_id) = suffix.strip_prefix('+') {
        if parent_block_id.contains('#') {
            return unsupported(
                "notebook_marker_unsupported",
                &super::line::notebook_no_drawers_error(&route),
            );
        }
        if parent_block_id.ends_with('!')
            || parent_block_id.contains(['^', ':', '=', '!'])
        {
            let marker = if parent_block_id.contains('!') {
                "!"
            } else if parent_block_id.contains('=') {
                "="
            } else {
                ":"
            };
            return unsupported(
                "notebook_marker_unsupported",
                &super::line::notebook_marker_unsupported_error(
                    marker, &route, None,
                ),
            );
        }
        if !parent_block_id.is_empty()
            && !super::tokens::is_block_id(parent_block_id)
        {
            return unsupported("notebook_block_id_required", "notebook parent block IDs must contain only A-Z, a-z, 0-9, or `-`");
        }
        return RouteScan::Supported(RouteMarker {
            route,
            route_range: (start, route_end),
            token_range: (start, end),
            form: NotebookForm::SubNote {
                parent_block_id: parent_block_id.to_string(),
                block_id_range: Some((route_end + 1, end)),
            },
        });
    }
    let marker = suffix
        .chars()
        .next()
        .map(|ch| ch.to_string())
        .unwrap_or_default();
    unsupported(
        "notebook_marker_unsupported",
        &super::line::notebook_marker_unsupported_error(&marker, &route, None),
    )
}

fn quoted_section_marker_len(suffix: &str) -> Option<usize> {
    let after_hash = suffix.strip_prefix('#')?;
    if !super::pomodoro_literal::opens_exact_name(after_hash) {
        return None;
    }
    let lex = super::pomodoro_literal::lex_exact_name_after_hash(
        after_hash,
        after_hash.len(),
    )?;
    if !lex.complete {
        return Some(suffix.len());
    }
    let mut len = 1 + lex.literal_len;
    if let Some(id) = suffix.get(len..).and_then(|rest| rest.strip_prefix('^'))
    {
        let id_len = id.find(char::is_whitespace).unwrap_or(id.len());
        len += 1 + id_len;
    }
    Some(len)
}

fn is_type_marker_token(token: &str) -> bool {
    let Some(rest) = token.strip_prefix('#') else {
        return false;
    };
    if rest.is_empty() || super::pomodoro_literal::opens_exact_name(rest) {
        return true;
    }
    !rest.contains(['@', '^', '+', ':', '/', '=', '#'])
}

fn is_edge_transparent(token: &str) -> bool {
    super::directive::is_directive_token(token)
        || super::markers::parse_schedule_token(token).is_some()
        || super::markers::is_priority_marker_token(token)
        || token == "s:"
        || is_type_marker_token(token)
        || token.starts_with('&')
        || token.starts_with('*')
        || token.starts_with('~')
        || token == "!"
        || (token.starts_with('=') && token != "==")
}

fn skip_leading_transparent(text: &str) -> usize {
    let mut start = text.len() - text.trim_start().len();
    loop {
        let rest = text.get(start..).unwrap_or("");
        let Some(token) = rest.split_whitespace().next() else {
            return start;
        };
        if !is_edge_transparent(token) {
            return start;
        }
        let after = &rest[token.len()..];
        start += token.len() + (after.len() - after.trim_start().len());
    }
}

fn notebook_marker_end(text: &str, start: usize) -> Option<usize> {
    let (_, route_end) = route_prefix(text, start)?;
    let suffix = text.get(route_end..).unwrap_or("");
    if suffix.starts_with('#')
        && super::pomodoro_literal::opens_exact_name(&suffix[1..])
    {
        Some(
            route_end
                + quoted_section_marker_len(suffix).unwrap_or(suffix.len()),
        )
    } else {
        Some(
            text[start..]
                .find(char::is_whitespace)
                .map(|offset| start + offset)
                .unwrap_or(text.len()),
        )
    }
}

fn trailing_rest_is_transparent(rest: &str) -> bool {
    rest.split_whitespace().all(is_edge_transparent)
}

fn edge_marker(item: &CaptureItem<'_>) -> Option<RouteScan> {
    if item.list.is_some() {
        return None;
    }
    let parent = item.lines.first()?.raw;
    let text = parent.text;
    let leading = skip_leading_transparent(text);
    if text
        .get(leading..)
        .is_some_and(|rest| rest.starts_with('@'))
    {
        let marker_end = notebook_marker_end(text, leading)?;
        return Some(shift_scan(
            scan_route_marker(text, leading, marker_end),
            parent.start,
        ));
    }
    let trimmed_end = text.trim_end().len();
    let prefix = &text[..trimmed_end];
    for (relative, _) in prefix.match_indices('@').rev() {
        if relative == 0
            || !prefix[..relative]
                .chars()
                .next_back()
                .is_some_and(char::is_whitespace)
        {
            continue;
        }
        let candidate = &prefix[relative..];
        let Some(marker_end) = notebook_marker_end(candidate, 0) else {
            continue;
        };
        let rest = candidate.get(marker_end..).unwrap_or("");
        if !trailing_rest_is_transparent(rest) {
            continue;
        }
        return Some(shift_scan(
            scan_route_marker(text, relative, relative + marker_end),
            parent.start,
        ));
    }
    None
}

fn shift_scan(scan: RouteScan, offset: usize) -> RouteScan {
    let shift = |range: (usize, usize)| (range.0 + offset, range.1 + offset);
    match scan {
        RouteScan::Supported(mut marker) => {
            marker.route_range = shift(marker.route_range);
            marker.token_range = shift(marker.token_range);
            match &mut marker.form {
                NotebookForm::Note {
                    section_range,
                    block_id_range,
                    ..
                } => {
                    *section_range = section_range.map(shift);
                    *block_id_range = block_id_range.map(shift);
                }
                NotebookForm::SubNote { block_id_range, .. } => {
                    *block_id_range = block_id_range.map(shift);
                }
            }
            RouteScan::Supported(marker)
        }
        RouteScan::Unsupported {
            route,
            range,
            code,
            message,
        } => RouteScan::Unsupported {
            route,
            range: shift(range),
            code,
            message,
        },
        RouteScan::NotNotebookRoute => RouteScan::NotNotebookRoute,
    }
}

fn body_without_marker(item: &CaptureItem<'_>, marker: &RouteMarker) -> String {
    let Some(parent) = item.lines.first() else {
        return String::new();
    };
    let local_start = marker.token_range.0.saturating_sub(parent.raw.start);
    let local_end = marker.token_range.1.saturating_sub(parent.raw.start);
    let parent_body = if local_start == 0 {
        parent.raw.text.get(local_end..).unwrap_or("").trim()
    } else {
        parent.raw.text.get(..local_start).unwrap_or("").trim()
    };
    parent_body.to_string()
}

fn notebook_sub_bullets(
    item: &CaptureItem<'_>,
) -> Result<Vec<AuthoredSubBullet>, CaptureItemParseError> {
    let list = item.list.is_some();
    let mut sub_bullets = Vec::new();
    for line in item.lines.iter().skip(1) {
        match super::draft::classify_authored_line(line.raw) {
            super::draft::AuthoredLineClass::EmptyOrPlaceholder => continue,
            super::draft::AuthoredLineClass::Item(authored) => {
                sub_bullets.push(AuthoredSubBullet {
                    body: authored.body.trim().to_string(),
                    depth: if list {
                        super::model::AuthoredDepth::First
                    } else {
                        authored.depth
                    },
                    task_id: None,
                    move_task: None,
                    move_choice: None,
                    drawer: None,
                });
            }
            super::draft::AuthoredLineClass::Invalid => {
                return Err(CaptureItemParseError::coded(
                    "invalid_child_line",
                    super::line::invalid_child_line_error(line.line_number),
                ));
            }
        }
    }
    Ok(sub_bullets)
}

#[derive(Debug, Clone)]
struct NotebookClaim {
    marker: RouteMarker,
    body: String,
    info: NotebookRouteInfo,
    inherited: bool,
}

fn classify_item(
    item: &CaptureItem<'_>,
    policy: Option<&NotebookRoutePolicy>,
    global_route: Option<&str>,
) -> Result<Option<NotebookClaim>, RouteScan> {
    let Some(policy) = policy else {
        return Ok(None);
    };
    if let Some(scan) = edge_marker(item) {
        let route = match &scan {
            RouteScan::NotNotebookRoute => return Ok(None),
            RouteScan::Supported(marker) => marker.route.as_str(),
            RouteScan::Unsupported { route, .. } => route,
        };
        let Some(info) = policy.get(route).cloned() else {
            return Ok(None);
        };
        return match scan {
            RouteScan::Supported(marker) => Ok(Some(NotebookClaim {
                body: body_without_marker(item, &marker),
                marker,
                info,
                inherited: false,
            })),
            refusal @ RouteScan::Unsupported { .. } => Err(refusal),
            RouteScan::NotNotebookRoute => Ok(None),
        };
    }
    let Some(route) = global_route else {
        return Ok(None);
    };
    let Some(info) = policy.get(route).cloned() else {
        return Ok(None);
    };
    Ok(Some(NotebookClaim {
        body: body_without_globals(item),
        marker: inherited_marker(route.to_string(), item),
        info,
        inherited: true,
    }))
}

fn inherited_marker(route: String, item: &CaptureItem<'_>) -> RouteMarker {
    let range = item
        .lines
        .first()
        .map(|line| (line.raw.start, line.raw.end))
        .unwrap_or((item.start, item.end));
    RouteMarker {
        route,
        route_range: range,
        token_range: range,
        form: NotebookForm::Note {
            section: None,
            exact_section: false,
            block_id: None,
            section_range: None,
            block_id_range: None,
        },
    }
}

fn body_without_globals(item: &CaptureItem<'_>) -> String {
    let Some(parent) = item.lines.first() else {
        return String::new();
    };
    let tokens = super::editor_parse::tokenize_line_with_spans(&parent.raw);
    let body: String = tokens
        .into_iter()
        .filter(|token| !token.text.starts_with("@@"))
        .map(|token| token.text)
        .collect::<Vec<_>>()
        .join(" ");
    body.trim().to_string()
}

fn notebook_kind(claim: &NotebookClaim) -> CaptureKind {
    match &claim.marker.form {
        NotebookForm::Note {
            section,
            exact_section,
            block_id,
            ..
        } => CaptureKind::NotebookNote {
            section: section.clone(),
            exact_section: *exact_section,
            block_id: block_id.clone(),
        },
        NotebookForm::SubNote {
            parent_block_id, ..
        } => CaptureKind::NotebookSubNote {
            parent_block_id: parent_block_id.clone(),
        },
    }
}

fn strip_clip_tokens(body: &str) -> String {
    super::model::tokenize_with_spans(body)
        .into_iter()
        .filter(|token| {
            !token
                .text
                .strip_prefix("%c")
                .is_some_and(|rest| rest.chars().all(|ch| ch.is_ascii_digit()))
        })
        .map(|token| token.text)
        .collect::<Vec<_>>()
        .join(" ")
}

pub(super) fn strip_trailing_caret_id(body: &str) -> (String, Option<String>) {
    let mut tokens: Vec<&str> = body.split_whitespace().collect();
    let Some(last) = tokens.last() else {
        return (body.trim().to_string(), None);
    };
    if let Some(id) = last.strip_prefix('^')
        && super::tokens::is_block_id(id)
    {
        let id = id.to_string();
        tokens.pop();
        return (tokens.join(" "), Some(id));
    }
    (body.trim().to_string(), None)
}

fn owner_of(info: &NotebookRouteInfo) -> Option<&str> {
    info.owner.as_deref()
}

fn reformat_unsupported(
    code: &'static str,
    message: String,
    route: &str,
    owner: Option<&str>,
) -> (&'static str, String) {
    match code {
        "notebook_holds_no_tasks" => (
            code,
            super::line::notebook_holds_no_tasks_error(route, owner),
        ),
        "notebook_marker_unsupported" => (code, message),
        "notebook_project_seal" => (code, message),
        _ => (code, message),
    }
}

/// A notebook route used as a task home (`>`, `<`, `@nb:id`).
pub(super) struct NotebookTaskRouteError {
    pub(super) route: String,
    message: String,
    span: Option<(usize, usize)>,
}

impl NotebookTaskRouteError {
    pub(super) fn into_parse_error(self) -> CaptureItemParseError {
        CaptureItemParseError::coded_at(
            "notebook_holds_no_tasks",
            self.message,
            self.span,
        )
    }

    pub(super) fn into_editor_view(self) -> NotebookEditorView {
        notebook_editor_error(
            self.route,
            "notebook_holds_no_tasks",
            self.message,
            self.span,
        )
    }
}

pub(super) fn notebook_task_route_error(
    policy: Option<&NotebookRoutePolicy>,
    route: &str,
    span: Option<(usize, usize)>,
) -> Option<NotebookTaskRouteError> {
    let info = policy?.get(route)?;
    Some(NotebookTaskRouteError {
        route: route.to_ascii_lowercase(),
        message: super::line::notebook_holds_no_tasks_error(
            route,
            info.owner.as_deref(),
        ),
        span,
    })
}

pub(super) fn first_notebook_task_route<'a>(
    policy: Option<&NotebookRoutePolicy>,
    routes: impl IntoIterator<Item = (&'a str, Option<(usize, usize)>)>,
) -> Option<NotebookTaskRouteError> {
    routes.into_iter().find_map(|(route, span)| {
        notebook_task_route_error(policy, route, span)
    })
}

fn item_is_claimed_operator(item: &CaptureItem<'_>) -> bool {
    let Some(parent) = item.lines.first() else {
        return false;
    };
    let has_children = item.lines.len() > 1;
    !matches!(
        super::transfer::scan_transfer_line(
            &parent.raw,
            item.source,
            has_children,
        ),
        super::transfer::TransferScan::Unclaimed
            | super::transfer::TransferScan::Escaped { .. }
    ) || !matches!(
        super::merge_in::scan_merge_in_line(
            &parent.raw,
            item.source,
            has_children,
        ),
        super::merge_in::MergeInScan::Unclaimed
            | super::merge_in::MergeInScan::Escaped { .. }
    )
}

fn refuse_notebook_item_markers(
    item: &CaptureItem<'_>,
    route: &str,
    owner: Option<&str>,
    marker_range: (usize, usize),
) -> Result<Option<super::model::ClipRequest>, CaptureItemParseError> {
    let occurrences = super::directive::collect_item(item);
    let directives = match super::directive::resolve(&occurrences) {
        Ok(directives) => directives,
        Err((message, code, span)) => {
            return Err(CaptureItemParseError::coded_at(
                code,
                message,
                Some((span.start, span.end)),
            ));
        }
    };
    if directives.listen {
        return Err(CaptureItemParseError::coded_at(
            "notebook_marker_unsupported",
            super::line::notebook_marker_unsupported_error("%l", route, owner),
            Some(marker_range),
        ));
    }
    if directives.note {
        return Err(CaptureItemParseError::coded_at(
            "pomodoro_note_conflict",
            super::line::pomodoro_note_route_conflict_error(),
            Some(marker_range),
        ));
    }
    let types = super::type_marker::collect_item(item);
    if let Some(occurrence) = types.iter().find(|item| item.is_marker()) {
        let raw = match occurrence {
            super::type_marker::TypeMarkerOccurrence::Complete(marker) => {
                format!("#{}", marker.name)
            }
            super::type_marker::TypeMarkerOccurrence::Incomplete { .. } => {
                "#".to_string()
            }
            super::type_marker::TypeMarkerOccurrence::Invalid {
                raw, ..
            } => raw.clone(),
            super::type_marker::TypeMarkerOccurrence::Escaped { .. } => {
                String::new()
            }
        };
        return Err(CaptureItemParseError::coded_at(
            "notebook_marker_unsupported",
            super::line::notebook_marker_unsupported_error(&raw, route, owner),
            Some(marker_range),
        ));
    }
    let deps = super::dependencies::scan_item_dependencies(item);
    if deps.has_entries() || deps.has_partial_tokens() {
        return Err(CaptureItemParseError::coded_at(
            "notebook_holds_no_tasks",
            super::line::notebook_holds_no_tasks_error(route, owner),
            Some(marker_range),
        ));
    }
    if let Some(parent) = item.lines.first() {
        for token in super::editor_parse::tokenize_line_with_spans(&parent.raw)
        {
            if super::markers::parse_schedule_token(token.text).is_some() {
                return Err(CaptureItemParseError::coded_at(
                    "notebook_marker_unsupported",
                    super::line::notebook_marker_unsupported_error(
                        "s:N", route, owner,
                    ),
                    Some((token.start, token.end)),
                ));
            }
            if super::markers::parse_priority_token(token.text).is_some() {
                return Err(CaptureItemParseError::coded_at(
                    "notebook_marker_unsupported",
                    super::line::notebook_marker_unsupported_error(
                        "$N", route, owner,
                    ),
                    Some((token.start, token.end)),
                ));
            }
            if token.text == "!"
                || (token.text.starts_with('=') && token.text != "==")
            {
                let marker = if token.text == "!" { "!" } else { "=" };
                return Err(CaptureItemParseError::coded_at(
                    "notebook_marker_unsupported",
                    super::line::notebook_marker_unsupported_error(
                        marker, route, owner,
                    ),
                    Some((token.start, token.end)),
                ));
            }
        }
    }
    Ok(directives.clip)
}

fn finish_notebook_claim<'a>(
    item: &CaptureItem<'a>,
    mut claim: NotebookClaim,
    clip: Option<super::model::ClipRequest>,
    local_marker: Option<String>,
) -> Result<ParsedCaptureItemOutcome<'a>, CaptureItemParseError> {
    match &claim.marker.form {
        NotebookForm::Note {
            section: None,
            section_range: Some(_),
            ..
        } => {
            return Err(CaptureItemParseError::coded_at(
                "notebook_section_required",
                "a notebook section marker needs a section name".to_string(),
                Some(claim.marker.token_range),
            ));
        }
        NotebookForm::Note {
            section: Some(section),
            ..
        } if section.is_empty() => {
            return Err(CaptureItemParseError::coded_at(
                "notebook_section_required",
                "a notebook section marker needs a section name".to_string(),
                Some(claim.marker.token_range),
            ));
        }
        NotebookForm::Note {
            block_id_range: Some(_),
            block_id: None,
            ..
        } => {
            return Err(CaptureItemParseError::coded_at(
                "notebook_block_id_required",
                "a notebook note route needs a block ID after `^` or `+`"
                    .to_string(),
                Some(claim.marker.token_range),
            ));
        }
        NotebookForm::SubNote {
            parent_block_id, ..
        } if parent_block_id.is_empty() => {
            return Err(CaptureItemParseError::coded_at(
                "notebook_block_id_required",
                "a notebook note route needs a block ID after `^` or `+`"
                    .to_string(),
                Some(claim.marker.token_range),
            ));
        }
        _ => {}
    }
    if clip.is_some() {
        claim.body = strip_clip_tokens(&claim.body);
    }
    if let NotebookForm::Note { block_id, .. } = &mut claim.marker.form
        && block_id.is_none()
    {
        let (body, trailing) = strip_trailing_caret_id(&claim.body);
        if trailing.is_some() {
            claim.body = body;
            *block_id = trailing;
        }
    }
    let sub_bullets = notebook_sub_bullets(item)?;
    let empty_ok = !sub_bullets.is_empty()
        || clip.is_some()
        || matches!(claim.marker.form, NotebookForm::SubNote { .. });
    if claim.body.trim().is_empty() && !empty_ok {
        return Err(CaptureItemParseError::coded_at(
            "notebook_note_required",
            "a notebook route needs note text; add the note after the route"
                .to_string(),
            Some(claim.marker.token_range),
        ));
    }
    let route = claim.marker.route.clone();
    let kind = notebook_kind(&claim);
    let local_marker = if claim.inherited {
        None
    } else {
        local_marker.or_else(|| {
            item.source
                .get(claim.marker.token_range.0..claim.marker.token_range.1)
                .map(|text| text.trim().to_string())
        })
    };
    Ok(super::item::parsed_capture_item_outcome(
        item,
        ParsedCaptureText {
            body: claim.body,
            clip,
            route: Some(route),
            kind,
            scheduled_offset: None,
            priority_level: None,
            sub_bullets,
            dependencies: Vec::new(),
            blocked_tasks: Vec::new(),
            relations: Vec::new(),
            dependency_target: None,
            type_marker: None,
            merge_in: None,
        },
        Vec::new(),
        local_marker,
    ))
}

fn claim_forced_notebook<'a>(
    item: &CaptureItem<'a>,
    forced_route: &str,
    forced_section: Option<&str>,
    options: &CaptureParseOptions<'_>,
) -> Result<Option<ParsedCaptureItemOutcome<'a>>, CaptureItemParseError> {
    let Some(policy) = options.notebook_policy else {
        return Ok(None);
    };
    let Some(info) = policy.get(forced_route) else {
        return Ok(None);
    };
    let route = forced_route.to_ascii_lowercase();
    let owner = owner_of(info);
    if options.forced_drawer.is_some() {
        return Err(CaptureItemParseError::coded(
            "notebook_marker_unsupported",
            super::line::notebook_no_drawers_error(&route),
        ));
    }
    let range = item
        .lines
        .first()
        .map(|line| (line.raw.start, line.raw.end))
        .unwrap_or((item.start, item.end));
    let clip = refuse_notebook_item_markers(item, &route, owner, range)?;
    let body = item
        .lines
        .first()
        .map(|line| line.raw.text.trim().to_string())
        .unwrap_or_default();
    let form = if let Some(parent) = options.forced_task {
        if parent.is_empty() || !super::tokens::is_block_id(parent) {
            return Err(CaptureItemParseError::coded(
                "notebook_block_id_required",
                "a notebook note route needs a block ID after `^` or `+`"
                    .to_string(),
            ));
        }
        NotebookForm::SubNote {
            parent_block_id: parent.to_string(),
            block_id_range: None,
        }
    } else {
        NotebookForm::Note {
            section: forced_section.map(str::to_string),
            exact_section: forced_section.is_some(),
            block_id: None,
            section_range: None,
            block_id_range: None,
        }
    };
    let claim = NotebookClaim {
        body,
        marker: RouteMarker {
            route,
            route_range: range,
            token_range: range,
            form,
        },
        info: info.clone(),
        inherited: false,
    };
    finish_notebook_claim(item, claim, clip, None).map(Some)
}

fn claim_list_notebook<'a>(
    item: &CaptureItem<'a>,
    options: &CaptureParseOptions<'_>,
) -> Result<Option<ParsedCaptureItemOutcome<'a>>, CaptureItemParseError> {
    let Some(list) = item.list.as_ref() else {
        return Ok(None);
    };
    let Some(policy) = options.notebook_policy else {
        return Ok(None);
    };
    let Some(info) = policy.get(&list.route) else {
        return Ok(None);
    };
    let route = list.route.clone();
    let owner = owner_of(info);
    let header_range = (list.header_start, list.header_end);
    if list.ownerless {
        return Ok(None);
    }
    if list.count == 0 && list.kind == super::model::ListKind::Tasks {
        return Err(CaptureItemParseError::coded_at(
            "notebook_holds_no_tasks",
            super::line::notebook_holds_no_tasks_error(&route, owner),
            Some(header_range),
        ));
    }
    if list.count == 0 {
        let header = item
            .source
            .get(list.header_start..list.header_end)
            .unwrap_or("")
            .trim();
        return Err(CaptureItemParseError::coded(
            "empty_list",
            super::project_tasks::empty_list_error(header, true),
        ));
    }
    let clip = refuse_notebook_item_markers(item, &route, owner, header_range)?;
    let body = item
        .lines
        .first()
        .map(|line| line.raw.text.trim().to_string())
        .unwrap_or_default();
    let claim = NotebookClaim {
        body,
        marker: RouteMarker {
            route,
            route_range: header_range,
            token_range: header_range,
            form: NotebookForm::Note {
                section: list.section.clone(),
                exact_section: list.exact,
                block_id: None,
                section_range: None,
                block_id_range: None,
            },
        },
        info: info.clone(),
        inherited: false,
    };
    finish_notebook_claim(item, claim, clip, Some(format!("@{}", list.route)))
        .map(Some)
}

pub(super) fn claim_strict_notebook<'a>(
    item: &CaptureItem<'a>,
    forced_route: Option<&str>,
    forced_section: Option<&str>,
    options: &CaptureParseOptions<'_>,
) -> Result<Option<ParsedCaptureItemOutcome<'a>>, CaptureItemParseError> {
    if let Some(forced) = forced_route {
        return claim_forced_notebook(item, forced, forced_section, options);
    }
    if forced_section.is_some() || options.explicit_destination {
        return Ok(None);
    }
    if let Some(outcome) = claim_list_notebook(item, options)? {
        return Ok(Some(outcome));
    }
    if item_is_claimed_operator(item) {
        return Ok(None);
    }
    let claim = match classify_item(
        item,
        options.notebook_policy,
        options.global_route,
    ) {
        Ok(claim) => claim,
        Err(RouteScan::Unsupported {
            message,
            range,
            code,
            route,
        }) => {
            let owner = options
                .notebook_policy
                .and_then(|policy| policy.get(&route))
                .and_then(|info| info.owner.clone());
            let (code, message) =
                reformat_unsupported(code, message, &route, owner.as_deref());
            return Err(CaptureItemParseError::coded_at(
                code,
                message,
                Some(range),
            ));
        }
        Err(RouteScan::Supported(_)) | Err(RouteScan::NotNotebookRoute) => None,
    };
    let Some(claim) = claim else {
        return Ok(None);
    };
    let owner = owner_of(&claim.info);
    let clip = refuse_notebook_item_markers(
        item,
        &claim.marker.route,
        owner,
        claim.marker.token_range,
    )?;
    finish_notebook_claim(item, claim, clip, None).map(Some)
}

/// Fan out a marker-only `@nb+id` plus first-level bullets into one
/// sub-note per bullet. Ordinary `@route+id` list headers stay unchanged.
pub(super) fn claim_notebook_members<'a>(
    item: &CaptureItem<'a>,
    forced_route: Option<&str>,
    forced_section: Option<&str>,
    options: &CaptureParseOptions<'_>,
) -> Result<Option<Vec<ParsedCaptureItemOutcome<'a>>>, CaptureItemParseError> {
    if forced_route.is_some()
        || forced_section.is_some()
        || options.explicit_destination
        || item.list.is_some()
    {
        if let Some(outcome) =
            claim_strict_notebook(item, forced_route, forced_section, options)?
        {
            return Ok(Some(vec![outcome]));
        }
        return Ok(None);
    }
    if let Some(outcomes) = fanout_notebook_plus(item, options)? {
        return Ok(Some(outcomes));
    }
    if let Some(outcome) =
        claim_strict_notebook(item, forced_route, forced_section, options)?
    {
        return Ok(Some(vec![outcome]));
    }
    Ok(None)
}

fn fanout_notebook_plus<'a>(
    item: &CaptureItem<'a>,
    options: &CaptureParseOptions<'_>,
) -> Result<Option<Vec<ParsedCaptureItemOutcome<'a>>>, CaptureItemParseError> {
    let Some(policy) = options.notebook_policy else {
        return Ok(None);
    };
    let Some(parent) = item.lines.first() else {
        return Ok(None);
    };
    let trimmed = parent.raw.text.trim();
    let Some(marker) = trimmed.strip_prefix('@') else {
        return Ok(None);
    };
    if marker.starts_with('@') {
        return Ok(None);
    }
    let Some((route_part, rest)) = marker.split_once('+') else {
        return Ok(None);
    };
    if rest.contains(['#', '^', ':', '=', '!', '/']) {
        return Ok(None);
    }
    if !super::markers::is_route_token(route_part)
        || !super::tokens::is_block_id(rest)
    {
        return Ok(None);
    }
    if policy.get(route_part).is_none() {
        return Ok(None);
    }
    if trimmed.split_whitespace().nth(1).is_some() {
        return Ok(None);
    }
    let first_level = item.lines.iter().skip(1).any(|line| {
        matches!(
            super::draft::classify_authored_line(line.raw),
            super::draft::AuthoredLineClass::Item(authored)
                if authored.depth == super::model::AuthoredDepth::First
        )
    });
    if !first_level {
        return Ok(None);
    }
    let mut outcomes = Vec::new();
    let mut current: Vec<super::model::ItemLine<'a>> = Vec::new();
    let flush = |block: &[super::model::ItemLine<'a>],
                 outcomes: &mut Vec<ParsedCaptureItemOutcome<'a>>|
     -> Result<(), CaptureItemParseError> {
        let Some(first) = block.first() else {
            return Ok(());
        };
        let authored = match super::draft::classify_authored_line(first.raw) {
            super::draft::AuthoredLineClass::Item(authored)
                if authored.depth == super::model::AuthoredDepth::First =>
            {
                authored
            }
            _ => return Ok(()),
        };
        let child = CaptureItem {
            source: item.source,
            index: item.index,
            start: first.raw.start,
            end: block
                .last()
                .map(|line| line.raw.end)
                .unwrap_or(first.raw.end),
            line_start: first.line_number,
            line_end: block
                .last()
                .map(|line| line.line_number)
                .unwrap_or(first.line_number),
            lines: std::iter::once(super::model::ItemLine {
                raw: super::model::RawLine {
                    text: authored.body,
                    start: authored.body_start,
                    end: first.raw.end,
                },
                line_number: first.line_number,
            })
            .chain(block.iter().skip(1).copied())
            .collect(),
            list: None,
        };
        let info = policy
            .get(route_part)
            .cloned()
            .expect("notebook plus fan-out checks the policy");
        let claim = NotebookClaim {
            body: authored.body.trim().to_string(),
            marker: RouteMarker {
                route: route_part.to_ascii_lowercase(),
                route_range: (parent.raw.start, parent.raw.end),
                token_range: (parent.raw.start, parent.raw.end),
                form: NotebookForm::SubNote {
                    parent_block_id: rest.to_string(),
                    block_id_range: None,
                },
            },
            info,
            inherited: false,
        };
        let owner = owner_of(&claim.info);
        let clip = refuse_notebook_item_markers(
            &child,
            &claim.marker.route,
            owner,
            claim.marker.token_range,
        )?;
        outcomes.push(finish_notebook_claim(
            &child,
            claim,
            clip,
            Some(trimmed.to_string()),
        )?);
        Ok(())
    };
    for line in item.lines.iter().skip(1).copied() {
        match super::draft::classify_authored_line(line.raw) {
            super::draft::AuthoredLineClass::EmptyOrPlaceholder => continue,
            super::draft::AuthoredLineClass::Item(authored)
                if authored.depth == super::model::AuthoredDepth::First =>
            {
                if !current.is_empty() {
                    flush(&current, &mut outcomes)?;
                    current.clear();
                }
                current.push(line);
            }
            _ => current.push(line),
        }
    }
    if !current.is_empty() {
        flush(&current, &mut outcomes)?;
    }
    let count = outcomes.len();
    let marker = trimmed.to_string();
    let line = parent.line_number;
    for (index, outcome) in outcomes.iter_mut().enumerate() {
        outcome.fanout = Some(FanoutContext {
            line,
            position: index + 1,
            count,
            marker: marker.clone(),
        });
    }
    Ok(Some(outcomes))
}

pub(crate) struct NotebookEditorView {
    pub(crate) mode: EditorMode,
    pub(crate) body: String,
    pub(crate) route: String,
    pub(crate) section: Option<String>,
    pub(crate) block_id: Option<String>,
    pub(crate) needs: Vec<Need>,
    pub(crate) spans: Vec<Span>,
    pub(crate) diagnostics: Vec<Diagnostic>,
    pub(crate) sub_bullets: Vec<AuthoredSubBullet>,
}

pub(crate) fn editor_view_for_item(
    item: &CaptureItem<'_>,
    options: &EditorParseOptions<'_>,
) -> Option<NotebookEditorView> {
    if let Some(list) = item.list.as_ref()
        && let Some(policy) = options.notebook_policy
        && let Some(info) = policy.get(&list.route)
    {
        let owner = info.owner.as_deref();
        let header_range = (list.header_start, list.header_end);
        if list.count == 0 && list.kind == super::model::ListKind::Tasks {
            return Some(notebook_editor_error(
                list.route.clone(),
                "notebook_holds_no_tasks",
                super::line::notebook_holds_no_tasks_error(&list.route, owner),
                Some(header_range),
            ));
        }
        if list.count == 0 {
            let header = item
                .source
                .get(list.header_start..list.header_end)
                .unwrap_or("")
                .trim();
            return Some(notebook_editor_error(
                list.route.clone(),
                "empty_list",
                super::project_tasks::empty_list_error(header, true),
                Some(header_range),
            ));
        }
        let clip = match refuse_notebook_item_markers(
            item,
            &list.route,
            owner,
            header_range,
        ) {
            Ok(clip) => clip,
            Err(error) => {
                return Some(notebook_editor_error(
                    list.route.clone(),
                    error.code.unwrap_or("notebook_marker_unsupported"),
                    error.message,
                    error.span.or(Some(header_range)),
                ));
            }
        };
        let body = item
            .lines
            .first()
            .map(|line| line.raw.text.trim().to_string())
            .unwrap_or_default();
        let (mut body, trailing) = strip_trailing_caret_id(&body);
        if clip.is_some() {
            body = strip_clip_tokens(&body);
        }
        let mut needs = Vec::new();
        if body.trim().is_empty() && trailing.is_none() {
            needs.push(Need::NotebookNote);
        }
        let sub_bullets = notebook_sub_bullets(item).unwrap_or_default();
        return Some(NotebookEditorView {
            mode: EditorMode::NotebookNote,
            body,
            route: list.route.clone(),
            section: list.section.clone(),
            block_id: trailing,
            needs,
            spans: vec![Span {
                start: list.header_start,
                end: list.header_end,
                kind: SpanKind::NotebookRoute,
                project_note: None,
                title: Some(info.title.clone()),
                key: info.owner.clone(),
            }],
            diagnostics: Vec::new(),
            sub_bullets,
        });
    }
    if item_is_claimed_operator(item) {
        return None;
    }
    let claim = match classify_item(
        item,
        options.notebook_policy,
        options.global_route,
    ) {
        Ok(Some(claim)) => claim,
        Ok(None) => return None,
        Err(RouteScan::Unsupported {
            message,
            range,
            code,
            route,
        }) => {
            let owner = options
                .notebook_policy
                .and_then(|policy| policy.get(&route))
                .and_then(|info| info.owner.clone());
            let (code, message) =
                reformat_unsupported(code, message, &route, owner.as_deref());
            return Some(notebook_editor_error(
                route,
                code,
                message,
                Some(range),
            ));
        }
        Err(RouteScan::Supported(_)) | Err(RouteScan::NotNotebookRoute) => {
            return None;
        }
    };
    let mut needs = Vec::new();
    let mut spans = vec![Span {
        start: claim.marker.route_range.0,
        end: claim.marker.route_range.1,
        kind: SpanKind::NotebookRoute,
        project_note: None,
        title: Some(claim.info.title.clone()),
        key: claim.info.owner.clone(),
    }];
    let (mode, section, block_id) = match &claim.marker.form {
        NotebookForm::Note {
            section,
            exact_section,
            block_id,
            section_range,
            block_id_range,
        } => {
            if let Some(range) = section_range {
                spans.push(Span {
                    start: range.0,
                    end: range.1,
                    kind: SpanKind::NotebookSection,
                    project_note: None,
                    title: section.clone(),
                    key: Some(
                        if *exact_section { "exact" } else { "slug" }
                            .to_string(),
                    ),
                });
            }
            if let Some(range) = block_id_range {
                spans.push(Span {
                    start: range.0,
                    end: range.1,
                    kind: SpanKind::NotebookBlockId,
                    project_note: None,
                    title: None,
                    key: None,
                });
            }
            if section_range.is_some()
                && section.as_ref().is_none_or(|section| section.is_empty())
            {
                needs.push(Need::Section);
            }
            if block_id_range.is_some() && block_id.is_none() {
                needs.push(Need::BlockId);
            }
            (EditorMode::NotebookNote, section.clone(), block_id.clone())
        }
        NotebookForm::SubNote {
            parent_block_id,
            block_id_range,
        } => {
            if let Some(range) = block_id_range {
                spans.push(Span {
                    start: range.0,
                    end: range.1,
                    kind: SpanKind::NotebookParentBlockId,
                    project_note: None,
                    title: None,
                    key: None,
                });
            }
            if parent_block_id.is_empty() {
                needs.push(Need::BlockId);
            }
            (
                EditorMode::NotebookSubNote,
                None,
                Some(parent_block_id.clone()),
            )
        }
    };
    let sub_bullets = notebook_sub_bullets(item).unwrap_or_default();
    let clip = match refuse_notebook_item_markers(
        item,
        &claim.marker.route,
        owner_of(&claim.info),
        claim.marker.token_range,
    ) {
        Ok(clip) => clip,
        Err(error) => {
            return Some(notebook_editor_error(
                claim.marker.route,
                error.code.unwrap_or("notebook_marker_unsupported"),
                error.message,
                error.span.or(Some(claim.marker.token_range)),
            ));
        }
    };
    let mut body = claim.body;
    if clip.is_some() {
        body = strip_clip_tokens(&body);
    }
    if body.trim().is_empty()
        && needs.is_empty()
        && sub_bullets.is_empty()
        && clip.is_none()
        && !matches!(claim.marker.form, NotebookForm::SubNote { .. })
    {
        needs.push(Need::NotebookNote);
    }
    spans.sort_by_key(|span| (span.start, span.end));
    Some(NotebookEditorView {
        mode,
        body,
        route: claim.marker.route,
        section,
        block_id,
        needs,
        spans,
        diagnostics: Vec::new(),
        sub_bullets,
    })
}

fn notebook_editor_error(
    route: String,
    code: &'static str,
    message: String,
    range: Option<(usize, usize)>,
) -> NotebookEditorView {
    NotebookEditorView {
        mode: EditorMode::NotebookNote,
        body: String::new(),
        route,
        section: None,
        block_id: None,
        needs: Vec::new(),
        spans: Vec::new(),
        diagnostics: vec![Diagnostic {
            severity: Severity::Error,
            code,
            message,
            range,
            rewrite: None,
        }],
        sub_bullets: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native::capture_language::{
        model::{CaptureParseOptions, EditorParseOptions},
        parse_capture_text_with_options,
    };

    fn policy() -> NotebookRoutePolicy {
        let mut policy = NotebookRoutePolicy::default();
        policy.insert(
            "dev".into(),
            NotebookRouteInfo {
                title: "Developer tools".into(),
                summary: Some("Tools".into()),
                owner: Some("dev".into()),
                breadcrumb: vec!["dev".into(), "Developer tools".into()],
            },
        );
        policy
    }

    #[test]
    fn candidates_are_deduplicated_and_include_global_routes() {
        assert_eq!(
            candidate_notebook_routes(
                "Try @dev_tools#git\n@dev_tools^x\n@@ideas"
            ),
            vec!["dev_tools", "ideas"]
        );
        assert_eq!(
            candidate_notebook_routes("Note {0} %t\n\n@dev hello\n"),
            vec!["dev"]
        );
        assert!(candidate_notebook_routes("Unrouted note text").is_empty());
    }

    #[test]
    fn classification_keeps_body_details_opaque_and_emits_spans() {
        let mut policy = policy();
        let info = policy.routes.remove("dev").unwrap();
        policy.insert("dev_tools".into(), info);
        let options = EditorParseOptions {
            url_routing: None,
            has_global_destination: false,
            record_policy: None,
            notebook_policy: Some(&policy),
            global_route: None,
        };
        let parsed = super::super::parse_for_editor_with(
            "@dev_tools#git Run `#task`\n  - keep this detail verbatim",
            &options,
        );
        assert_eq!(parsed.items[0].mode, EditorMode::NotebookNote);
        assert_eq!(parsed.items[0].route.as_deref(), Some("dev_tools"));
        assert_eq!(parsed.items[0].section.as_deref(), Some("git"));
        assert_eq!(parsed.items[0].body, "Run `#task`");
        assert_eq!(
            parsed.items[0]
                .sub_bullets
                .iter()
                .map(|bullet| bullet.body.as_str())
                .collect::<Vec<_>>(),
            vec!["keep this detail verbatim"]
        );
        assert!(parsed
            .spans
            .iter()
            .any(|span| span.kind == SpanKind::NotebookRoute));
        assert!(parsed
            .spans
            .iter()
            .any(|span| span.kind == SpanKind::NotebookSection));
    }

    #[test]
    fn unsupported_suffix_is_a_fail_closed_editor_diagnostic() {
        let mut policy = NotebookRoutePolicy::default();
        policy.insert(
            "dev".into(),
            NotebookRouteInfo {
                title: "Dev".into(),
                summary: None,
                owner: Some("dev".into()),
                breadcrumb: vec!["dev".into()],
            },
        );
        let options = EditorParseOptions {
            url_routing: None,
            has_global_destination: false,
            record_policy: None,
            notebook_policy: Some(&policy),
            global_route: None,
        };
        let parsed =
            super::super::parse_for_editor_with("X @dev^x#tail", &options);
        assert_eq!(parsed.items[0].mode, EditorMode::NotebookNote);
        assert_eq!(
            parsed.items[0].diagnostics[0].code,
            "notebook_marker_unsupported"
        );
        let _ = CaptureParseOptions::routing_off();
    }

    #[test]
    fn route_scan_recognizes_the_four_individual_forms() {
        for raw in [
            "X @dev",
            "@dev X",
            "X @dev#git",
            "X @dev^note",
            "X @dev+note",
        ] {
            let parsed =
                edge_marker(&super::super::split_capture_draft(raw).items[0])
                    .unwrap();
            assert!(matches!(parsed, RouteScan::Supported(_)), "{raw}");
        }
    }

    #[test]
    fn strict_parser_claims_individual_notebook_note_forms() {
        let policy = policy();
        let options = CaptureParseOptions {
            notebook_policy: Some(&policy),
            ..CaptureParseOptions::routing_off()
        };
        let examples = [
            (
                "A note @dev",
                CaptureKind::NotebookNote {
                    section: None,
                    exact_section: false,
                    block_id: None,
                },
                "A note",
            ),
            (
                "@dev#Ideas Keep `#task` as prose",
                CaptureKind::NotebookNote {
                    section: Some("Ideas".into()),
                    exact_section: false,
                    block_id: None,
                },
                "Keep `#task` as prose",
            ),
            (
                "An authored note @dev^idea-id",
                CaptureKind::NotebookNote {
                    section: None,
                    exact_section: false,
                    block_id: Some("idea-id".into()),
                },
                "An authored note",
            ),
            (
                "Child note @dev+parent-id",
                CaptureKind::NotebookSubNote {
                    parent_block_id: "parent-id".into(),
                },
                "Child note",
            ),
        ];
        for (raw, expected_kind, expected_body) in examples {
            let parsed =
                parse_capture_text_with_options(raw, None, None, &options)
                    .unwrap();
            assert_eq!(parsed.kind, expected_kind, "{raw}");
            assert_eq!(parsed.body, expected_body, "{raw}");
            assert_eq!(parsed.route.as_deref(), Some("dev"), "{raw}");
        }
    }

    #[test]
    fn editor_spans_use_utf8_byte_offsets_for_trailing_markers() {
        let policy = policy();
        let options = EditorParseOptions {
            notebook_policy: Some(&policy),
            ..EditorParseOptions::routing_off()
        };
        let raw = "é note @dev+parent-id";
        let parsed = super::super::parse_for_editor_with(raw, &options);
        let route = parsed
            .spans
            .iter()
            .find(|span| span.kind == SpanKind::NotebookRoute)
            .unwrap();
        let parent = parsed
            .spans
            .iter()
            .find(|span| span.kind == SpanKind::NotebookParentBlockId)
            .unwrap();
        assert_eq!(&raw[route.start..route.end], "@dev");
        assert_eq!(&raw[parent.start..parent.end], "parent-id");
        assert!(parent.start > route.end);
    }
}
