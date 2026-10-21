//! Draft-mode contract for `bob capture-related`.

use std::{
    collections::{BTreeSet, HashMap, HashSet},
    path::{Path, PathBuf},
};

use super::{
    bands, corpus, score, tokenize, ClosedJson, DraftRequest, LinkedMatch,
    RelatedItem, RelatedMatch, Replacements, SourceRange, SCHEMA_VERSION,
};
use crate::native::{
    capture_dependency_tasks,
    capture_language::{
        self, EditorItemParse, EditorMode, EditorParseOptions, SpanKind,
    },
    capture_parse,
    capture_related::corpus::Candidate,
    capture_related::tokenize::Token,
    task_fields,
    vault_links::NoteIndex,
};

pub(super) fn draft_related(
    request: &DraftRequest,
) -> super::CaptureRelatedResult {
    // Template definitions are capture-editor syntax, not task-creating
    // drafts. In particular, do this before resolving BOB_DIR or walking it.
    if capture_language::template::analyze(&request.draft)
        .header
        .is_some()
    {
        return empty_result();
    }

    let policy = capture_parse::load_parse_routing(false);
    let options = EditorParseOptions {
        url_routing: policy.as_ref(),
        has_global_destination: false,
    };
    let parsed =
        capture_language::parse_for_editor_with(&request.draft, &options);
    let eligible = parsed
        .items
        .iter()
        .filter(|item| is_task_creating_mode(item.mode))
        .collect::<Vec<_>>();
    if eligible.is_empty() {
        return empty_result();
    }

    let discovery = corpus::discover(&request.bob_dir, request.days);
    let candidate_paths = discovery
        .candidates
        .iter()
        .map(|candidate| PathBuf::from(&candidate.note_path))
        .collect::<Vec<_>>();
    let link_index = NoteIndex::from_paths(candidate_paths);

    let per_item = eligible
        .iter()
        .map(|item| {
            let title = tokenize::display_title(
                &item.body,
                &discovery.global_filter,
                None,
            );
            let tokens = tokenize::tokenize(&title);
            let scores = score::cosine_scores(&tokens, &discovery.candidates);
            ItemScoring {
                item,
                title,
                tokens,
                scores,
            }
        })
        .collect::<Vec<_>>();

    let linked = per_item
        .iter()
        .map(|scoring| {
            linked_for_item(
                scoring.item,
                parsed.global_destination.as_ref(),
                &discovery.candidates,
                &link_index,
                &discovery.route_aliases,
            )
        })
        .collect::<Vec<_>>();
    let globally_linked = linked
        .iter()
        .flat_map(|entries| entries.iter().map(|entry| entry.0))
        .collect::<HashSet<_>>();

    // A candidate belongs to the item with its highest raw score. Ties keep
    // the earliest eligible item, preserving the capture draft's order.
    let mut owner: HashMap<usize, (f64, usize)> = HashMap::new();
    for (item_position, scoring) in per_item.iter().enumerate() {
        for (candidate_index, value) in
            scoring.scores.iter().copied().enumerate()
        {
            if globally_linked.contains(&candidate_index) {
                continue;
            }
            let replace = owner.get(&candidate_index).is_none_or(
                |(best, best_position)| {
                    value > *best
                        || (value == *best && item_position < *best_position)
                },
            );
            if replace {
                owner.insert(candidate_index, (value, item_position));
            }
        }
    }

    let result_items = per_item
        .iter()
        .enumerate()
        .map(|(item_position, scoring)| {
            let item_candidates = discovery
                .candidates
                .iter()
                .enumerate()
                .filter(|(candidate_index, _)| {
                    !globally_linked.contains(candidate_index)
                        && owner.get(candidate_index).is_some_and(
                            |(_, selected)| *selected == item_position,
                        )
                })
                .map(|(candidate_index, candidate)| {
                    (candidate, scoring.scores[candidate_index])
                })
                .collect::<Vec<_>>();
            let candidates = item_candidates
                .iter()
                .map(|(candidate, _)| (*candidate).clone())
                .collect::<Vec<_>>();
            let scores = item_candidates
                .iter()
                .map(|(_, value)| *value)
                .collect::<Vec<_>>();
            let ranked = bands::rank(
                &scoring.title,
                &scoring.tokens,
                &candidates,
                &scores,
                scoring.item.route.as_deref(),
                request.limit,
            );
            let range = SourceRange {
                start: scoring.item.start,
                end: scoring.item.end,
            };
            let marker_at = marker_at(&request.draft, scoring.item);
            let route_range = route_range(&request.draft, scoring.item);
            let discard_range =
                discard_range(&request.draft, scoring.item, &parsed.items);
            let matches = ranked
                .into_iter()
                .map(|ranked| {
                    draft_match(
                        ranked,
                        scoring.item,
                        &request.draft,
                        marker_at,
                        route_range,
                        discard_range,
                        &options,
                        &link_index,
                    )
                })
                .collect();
            let linked = linked[item_position]
                .iter()
                .map(|(candidate_index, link)| LinkedMatch {
                    key: discovery.candidates[*candidate_index].key.clone(),
                    link: link.clone(),
                })
                .collect();
            RelatedItem {
                index: scoring.item.index,
                mode: scoring.item.mode.label().to_string(),
                range: Some(range),
                marker_at: Some(marker_at),
                route_range,
                discard_range,
                matches,
                linked: Some(linked),
            }
        })
        .collect();

    super::CaptureRelatedResult {
        ok: true,
        schema_version: SCHEMA_VERSION,
        warnings: discovery.warnings,
        items: result_items,
    }
}

