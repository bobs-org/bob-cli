//! Same-folder near-title matches for record create items.

use std::path::PathBuf;

use super::{
    bands, score, tokenize, DraftRequest, LinkedMatch, RecordAddTo,
    RecordFieldDisplay, RelatedItem, RelatedMatch, SourceRange,
};
use crate::native::{
    capture_language::{
        self, record::RecordAction, EditorItemParse, EditorParseOptions,
        RecordNoteIntent,
    },
    capture_record::{create_to_update_rewrite, record_policy_for_draft},
    records::{self, FieldKind, IndexedRecord, RecordCatalog, RecordType},
};

/// Build `RelatedItem`s for record create items. Update items are skipped
/// entirely (no rows on update items). Create items always emit an entry,
/// even when nothing in the folder clears the bars.
pub(super) fn record_items(
    request: &DraftRequest,
    parsed: &capture_language::EditorParse,
    options: &EditorParseOptions<'_>,
) -> Vec<RelatedItem> {
    let intents: Vec<&RecordNoteIntent> = parsed
        .record_intents
        .iter()
        .map(|(_, intent)| intent)
        .collect();
    if intents.is_empty() {
        return Vec::new();
    }
    let policy = record_policy_for_draft(&request.bob_dir, &request.draft);
    let mut catalog = RecordCatalog::new(&request.bob_dir);
    let mut out = Vec::new();
    for item in parsed.items.iter().filter(|item| {
        item.mode == crate::native::capture_language::EditorMode::RecordNote
    }) {
        let Some(intent) =
            parsed.record_intents.iter().find_map(|(index, intent)| {
                (*index == item.index).then_some(intent)
            })
        else {
            continue;
        };
        if intent.action != RecordAction::Create {
            continue;
        }
        // An empty title cannot clear the two-term bar; still emit the
        // item so clients see an explicit empty match list.
        if intent.title.trim().is_empty() {
            out.push(empty_record_item(item, &request.draft, parsed));
            continue;
        }
        let Some(record_type) =
            policy.usable_record_type_for_route(&intent.dir).cloned()
        else {
            out.push(empty_record_item(item, &request.draft, parsed));
            continue;
        };
        let notes = catalog.folder(&intent.dir).notes().to_vec();
        if notes.is_empty() {
            out.push(empty_record_item(item, &request.draft, parsed));
            continue;
        }
        let planned_stem = planned_create_stem(intent);
        let kept: Vec<IndexedRecord> = notes
            .into_iter()
            .filter(|note| !is_excluded_stem(note, planned_stem.as_deref()))
            .collect();
        if kept.is_empty() {
            out.push(empty_record_item(item, &request.draft, parsed));
            continue;
        }
        let candidates = to_candidates(&kept, &record_type, &request.bob_dir);
        let query_title = intent.title.clone();
        let query_tokens = tokenize::tokenize(&query_title);
        let scores = score::cosine_scores(&query_tokens, &candidates);
        let ranked = bands::rank(
            &query_title,
            &query_tokens,
            &candidates,
            &scores,
            Some(record_type.dir.as_str()),
            request.limit,
        );
        let range = SourceRange {
            start: item.start,
            end: item.end,
        };
        let marker = super::draft::marker_at(&request.draft, item);
        let discard =
            super::draft::discard_range(&request.draft, item, &parsed.items);
        let matches = ranked
            .into_iter()
            .filter_map(|ranked| {
                let note = kept.iter().find(|note| {
                    note.relative_path.to_string_lossy() == ranked.candidate.key
                })?;
                Some(record_match(
                    ranked,
                    note,
                    &record_type,
                    intent,
                    &request.draft,
                    item,
                    options,
                ))
            })
            .collect::<Vec<_>>();
        out.push(RelatedItem {
            index: item.index,
            mode: item.mode.label().to_string(),
            range: Some(range),
            marker_at: Some(marker),
            route_range: None,
            discard_range: discard,
            matches,
            linked: Some(Vec::new()),
        });
    }
    out
}

fn empty_record_item(
    item: &EditorItemParse,
    draft: &str,
    parsed: &capture_language::EditorParse,
) -> RelatedItem {
    RelatedItem {
        index: item.index,
        mode: item.mode.label().to_string(),
        range: Some(SourceRange {
            start: item.start,
            end: item.end,
        }),
        marker_at: Some(super::draft::marker_at(draft, item)),
        route_range: None,
        discard_range: super::draft::discard_range(draft, item, &parsed.items),
        matches: Vec::new(),
        linked: Some(Vec::new()),
    }
}

fn planned_create_stem(intent: &RecordNoteIntent) -> Option<String> {
    if let Some(explicit) = intent.explicit_stem.as_deref() {
        return Some(explicit.to_owned());
    }
    records::stem::normalize_stem(&intent.title)
}

fn is_excluded_stem(note: &IndexedRecord, planned: Option<&str>) -> bool {
    let Some(planned) = planned else {
        return false;
    };
    note.stem.to_lowercase() == planned.to_lowercase()
}

fn to_candidates(
    notes: &[IndexedRecord],
    record_type: &RecordType,
    bob_dir: &std::path::Path,
) -> Vec<super::corpus::Candidate> {
    notes
        .iter()
        .map(|note| {
            let display = note.display.clone();
            super::corpus::Candidate {
                key: note.relative_path.to_string_lossy().into_owned(),
                tokens: tokenize::tokenize(&display),
                title: display,
                kind: super::corpus::CandidateKind::Task,
                status_symbol: ' ',
                status_name: String::new(),
                status_type: "record",
                closed: None,
                route: record_type.dir.clone(),
                locator: record_type.dir.clone(),
                note_path: note.relative_path.to_string_lossy().into_owned(),
                target: bob_dir.join(&note.relative_path),
                line: 1,
                task_ref: String::new(),
                block_id: None,
                block_id_suggestions: Vec::new(),
                created: None,
                open: true,
            }
        })
        .collect()
}

