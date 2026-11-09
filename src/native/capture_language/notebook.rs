//! Lexical notebook-route classification shared by capture and its editor.

use std::collections::HashMap;

use super::{
    editor_model::{Diagnostic, EditorMode, Need, Severity, Span, SpanKind},
    item::{CaptureItemParseError, ParsedCaptureItemOutcome},
    model::{
        CaptureItem, CaptureKind, CaptureParseOptions, EditorParseOptions,
        ParsedCaptureText,
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
}

impl NotebookRoutePolicy {
    pub(crate) fn absent() -> Self {
        Self::default()
    }

    pub(crate) fn insert(&mut self, route: String, info: NotebookRouteInfo) {
        self.routes.insert(route.to_ascii_lowercase(), info);
    }

    pub(crate) fn get(&self, route: &str) -> Option<&NotebookRouteInfo> {
        self.routes.get(&route.to_ascii_lowercase())
    }
}

/// Routes mentioned by capture syntax, in first-seen order. This is a
/// deliberately cheap lexical preflight: native callers use it to avoid
/// opening any note for an unrouted draft, and deduplicate repeated routes
/// before resolving notebook frontmatter.
pub(crate) fn candidate_notebook_routes(raw_text: &str) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    let mut routes = Vec::new();
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
    let unsupported = |message: &str| RouteScan::Unsupported {
        route: route.clone(),
        range: (start, end),
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
    if suffix.starts_with('/') || suffix.starts_with(':') {
        // Record routes and task/Pomodoro operation markers keep their
        // established grammar even when their leading route names a notebook.
        return RouteScan::NotNotebookRoute;
    }
    if suffix.starts_with('#') {
        let rest = &suffix[1..];
        let (section, exact, section_end) =
            if let Some(after_quote) = rest.strip_prefix('"') {
                let mut escaped = false;
                let mut close = None;
                for (offset, ch) in after_quote.char_indices() {
                    if escaped {
                        escaped = false;
                    } else if ch == '\\' {
                        escaped = true;
                    } else if ch == '"' {
                        close = Some(offset);
                        break;
                    }
                }
                match close {
                    Some(offset) => {
                        let raw = &after_quote[..offset];
                        let decoded =
                            raw.replace("\\\"", "\"").replace("\\\\", "\\");
                        (decoded, true, 1 + offset + 2)
                    }
                    None => (after_quote.to_string(), true, suffix.len()),
                }
            } else {
                let section = rest
                    .chars()
                    .take_while(|ch| !ch.is_whitespace())
                    .collect::<String>();
                (section.clone(), false, 1 + section.len())
            };
        let section_end_abs = route_end + section_end;
        if section_end_abs < end {
            return unsupported(
                "notebook route markers must be at an item edge",
            );
        }
        if section.contains('^') || section.contains('+') {
            return unsupported("combine notebook sections and block IDs only after the notebook capture grammar is complete");
        }
        return RouteScan::Supported(RouteMarker {
            route,
            route_range: (start, route_end),
            token_range: (start, end),
            form: NotebookForm::Note {
                section: Some(section),
                exact_section: exact,
                block_id: None,
                section_range: Some((route_end, section_end_abs)),
                block_id_range: None,
            },
        });
    }
    if let Some(block_id) = suffix.strip_prefix('^') {
        if block_id.ends_with('@') {
            // A sealed `@parent^id@` is a project-note/notebook-creation
            // marker, owned by its own phase.
            return RouteScan::NotNotebookRoute;
        }
        if block_id.contains(['#', '+', ':', '=', '!']) {
            return unsupported("unsupported suffix on a notebook note route");
        }
        let id_range = (route_end + 1, end);
        if !block_id.is_empty() && !super::tokens::is_block_id(block_id) {
            return unsupported("notebook note block IDs must contain only A-Z, a-z, 0-9, or `-`");
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
        if parent_block_id.ends_with('!') {
            // Existing explicit task toggles remain on their established path.
            return RouteScan::NotNotebookRoute;
        }
        if parent_block_id.contains(['#', '^', ':', '=', '!']) {
            return unsupported(
                "unsupported suffix on a notebook sub-note route",
            );
        }
        if !parent_block_id.is_empty()
            && !super::tokens::is_block_id(parent_block_id)
        {
            return unsupported("notebook parent block IDs must contain only A-Z, a-z, 0-9, or `-`");
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
    unsupported("this route form is not supported for notebook notes")
}

fn quoted_section_marker_len(suffix: &str) -> Option<usize> {
    let after_quote = suffix.strip_prefix("#\"")?;
    let mut escaped = false;
    for (offset, ch) in after_quote.char_indices() {
        if escaped {
            escaped = false;
        } else if ch == '\\' {
            escaped = true;
        } else if ch == '"' {
            return Some(2 + offset + ch.len_utf8());
        }
    }
    None
}

fn edge_marker(item: &CaptureItem<'_>) -> Option<RouteScan> {
    if item.list.is_some() {
        // List destinations belong to the next grammar phase. The planner
        // still refuses a parsed list whose resolved route is a notebook.
        return None;
    }
    let parent = item.lines.first()?.raw;
    let text = parent.text;
    let leading = text.len() - text.trim_start().len();
    if text[leading..].starts_with('@') {
        // Keep quoted section titles with spaces inside their marker.
        let (_, route_end) = route_prefix(text, leading)?;
        let suffix = &text[route_end..];
        let marker_end =
            if suffix.starts_with('#') && suffix[1..].starts_with('"') {
                route_end
                    + quoted_section_marker_len(suffix).unwrap_or(suffix.len())
            } else {
                text[leading..]
                    .find(char::is_whitespace)
                    .map(|offset| leading + offset)
                    .unwrap_or(text.len())
            };
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
        let Some((_, route_end)) = route_prefix(candidate, 0) else {
            continue;
        };
        let suffix = &candidate[route_end..];
        let consumed =
            if suffix.starts_with('#') && suffix[1..].starts_with('"') {
                route_end
                    + quoted_section_marker_len(suffix).unwrap_or(suffix.len())
            } else {
                candidate.len()
            };
        if consumed != candidate.len() {
            continue;
        }
        return Some(shift_scan(
            scan_route_marker(text, relative, trimmed_end),
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
            message,
        } => RouteScan::Unsupported {
            route,
            range: shift(range),
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
    let mut parts = Vec::new();
    if !parent_body.is_empty() {
        parts.push(parent_body.to_string());
    }
    parts.extend(
        item.lines
            .iter()
            .skip(1)
            .map(|line| line.raw.text.to_string()),
    );
    parts.join("\n")
}

#[derive(Debug, Clone)]
struct NotebookClaim {
    marker: RouteMarker,
    body: String,
    info: NotebookRouteInfo,
}

fn classify_item(
    item: &CaptureItem<'_>,
    policy: Option<&NotebookRoutePolicy>,
) -> Result<Option<NotebookClaim>, RouteScan> {
    let Some(policy) = policy else {
        return Ok(None);
    };
    let Some(scan) = edge_marker(item) else {
        return Ok(None);
    };
    let route = match &scan {
        RouteScan::NotNotebookRoute => return Ok(None),
        RouteScan::Supported(marker) => marker.route.as_str(),
        RouteScan::Unsupported { route, .. } => route,
    };
    let Some(info) = policy.get(route).cloned() else {
        return Ok(None);
    };
    match scan {
        RouteScan::Supported(marker) => Ok(Some(NotebookClaim {
            body: body_without_marker(item, &marker),
            marker,
            info,
        })),
        refusal @ RouteScan::Unsupported { .. } => Err(refusal),
        RouteScan::NotNotebookRoute => Ok(None),
    }
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

pub(super) fn claim_strict_notebook<'a>(
    item: &CaptureItem<'a>,
    forced_route: Option<&str>,
    forced_section: Option<&str>,
    options: &CaptureParseOptions<'_>,
) -> Result<Option<ParsedCaptureItemOutcome<'a>>, CaptureItemParseError> {
    if forced_route.is_some()
        || forced_section.is_some()
        || options.explicit_destination
    {
        return Ok(None);
    }
    let claim = match classify_item(item, options.notebook_policy) {
        Ok(claim) => claim,
        Err(RouteScan::Unsupported { message, range, .. }) => {
            return Err(CaptureItemParseError::coded_at(
                "notebook_route_unsupported",
                message,
                Some(range),
            ));
        }
        Err(RouteScan::Supported(_)) | Err(RouteScan::NotNotebookRoute) => None,
    };
    let Some(claim) = claim else {
        return Ok(None);
    };
    if claim.body.trim().is_empty() {
        return Err(CaptureItemParseError::coded_at(
            "notebook_note_required",
            "a notebook route needs note text; add the note after the route"
                .to_string(),
            Some(claim.marker.token_range),
        ));
    }
    match &claim.marker.form {
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
    let route = claim.marker.route.clone();
    let kind = notebook_kind(&claim);
    Ok(Some(super::item::parsed_capture_item_outcome(
        item,
        ParsedCaptureText {
            body: claim.body,
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
            merge_in: None,
        },
        Vec::new(),
        Some(
            item.lines
                .first()
                .map_or(String::new(), |line| line.raw.text.trim().to_string()),
        ),
    )))
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
}

pub(crate) fn editor_view_for_item(
    item: &CaptureItem<'_>,
    options: &EditorParseOptions<'_>,
) -> Option<NotebookEditorView> {
    let claim = match classify_item(item, options.notebook_policy) {
        Ok(Some(claim)) => claim,
        Ok(None) => return None,
        Err(RouteScan::Unsupported { message, range, .. }) => {
            return Some(NotebookEditorView {
                mode: EditorMode::NotebookNote,
                body: String::new(),
                route: String::new(),
                section: None,
                block_id: None,
                needs: Vec::new(),
                spans: Vec::new(),
                diagnostics: vec![Diagnostic {
                    severity: Severity::Error,
                    code: "notebook_route_unsupported",
                    message,
                    range: Some(range),
                    rewrite: None,
                }],
            });
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
            if section.as_deref() == Some("") {
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
    if claim.body.trim().is_empty() && needs.is_empty() {
        needs.push(Need::NotebookNote);
    }
    spans.sort_by_key(|span| (span.start, span.end));
    Some(NotebookEditorView {
        mode,
        body: claim.body,
        route: claim.marker.route,
        section,
        block_id,
        needs,
        spans,
        diagnostics: Vec::new(),
    })
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
        };
        let parsed = super::super::parse_for_editor_with(
            "@dev_tools#git Run `#task`\n  - keep this detail verbatim",
            &options,
        );
        assert_eq!(parsed.items[0].mode, EditorMode::NotebookNote);
        assert_eq!(parsed.items[0].route.as_deref(), Some("dev_tools"));
        assert_eq!(parsed.items[0].section.as_deref(), Some("git"));
        assert_eq!(
            parsed.items[0].body,
            "Run `#task`\n  - keep this detail verbatim"
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
        };
        let parsed =
            super::super::parse_for_editor_with("X @dev^x#tail", &options);
        assert_eq!(parsed.items[0].mode, EditorMode::NotebookNote);
        assert_eq!(
            parsed.items[0].diagnostics[0].code,
            "notebook_route_unsupported"
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
                "@dev#Ideas Keep #task as prose",
                CaptureKind::NotebookNote {
                    section: Some("Ideas".into()),
                    exact_section: false,
                    block_id: None,
                },
                "Keep #task as prose",
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