struct ItemScoring<'a> {
    item: &'a EditorItemParse,
    title: String,
    tokens: Vec<Token>,
    scores: Vec<f64>,
}

fn empty_result() -> super::CaptureRelatedResult {
    super::CaptureRelatedResult {
        ok: true,
        schema_version: SCHEMA_VERSION,
        warnings: Vec::new(),
        items: Vec::new(),
    }
}

fn is_task_creating_mode(mode: EditorMode) -> bool {
    matches!(
        mode,
        EditorMode::Task
            | EditorMode::PomodoroTask
            | EditorMode::ProjectNote
            | EditorMode::PomodoroProjectNote
    )
}

fn marker_at(draft: &str, item: &EditorItemParse) -> usize {
    let Some((line_start, line_end)) =
        physical_line_range(draft, item.line_start)
    else {
        return item.end;
    };
    let suffix_start = item
        .spans
        .iter()
        .filter(|span| span.start >= line_start && span.end <= line_end)
        .filter(|span| {
            matches!(
                span.kind,
                SpanKind::Schedule
                    | SpanKind::Priority
                    | SpanKind::Clipboard
                    | SpanKind::PomodoroStart
                    | SpanKind::PomodoroStartDrop
                    | SpanKind::PomodoroClose
                    | SpanKind::PomodoroCloseInProgress
                    | SpanKind::PomodoroClosePark
                    | SpanKind::PomodoroCloseComplete
                    | SpanKind::PomodoroCloseDrop
            )
        })
        .map(|span| span.start)
        .min();
    let mut at = line_end;
    if let Some(start) = suffix_start.filter(|start| *start >= line_start) {
        at = at.min(start);
    }
    while at > line_start
        && draft
            .as_bytes()
            .get(at - 1)
            .is_some_and(u8::is_ascii_whitespace)
    {
        at -= 1;
    }
    at
}

fn route_range(draft: &str, item: &EditorItemParse) -> Option<SourceRange> {
    let (line_start, line_end) = physical_line_range(draft, item.line_start)?;
    item.local_destination_markers
        .iter()
        .find(|marker| marker.start >= line_start && marker.end <= line_end)
        .map(|marker| SourceRange {
            start: marker.start,
            end: marker.end,
        })
}