fn record_match(
    ranked: bands::RankedMatch<'_>,
    note: &IndexedRecord,
    record_type: &RecordType,
    intent: &RecordNoteIntent,
    draft: &str,
    item: &EditorItemParse,
    options: &EditorParseOptions<'_>,
) -> RelatedMatch {
    let status = record_type
        .field("status")
        .and_then(|field| indexed_field_display(field, note));
    let fields = record_type
        .fields
        .iter()
        .filter(|field| field.suggest)
        .take(2)
        .filter_map(|field| {
            indexed_field_display(field, note).map(|value| RecordFieldDisplay {
                key: field.key.clone(),
                label: field.label.clone(),
                value,
            })
        })
        .collect::<Vec<_>>();
    let add_to =
        create_to_update_rewrite(draft, intent, &record_type.dir, &note.stem)
            .filter(|rewrite| verify_add_to(draft, item, rewrite, options))
            .map(|rewrite| RecordAddTo {
                start: rewrite.start,
                end: rewrite.end,
                replacement: rewrite.replacement,
            });
    let mut actions = Vec::new();
    if add_to.is_some() {
        actions.push("add_to".to_string());
    }
    actions.push("open".to_string());
    // Discard is always available when the item can be removed; the
    // item-level discard_range owns that anchor.
    actions.push("discard".to_string());
    let default_action = match ranked.band {
        bands::Band::PossibleDuplicate if add_to.is_some() => "add_to",
        bands::Band::SameTitle => "discard",
        _ => "open",
    }
    .to_string();
    let key = note.relative_path.to_string_lossy().into_owned();
    RelatedMatch {
        key: key.clone(),
        band: ranked.band.as_str().to_string(),
        title: note.display.clone(),
        matched_ranges: ranked.matched_ranges,
        kind: "record".to_string(),
        status_symbol: " ".to_string(),
        status_name: String::new(),
        status_type: "record".to_string(),
        closed: None,
        route: record_type.dir.clone(),
        locator: record_type.dir.clone(),
        note_path: key,
        target: PathBuf::from(&ranked.candidate.target)
            .display()
            .to_string(),
        line: 1,
        task_ref: String::new(),
        block_id: None,
        requires_block_id: None,
        block_id_suggestions: None,
        replacements: None,
        actions: Some(actions),
        default_action: Some(default_action),
        score: None,
        scope: Some("record".to_string()),
        record_type: Some(record_type.id.clone()),
        stem: Some(note.stem.clone()),
        status,
        fields: Some(fields),
        add_to,
    }
}

fn verify_add_to(
    draft: &str,
    item: &EditorItemParse,
    rewrite: &crate::native::capture_language::DiagnosticRewrite,
    options: &EditorParseOptions<'_>,
) -> bool {
    if rewrite.start > rewrite.end
        || rewrite.end > draft.len()
        || !draft.is_char_boundary(rewrite.start)
        || !draft.is_char_boundary(rewrite.end)
    {
        return false;
    }
    let mut modified = String::with_capacity(
        draft.len() - (rewrite.end - rewrite.start) + rewrite.replacement.len(),
    );
    modified.push_str(&draft[..rewrite.start]);
    modified.push_str(&rewrite.replacement);
    modified.push_str(&draft[rewrite.end..]);
    let parsed = capture_language::parse_for_editor_with(&modified, options);
    parsed
        .items
        .iter()
        .find(|candidate| candidate.index == item.index)
        .is_some_and(|after| {
            after.mode
                == crate::native::capture_language::EditorMode::RecordUpdate
                && !after.diagnostics.iter().any(|diagnostic| {
                    diagnostic.severity == capture_language::Severity::Error
                })
        })
}

fn indexed_field_display(
    field: &crate::native::records::RecordField,
    note: &IndexedRecord,
) -> Option<String> {
    let indexed = note.fields.iter().find(|value| value.key == field.key)?;
    let display_scalar = |raw: &str| {
        let raw = raw.trim();
        if raw.is_empty() {
            return None;
        }
        let display = match &field.kind {
            FieldKind::Choices(choices) => choices
                .iter()
                .find(|choice| choice.value == raw)
                .map(|choice| choice.label.clone())
                .unwrap_or_else(|| raw.to_owned()),
            FieldKind::Link => raw
                .strip_prefix("[[")
                .and_then(|inner| inner.strip_suffix("]]"))
                .map(crate::native::capture_block_ids::wikilink_phrase)
                .unwrap_or_else(|| raw.to_owned()),
            _ => raw.to_owned(),
        };
        (!display.is_empty()).then_some(display)
    };
    match &indexed.value {
        crate::native::records::IndexedFieldValue::Scalar(value) => {
            display_scalar(value)
        }
        crate::native::records::IndexedFieldValue::List(values) => {
            let display = values
                .iter()
                .filter_map(|value| display_scalar(value))
                .collect::<Vec<_>>()
                .join(", ");
            (!display.is_empty()).then_some(display)
        }
        crate::native::records::IndexedFieldValue::Null
        | crate::native::records::IndexedFieldValue::Unsupported => None,
    }
}

#[allow(dead_code)]
fn _linked_placeholder() -> Option<Vec<LinkedMatch>> {
    Some(Vec::new())
}