/// The editor parse's `line_start` is a 1-based physical line number; this
/// maps it back to UTF-8 byte offsets in the original draft.
fn physical_line_range(
    draft: &str,
    line_number: usize,
) -> Option<(usize, usize)> {
    let bytes = draft.as_bytes();
    let mut start = 0;
    let mut line = 1;
    let mut index = 0;
    while index < bytes.len() {
        if matches!(bytes[index], b'\n' | b'\r') {
            if line == line_number {
                return Some((start, index));
            }
            line += 1;
            index += 1;
            if bytes[index - 1] == b'\r' && bytes.get(index) == Some(&b'\n') {
                index += 1;
            }
            start = index;
        } else {
            index += 1;
        }
    }
    (line == line_number).then_some((start, draft.len()))
}

fn discard_range(
    draft: &str,
    item: &EditorItemParse,
    all_items: &[EditorItemParse],
) -> Option<SourceRange> {
    if !item.sub_bullets.is_empty() {
        return None;
    }
    let position = all_items
        .iter()
        .position(|candidate| candidate.index == item.index)?;
    if all_items.len() == 1 {
        return Some(SourceRange {
            start: 0,
            end: draft.len(),
        });
    }
    if let Some(next) = all_items.get(position + 1) {
        Some(SourceRange {
            start: item.start,
            end: next.start,
        })
    } else {
        let previous = all_items.get(position.checked_sub(1)?)?;
        Some(SourceRange {
            start: previous.end,
            end: item.end,
        })
    }
}

fn linked_for_item(
    item: &EditorItemParse,
    global: Option<&capture_language::EditorGlobalDestination>,
    candidates: &[Candidate],
    index: &NoteIndex,
    route_aliases: &[(String, String)],
) -> Vec<(usize, String)> {
    let mut targets = Vec::<(String, String, String)>::new();
    targets.extend(item.relations.iter().map(|entry| {
        (
            entry.note.clone(),
            entry.block_id.clone(),
            "relation".to_string(),
        )
    }));
    targets.extend(item.dependencies.iter().map(|entry| {
        (
            entry.note.clone(),
            entry.block_id.clone(),
            "dependency".to_string(),
        )
    }));
    targets.extend(item.blocked_tasks.iter().map(|entry| {
        (
            entry.note.clone(),
            entry.block_id.clone(),
            "blocked_task".to_string(),
        )
    }));
    for marker in &item.local_destination_markers {
        if let (Some(route), Some(block_id)) = (&marker.route, &marker.block_id)
        {
            targets.push((
                route.clone(),
                block_id.clone(),
                "parent".to_string(),
            ));
        }
    }
    if let Some(global) = global.filter(|global| global.block_id.is_some()) {
        if item.local_destination_markers.is_empty() {
            if let (Some(route), Some(block_id)) =
                (&global.route, &global.block_id)
            {
                targets.push((
                    route.clone(),
                    block_id.clone(),
                    "parent".to_string(),
                ));
            }
        }
    }

    let mut linked = BTreeSet::new();
    for (note, block_id, link) in targets {
        let note_path = route_aliases
            .iter()
            .find(|(alias, _)| alias.eq_ignore_ascii_case(&note))
            .map(|(_, path)| path.as_str());
        let resolved = note_path
            .and_then(|path| index.resolve(None, path))
            .or_else(|| index.resolve(None, &note));
        for (candidate_index, candidate) in candidates.iter().enumerate() {
            if candidate.block_id.as_deref() != Some(block_id.as_str()) {
                continue;
            }
            let route_match = candidate.route.eq_ignore_ascii_case(&note)
                || candidate.note_path.eq_ignore_ascii_case(&note)
                || Path::new(&candidate.note_path)
                    .file_stem()
                    .and_then(|stem| stem.to_str())
                    .is_some_and(|stem| stem.eq_ignore_ascii_case(&note));
            let path_match = resolved.as_ref().is_some_and(|path| {
                path.to_string_lossy()
                    .eq_ignore_ascii_case(&candidate.note_path)
            });
            let alias_match = note_path.is_some_and(|path| {
                path.eq_ignore_ascii_case(&candidate.note_path)
                    || candidate.route.eq_ignore_ascii_case(&note)
            });
            if route_match || path_match || alias_match {
                linked.insert((candidate_index, link.clone()));
            }
        }
    }
    linked.into_iter().collect()
}

fn draft_match(
    ranked: bands::RankedMatch<'_>,
    item: &EditorItemParse,
    draft: &str,
    marker_at: usize,
    route_range: Option<SourceRange>,
    discard_range: Option<SourceRange>,
    options: &EditorParseOptions<'_>,
    index: &NoteIndex,
) -> RelatedMatch {
    let candidate = ranked.candidate;
    let replacement_id = candidate
        .block_id
        .as_deref()
        .or_else(|| candidate.block_id_suggestions.first().map(String::as_str));
    let replacements =
        replacement_id.map(|id| replacements(candidate, id, index));
    let closed = candidate.closed.as_ref().map(|closed| ClosedJson {
        status: closed.status.as_str().to_string(),
        date: task_fields::format_calendar_date(closed.date),
        days_ago: closed.days_ago,
    });

    let mut actions = Vec::new();
    if candidate.open {
        if let Some(replacements) = replacements.as_ref()
            && verify_link_action(
                "relate",
                item,
                draft,
                marker_at,
                route_range,
                replacements,
                candidate,
                options,
            )
        {
            actions.push("relate".to_string());
        }
        if replacement_id.is_some_and(|id| {
            verify_note_action(
                item,
                draft,
                marker_at,
                route_range,
                candidate,
                id,
                options,
            )
        }) {
            actions.push("note".to_string());
        }
        if let Some(replacements) = replacements.as_ref()
            && verify_link_action(
                "needs",
                item,
                draft,
                marker_at,
                route_range,
                replacements,
                candidate,
                options,
            )
        {
            actions.push("needs".to_string());
        }
        if let Some(replacements) = replacements.as_ref()
            && verify_link_action(
                "blocks",
                item,
                draft,
                marker_at,
                route_range,
                replacements,
                candidate,
                options,
            )
        {
            actions.push("blocks".to_string());
        }
    }
    actions.push("open".to_string());
    if discard_range.is_some() {
        actions.push("discard".to_string());
    }
    let default_action =
        default_action(ranked.band, item, &candidate.title, &actions);
    RelatedMatch {
        key: candidate.key.clone(),
        band: ranked.band.as_str().to_string(),
        title: candidate.title.clone(),
        matched_ranges: ranked.matched_ranges,
        kind: candidate.kind_name().to_string(),
        status_symbol: candidate.status_symbol.to_string(),
        status_name: candidate.status_name.clone(),
        status_type: candidate.status_type.to_string(),
        closed,
        route: candidate.route.clone(),
        locator: candidate.locator.clone(),
        note_path: candidate.note_path.clone(),
        target: candidate.target.display().to_string(),
        line: candidate.line,
        task_ref: candidate.task_ref.clone(),
        block_id: candidate.block_id.clone(),
        requires_block_id: Some(candidate.block_id.is_none()),
        block_id_suggestions: Some(candidate.block_id_suggestions.clone()),
        replacements: candidate.block_id.as_ref().and(replacements),
        actions: Some(actions),
        default_action: Some(default_action),
        score: None,
    }
}

fn replacements(
    candidate: &Candidate,
    id: &str,
    index: &NoteIndex,
) -> Replacements {
    let path = Path::new(&candidate.note_path);
    Replacements {
        relation_to: capture_dependency_tasks::replacement_for_prefix(
            index, path, id, "~",
        ),
        parent: format!("@{}+{id}", candidate.route),
        dependency: capture_dependency_tasks::replacement_for(index, path, id),
        blocked_task: capture_dependency_tasks::replacement_for_sigil(
            index, path, id, b'*',
        ),
    }
}

fn verify_link_action(
    action: &str,
    item: &EditorItemParse,
    draft: &str,
    marker_at: usize,
    _route_range: Option<SourceRange>,
    replacements: &Replacements,
    _candidate: &Candidate,
    options: &EditorParseOptions<'_>,
) -> bool {
    let marker = match action {
        "relate" => &replacements.relation_to,
        "needs" => &replacements.dependency,
        "blocks" => &replacements.blocked_task,
        _ => return false,
    };
    let modified = insert_at(draft, marker_at, &format!(" {marker}"));
    let parsed = capture_language::parse_for_editor_with(&modified, options);
    parsed
        .items
        .iter()
        .find(|candidate| candidate.index == item.index)
        .is_some_and(|after| {
            after.mode == item.mode
                && !has_error_diagnostics(after)
                && match action {
                    "relate" => {
                        after.relations.len() == item.relations.len() + 1
                    }
                    "needs" => {
                        after.dependencies.len() == item.dependencies.len() + 1
                    }
                    "blocks" => {
                        after.blocked_tasks.len()
                            == item.blocked_tasks.len() + 1
                    }
                    _ => false,
                }
        })
}

fn verify_note_action(
    item: &EditorItemParse,
    draft: &str,
    marker_at: usize,
    route_range: Option<SourceRange>,
    candidate: &Candidate,
    id: &str,
    options: &EditorParseOptions<'_>,
) -> bool {
    let parent = format!("@{}+{id}", candidate.route);
    let modified = if let Some(range) = route_range {
        replace_range(draft, range, &parent)
    } else {
        insert_at(draft, marker_at, &format!(" {parent}"))
    };
    let parsed = capture_language::parse_for_editor_with(&modified, options);
    parsed
        .items
        .iter()
        .find(|candidate| candidate.index == item.index)
        .is_some_and(|after| {
            after.mode == EditorMode::SubBullet && !has_error_diagnostics(after)
        })
}

fn has_error_diagnostics(item: &EditorItemParse) -> bool {
    item.diagnostics.iter().any(|diagnostic| {
        diagnostic.severity == capture_language::Severity::Error
    })
}

fn insert_at(draft: &str, at: usize, insert: &str) -> String {
    let mut modified = String::with_capacity(draft.len() + insert.len());
    modified.push_str(&draft[..at]);
    modified.push_str(insert);
    modified.push_str(&draft[at..]);
    modified
}

fn replace_range(draft: &str, range: SourceRange, replacement: &str) -> String {
    let mut modified = String::with_capacity(
        draft.len() - (range.end - range.start) + replacement.len(),
    );
    modified.push_str(&draft[..range.start]);
    modified.push_str(replacement);
    modified.push_str(&draft[range.end..]);
    modified
}

fn default_action(
    band: bands::Band,
    item: &EditorItemParse,
    candidate_title: &str,
    actions: &[String],
) -> String {
    let same_title = tokenize::normalize_title(&item.body)
        == tokenize::normalize_title(candidate_title);
    let preferred = match band {
        bands::Band::SameTitle => "discard",
        bands::Band::PossibleDuplicate => "note",
        bands::Band::ClosedDuplicate if same_title => "discard",
        bands::Band::ClosedDuplicate => "open",
        bands::Band::Similar | bands::Band::Hold => "relate",
    };
    if actions.iter().any(|action| action == preferred) {
        preferred.to_string()
    } else if actions.iter().any(|action| action == "note") {
        "note".to_string()
    } else if actions.iter().any(|action| action == "relate") {
        "relate".to_string()
    } else if actions.iter().any(|action| action == "open") {
        "open".to_string()
    } else {
        "discard".to_string()
    }
}
