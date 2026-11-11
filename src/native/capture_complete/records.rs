//! Record field/value completion for `capture-complete`.
//!
//! Reuses the capture record policy, editor parse, and `plan_fields` so
//! menus never invent a second grammar. Catalog I/O happens only when a
//! claimed record item needs values or counts.

use std::cell::Cell;
use std::collections::HashSet;
use std::ops::Range;
use std::path::Path;

use super::engine::CompleteRequest;
use super::model::{
    CandidateContinuation, Candidates, CaptureCompleteDiagnostic,
    CaptureCompleteResult, CompleteError, RecordCompletionMeta,
    RecordEditAction, RecordFieldCandidate, RecordTargetCandidate,
    RecordTargetField, RecordValueCandidate, Replacement,
};
use crate::native::capture_language::{
    self, candidate_collection_routes, candidate_dirs, CompletionContext,
    EditorItemParse, EditorParse, EditorParseOptions, RecordNoteIntent,
    RecordRoutePolicy, RECORDS_CONFIG_INVALID_CODE, RECORD_ASSIST_VERSION,
};
use crate::native::capture_record::{
    record_policy_for_draft, resolve_record_intents_with_catalog,
    RecordParseEntry,
};
use crate::native::records::{
    self, FieldInputState, FieldKind, FolderIndex, IndexedFieldValue,
    IndexedRecord, NearValueHint, RecordCatalog, RecordDate, RecordField,
    RecordPropertyState, RecordScalar, RecordType, RecordValue,
    ValidatedFieldInput,
};

thread_local! {
    static LAST_CATALOG_FILES_OPENED: Cell<usize> = const { Cell::new(0) };
}

#[cfg(test)]
pub(super) fn last_catalog_files_opened() -> usize {
    LAST_CATALOG_FILES_OPENED.get()
}

fn set_files_opened(count: usize) {
    LAST_CATALOG_FILES_OPENED.set(count);
}

pub(super) fn try_complete(
    request: &CompleteRequest<'_>,
) -> Result<Option<CaptureCompleteResult>, CompleteError> {
    set_files_opened(0);
    if request.next_missing_field {
        return Ok(Some(next_missing_field(request)?));
    }
    if request.record_field.is_some() {
        return Ok(Some(named_field(request)?));
    }
    if candidate_dirs(request.raw_text).is_empty()
        && candidate_collection_routes(request.raw_text).is_empty()
    {
        return Ok(None);
    }
    let mut session = Session::open(request);
    session.complete()
}

pub(super) fn next_missing_field(
    request: &CompleteRequest<'_>,
) -> Result<CaptureCompleteResult, CompleteError> {
    set_files_opened(0);
    let mut session = Session::open(request);
    session.next_missing()
}

pub(super) fn named_field(
    request: &CompleteRequest<'_>,
) -> Result<CaptureCompleteResult, CompleteError> {
    set_files_opened(0);
    let key = request.record_field.unwrap_or("");
    let policy =
        record_policy_for_draft(request.bob_dir, None, request.raw_text);
    let mut session = Session::open_with_policy(request, policy);
    if let Some(result) = session.route_diagnostic_result() {
        return Ok(result);
    }
    session.named_field(key)
}

struct Session<'a> {
    request: &'a CompleteRequest<'a>,
    policy: RecordRoutePolicy,
    parsed: EditorParse,
    catalog: RecordCatalog,
    today: chrono::NaiveDate,
}

impl<'a> Session<'a> {
    fn open(request: &'a CompleteRequest<'a>) -> Self {
        let policy =
            record_policy_for_draft(request.bob_dir, None, request.raw_text);
        Self::open_with_policy(request, policy)
    }

    fn open_with_policy(
        request: &'a CompleteRequest<'a>,
        policy: RecordRoutePolicy,
    ) -> Self {
        let notebook_policy =
            crate::native::capture_notebooks::notebook_policy_for_draft(
                request.bob_dir,
                request.raw_text,
            );
        let parsed = capture_language::parse_for_editor_with(
            request.raw_text,
            &EditorParseOptions {
                url_routing: None,
                has_global_destination: false,
                record_policy: Some(&policy),
                notebook_policy: Some(&notebook_policy),
                global_route: None,
            },
        );
        Self {
            request,
            policy,
            parsed,
            catalog: RecordCatalog::new(request.bob_dir),
            today: crate::native::env::current_datetime().date(),
        }
    }

    /// Route error scoped to the cursor item: a broken album item keeps its
    /// diagnostic and remains non-actionable, while a different healthy
    /// album's field picker or an ordinary/notebook item keeps working.
    /// Diagnostics without a range are only returned when the cursor item
    /// itself carries a record intent.
    fn route_diagnostic_for_current(
        &self,
    ) -> Option<&capture_language::Diagnostic> {
        let cursor = self.request.cursor;
        let current = self
            .parsed
            .items
            .iter()
            .find(|item| item.start <= cursor && cursor <= item.end)?;
        let current_intent =
            self.parsed
                .record_intents
                .iter()
                .find_map(|(index, intent)| {
                    (*index == current.index).then_some(intent)
                });
        self.parsed.diagnostics.iter().find(|diagnostic| {
            let is_route_error = diagnostic.code == "album_schema_invalid"
                || diagnostic.code == RECORDS_CONFIG_INVALID_CODE
                || diagnostic.code
                    == capture_language::RECORD_DISCOVERY_INCOMPLETE_CODE;
            if !is_route_error {
                return false;
            }
            match diagnostic.range {
                Some((start, end)) => {
                    // Overlap the cursor item so another item's broken album
                    // never blocks this item's completion.
                    start < current.end && end > current.start
                }
                None => current_intent.is_some(),
            }
        })
    }

    fn route_diagnostic_result(&self) -> Option<CaptureCompleteResult> {
        let diagnostic = self.route_diagnostic_for_current()?;
        let diagnostic = diagnostic.clone();
        let mut result = CaptureCompleteResult::empty(self.request.cursor);
        result.diagnostics.push(CaptureCompleteDiagnostic {
            severity: diagnostic.severity.label(),
            code: diagnostic.code,
            message: diagnostic.message.clone(),
            range: diagnostic
                .range
                .map(|(start, end)| Replacement { start, end }),
            field: self.request.record_field.map(str::to_owned),
            authored: None,
            proposed: None,
        });
        Some(result)
    }

    fn complete(
        &mut self,
    ) -> Result<Option<CaptureCompleteResult>, CompleteError> {
        let Some((item, intent)) = self.record_item() else {
            if let Some(result) = self.route_diagnostic_result() {
                set_files_opened(self.catalog.files_opened());
                return Ok(Some(result));
            }
            set_files_opened(self.catalog.files_opened());
            return Ok(None);
        };
        let item = item.clone();
        let intent = intent.cloned();
        if intent.is_none() {
            if let Some(result) = self.route_diagnostic_result() {
                set_files_opened(self.catalog.files_opened());
                return Ok(Some(result));
            }
            set_files_opened(self.catalog.files_opened());
            return Ok(None);
        }
        if let Some(intent) = intent.as_ref()
            && let Some(probe) = value_probe(self.request, intent, &self.policy)
        {
            let result = self.value_menu(&item, intent, probe)?;
            set_files_opened(self.catalog.files_opened());
            return Ok(Some(result));
        }
        if let Some(intent) = intent.as_ref()
            && let Some(probe) = field_key_probe(
                self.request.raw_text,
                &item,
                self.request.cursor,
            )
        {
            let result = self.field_menu(&item, intent, probe)?;
            set_files_opened(self.catalog.files_opened());
            return Ok(result);
        }
        if let Some(intent) = intent.as_ref()
            && let Some(probe) = target_probe(self.request, intent)
        {
            let result = self.target_menu(intent, probe);
            set_files_opened(self.catalog.files_opened());
            return Ok(Some(result));
        }
        set_files_opened(self.catalog.files_opened());
        Ok(None)
    }

    fn target_menu(
        &mut self,
        intent: &RecordNoteIntent,
        probe: TargetProbe,
    ) -> CaptureCompleteResult {
        let Some(record_type) = self
            .policy
            .usable_record_type_for_route(&intent.dir)
            .cloned()
        else {
            return CaptureCompleteResult::empty(self.request.cursor);
        };
        let config = crate::native::records::RecordsConfig {
            types: vec![record_type.clone()],
        };
        let notes = self.catalog.folder(&intent.dir).notes().to_vec();
        let mut ranked = Vec::new();
        for note in notes {
            let Ok(target) = records::resolve_update_target(
                &config,
                &mut self.catalog,
                &record_type.id,
                &note.stem,
            ) else {
                // An ambiguous or unsafe filesystem identity is not an
                // actionable completion target.
                continue;
            };
            if !super::super::capture_language::is_addressable_update_stem(
                &target.stem,
            ) {
                // A grammar-unaddressable stem (for example with spaces)
                // no longer parses as that target, so it is not actionable.
                continue;
            }
            let title = note.title.clone().unwrap_or_else(|| note.stem.clone());
            let Some((rank, distance)) =
                target_match_rank(&probe.query, &target.stem, &title)
            else {
                continue;
            };
            let status = record_type
                .field("status")
                .and_then(|field| indexed_field_display(field, &note));
            let fields = record_type
                .fields
                .iter()
                .filter(|field| field.suggest)
                .take(2)
                .filter_map(|field| {
                    indexed_field_display(field, &note).map(|value| {
                        RecordTargetField {
                            key: field.key.clone(),
                            label: field.label.clone(),
                            value,
                        }
                    })
                })
                .collect::<Vec<_>>();
            let route = format!("@{}/{}", record_type.dir, target.stem);
            let cursor_after = probe.replace.start + route.len();
            ranked.push((
                rank,
                distance,
                title.to_lowercase(),
                target.stem.to_lowercase(),
                target.path.to_string_lossy().into_owned(),
                RecordTargetCandidate {
                    replacement: route.clone(),
                    route,
                    stem: target.stem,
                    title,
                    status,
                    fields,
                    cursor_after,
                    continuation: CandidateContinuation {
                        cursor: cursor_after,
                        next_missing_field: true,
                    },
                },
            ));
        }
        ranked.sort_by(|left, right| {
            left.0
                .cmp(&right.0)
                .then_with(|| left.1.cmp(&right.1))
                .then_with(|| left.2.cmp(&right.2))
                .then_with(|| left.3.cmp(&right.3))
                .then_with(|| left.4.cmp(&right.4))
        });
        let mut result = CaptureCompleteResult::empty(self.request.cursor);
        result.context = Some(CompletionContext::RecordTarget);
        result.replacement = Replacement {
            start: probe.replace.start,
            end: probe.replace.end,
        };
        result.query = Some(probe.query);
        result.candidates = Candidates::RecordTarget(
            ranked.into_iter().map(|row| row.5).collect(),
        );
        result.record = Some(RecordCompletionMeta {
            record_type: record_type.id,
            dir: record_type.dir,
            label: record_type.label,
            emoji: record_type.emoji,
            field: None,
            title: None,
            current: None,
            assist_version: Some(RECORD_ASSIST_VERSION),
            edit: None,
        });
        result
    }

    fn next_missing(&mut self) -> Result<CaptureCompleteResult, CompleteError> {
        let Some((item, intent)) = self.record_item() else {
            if let Some(result) = self.route_diagnostic_result() {
                set_files_opened(self.catalog.files_opened());
                return Ok(result);
            }
            set_files_opened(self.catalog.files_opened());
            return Ok(CaptureCompleteResult::empty(self.request.cursor));
        };
        let item = item.clone();
        let Some(intent) = intent.cloned() else {
            if let Some(result) = self.route_diagnostic_result() {
                set_files_opened(self.catalog.files_opened());
                return Ok(result);
            }
            set_files_opened(self.catalog.files_opened());
            return Ok(CaptureCompleteResult::empty(self.request.cursor));
        };
        let entry = self.plan_entry(&intent);
        let Some(missing_key) = entry.missing.first().cloned() else {
            set_files_opened(self.catalog.files_opened());
            return Ok(CaptureCompleteResult::empty(self.request.cursor));
        };
        let Some(field) = entry.record_type.field(&missing_key).cloned() else {
            set_files_opened(self.catalog.files_opened());
            return Ok(CaptureCompleteResult::empty(self.request.cursor));
        };
        let result =
            if let Some(existing) = intent.fields.iter().find(|input| {
                input.key == missing_key && input.value.is_empty()
            }) {
                let probe = ValueProbe {
                    field: field.clone(),
                    query: String::new(),
                    replace: existing.value_range.0..existing.value_range.1,
                    insert_bullet: None,
                };
                self.value_menu(&item, &intent, probe)?
            } else {
                let at = insert_point_after_fields(
                    self.request.raw_text,
                    &item,
                    &intent,
                );
                let probe = ValueProbe {
                    field: field.clone(),
                    query: String::new(),
                    replace: at..at,
                    insert_bullet: Some(field.key.clone()),
                };
                self.value_menu(&item, &intent, probe)?
            };
        set_files_opened(self.catalog.files_opened());
        Ok(result)
    }

    fn named_field(
        &mut self,
        key: &str,
    ) -> Result<CaptureCompleteResult, CompleteError> {
        let Some((item, intent)) = self.record_item() else {
            set_files_opened(self.catalog.files_opened());
            return Ok(CaptureCompleteResult::empty(self.request.cursor));
        };
        let item = item.clone();
        let Some(intent) = intent.cloned() else {
            set_files_opened(self.catalog.files_opened());
            return Ok(CaptureCompleteResult::empty(self.request.cursor));
        };
        let Some(record_type) = self.record_type(&intent) else {
            set_files_opened(self.catalog.files_opened());
            return Ok(CaptureCompleteResult::empty(self.request.cursor));
        };
        let entry = self.plan_entry(&intent);
        let Some(field) = record_type.field(key).cloned() else {
            let mut result = self.finish(
                &item,
                &intent,
                CompletionContext::RecordValue,
                self.request.cursor..self.request.cursor,
                Candidates::RecordValue(Vec::new()),
                None,
                Some(key.to_string()),
                &entry,
            );
            result
                .diagnostics
                .push(unknown_field_diagnostic(key, &record_type));
            set_files_opened(self.catalog.files_opened());
            return Ok(result);
        };
        let probe = named_value_probe(self.request, &item, &intent, &field);
        let mut result = self.value_menu(&item, &intent, probe.clone())?;
        if let Some(record) = result.record.as_mut() {
            record.edit = record_edit_action(
                self.request,
                &item,
                &probe,
                &entry,
                self.today,
            );
        }
        set_files_opened(self.catalog.files_opened());
        Ok(result)
    }

    fn record_item(
        &self,
    ) -> Option<(&EditorItemParse, Option<&RecordNoteIntent>)> {
        let cursor = self.request.cursor;
        let item = self
            .parsed
            .items
            .iter()
            .find(|item| item.start <= cursor && cursor <= item.end)?;
        let intent =
            self.parsed
                .record_intents
                .iter()
                .find_map(|(index, intent)| {
                    (*index == item.index).then_some(intent)
                });
        Some((item, intent))
    }

    fn plan_entry(&mut self, intent: &RecordNoteIntent) -> RecordParseEntry {
        resolve_record_intents_with_catalog(
            &[(0, intent.clone())],
            Some(&self.policy),
            &mut self.catalog,
        )
        .into_iter()
        .next()
        .unwrap_or_else(|| empty_entry(intent))
    }

    fn record_type(&self, intent: &RecordNoteIntent) -> Option<RecordType> {
        self.policy
            .usable_record_type_for_route(&intent.dir)
            .cloned()
    }

    fn field_menu(
        &mut self,
        item: &EditorItemParse,
        intent: &RecordNoteIntent,
        probe: FieldKeyProbe,
    ) -> Result<Option<CaptureCompleteResult>, CompleteError> {
        let Some(record_type) = self.record_type(intent) else {
            return Ok(None);
        };
        let entry = self.plan_entry(intent);
        let authored: HashSet<&str> = intent
            .fields
            .iter()
            .map(|field| field.key.as_str())
            .collect();
        let mut required = Vec::new();
        let mut optional = Vec::new();
        for field in &record_type.fields {
            if authored.contains(field.key.as_str()) {
                continue;
            }
            if !query_matches(&probe.query, &field.key)
                && !query_matches(&probe.query, &field.label)
            {
                continue;
            }
            let defaulted = entry
                .properties
                .iter()
                .find(|property| property.key == field.key)
                .is_some_and(|property| {
                    property.state == RecordPropertyState::Default
                });
            if field.required {
                required.push((field, defaulted));
            } else {
                optional.push((field, defaulted));
            }
        }
        required.extend(optional);
        if required.is_empty() {
            return Ok(None);
        }
        let mut rows = Vec::new();
        for (field, defaulted) in required {
            let insert = format!("{}:: ", field.key);
            let cursor_after = probe.replace.start + insert.len();
            let current = entry
                .update_properties
                .iter()
                .find(|property| property.key == field.key)
                .and_then(|property| property.current_display.clone());
            rows.push(RecordFieldCandidate {
                replacement: insert,
                key: field.key.clone(),
                label: field.label.clone(),
                kind: field.kind.kind_name(),
                required: field.required,
                list: field.list,
                defaulted: defaulted
                    && entry.action
                        == super::super::capture_record::RecordParseAction::Create,
                current,
                cursor_after,
                continuation: CandidateContinuation {
                    cursor: cursor_after,
                    next_missing_field: false,
                },
            });
        }
        Ok(Some(self.finish(
            item,
            intent,
            CompletionContext::RecordField,
            probe.replace,
            Candidates::RecordField(rows),
            Some(probe.query),
            None,
            &entry,
        )))
    }

    fn value_menu(
        &mut self,
        item: &EditorItemParse,
        intent: &RecordNoteIntent,
        probe: ValueProbe,
    ) -> Result<CaptureCompleteResult, CompleteError> {
        let entry = self.plan_entry(intent);
        let folder = self.catalog.folder(&intent.dir).clone();
        let rows = self.value_rows(item, &probe, &entry, &folder)?;
        let query = probe.query.clone();
        let field_key = probe.field.key.clone();
        Ok(self.finish(
            item,
            intent,
            CompletionContext::RecordValue,
            probe.replace.clone(),
            Candidates::RecordValue(rows),
            Some(query),
            Some(field_key),
            &entry,
        ))
    }

    fn value_rows(
        &self,
        item: &EditorItemParse,
        probe: &ValueProbe,
        entry: &RecordParseEntry,
        folder: &FolderIndex,
    ) -> Result<Vec<RecordValueCandidate>, CompleteError> {
        let field = &probe.field;
        let mut suggestions = match &field.kind {
            FieldKind::Choices(choices) => choices
                .iter()
                .filter(|choice| {
                    query_matches(&probe.query, &choice.value)
                        || query_matches(&probe.query, &choice.label)
                })
                .map(|choice| SuggestedValue {
                    insert: choice.value.clone(),
                    display: choice.label.clone(),
                    count: None,
                })
                .collect(),
            FieldKind::Date => ["today", "yesterday"]
                .into_iter()
                .filter(|value| query_matches(&probe.query, value))
                .map(|value| SuggestedValue {
                    insert: value.to_string(),
                    display: value.to_string(),
                    count: None,
                })
                .collect(),
            FieldKind::Text if field.suggest => folder
                .value_suggestions(&field.key)
                .into_iter()
                .filter(|suggestion| {
                    value_is_valid(field, &suggestion.insert, self.today)
                        && (query_matches(&probe.query, &suggestion.insert)
                            || query_matches(
                                &probe.query,
                                &suggestion.display(),
                            ))
                })
                .map(|suggestion| SuggestedValue {
                    display: suggestion.display(),
                    count: Some(suggestion.count),
                    insert: suggestion.insert,
                })
                .collect(),
            FieldKind::Link => link_suggestions(
                self.request.bob_dir,
                field,
                folder,
                &probe.query,
                self.today,
            )?,
            FieldKind::Text | FieldKind::Url | FieldKind::Number => Vec::new(),
        };
        if matches!(
            field.kind,
            FieldKind::Url | FieldKind::Number | FieldKind::Text
        ) && !field.suggest
            && suggestions.is_empty()
            && probe.query.is_empty()
        {
            suggestions.push(SuggestedValue {
                insert: String::new(),
                display: String::new(),
                count: None,
            });
        }
        let remaining_after = remaining_missing(entry, &field.key);
        let mut rows = Vec::new();
        for suggestion in suggestions {
            let insert = value_insert_text(
                self.request.raw_text,
                &probe.replace,
                &suggestion.insert,
                probe.insert_bullet.as_deref(),
                item_newline(self.request.raw_text, item.start..item.end),
            );
            let cursor_after = probe.replace.start
                + insert.trim_end_matches(['\n', '\r']).len();
            let continuation = value_continuation(
                cursor_after,
                remaining_after,
                &suggestion.insert,
            );
            rows.push(RecordValueCandidate {
                replacement: insert,
                value: suggestion.insert,
                display: suggestion.display,
                kind: field.kind.kind_name(),
                count: suggestion.count,
                cursor_after,
                continuation,
            });
        }
        Ok(rows)
    }

    #[allow(clippy::too_many_arguments)]
    fn finish(
        &self,
        item: &EditorItemParse,
        intent: &RecordNoteIntent,
        context: CompletionContext,
        replace: Range<usize>,
        candidates: Candidates,
        query: Option<String>,
        field: Option<String>,
        entry: &RecordParseEntry,
    ) -> CaptureCompleteResult {
        let mut result = CaptureCompleteResult::empty(self.request.cursor);
        result.context = Some(context);
        result.replacement = Replacement {
            start: replace.start,
            end: replace.end,
        };
        result.candidates = candidates;
        result.query = query;
        let current = field.as_ref().and_then(|key| {
            entry
                .update_properties
                .iter()
                .find(|property| &property.key == key)
                .and_then(|property| property.current_display.clone())
        });
        // For updates the parent-line note is a note to append, never the
        // record title: use the planned entry's stored title, or None when
        // the target is unresolved.
        let title = if entry.action
            == super::super::capture_record::RecordParseAction::Update
        {
            entry.title.clone()
        } else {
            (!intent.title.is_empty()).then(|| intent.title.clone())
        };
        result.record = Some(RecordCompletionMeta {
            record_type: entry.record_type.id.clone(),
            dir: intent.dir.clone(),
            label: entry.record_type.label.clone(),
            emoji: entry.record_type.emoji.clone(),
            field,
            title,
            current,
            assist_version: Some(RECORD_ASSIST_VERSION),
            edit: None,
        });
        result.diagnostics =
            near_diagnostics(self.request.raw_text, &entry.near_values);
        let _ = item;
        result
    }
}

struct FieldKeyProbe {
    query: String,
    replace: Range<usize>,
}

#[derive(Clone)]
struct ValueProbe {
    field: RecordField,
    query: String,
    replace: Range<usize>,
    insert_bullet: Option<String>,
}

struct SuggestedValue {
    insert: String,
    display: String,
    count: Option<usize>,
}

struct TargetProbe {
    query: String,
    replace: Range<usize>,
}

fn target_probe(
    request: &CompleteRequest<'_>,
    intent: &RecordNoteIntent,
) -> Option<TargetProbe> {
    if intent.explicit_stem.is_some()
        || request.cursor < intent.route_range.0
        || request.cursor > intent.route_range.1
    {
        return None;
    }
    let query = match intent.action {
        crate::native::capture_language::record::RecordAction::Create => {
            String::new()
        }
        crate::native::capture_language::record::RecordAction::Update => {
            intent.update_stem.clone().unwrap_or_default()
        }
    };
    Some(TargetProbe {
        query,
        replace: intent.route_range.0..intent.route_range.1,
    })
}

fn target_match_rank(
    query: &str,
    stem: &str,
    title: &str,
) -> Option<(u8, usize)> {
    let query = query.to_lowercase();
    if query.is_empty() {
        return Some((0, 0));
    }
    let stem = stem.to_lowercase();
    let title = title.to_lowercase();
    if query == stem || query == title {
        Some((0, 0))
    } else if stem.starts_with(&query) || title.starts_with(&query) {
        Some((1, 0))
    } else {
        crate::native::note_tasks::bounded_levenshtein(&query, &title, 2)
            .map(|distance| (2, distance))
    }
}

fn indexed_field_display(
    field: &RecordField,
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
        IndexedFieldValue::Scalar(value) => display_scalar(value),
        IndexedFieldValue::List(values) => {
            let display = values
                .iter()
                .filter_map(|value| display_scalar(value))
                .collect::<Vec<_>>()
                .join(", ");
            (!display.is_empty()).then_some(display)
        }
        IndexedFieldValue::Null | IndexedFieldValue::Unsupported => None,
    }
}

fn field_key_probe(
    raw_text: &str,
    item: &EditorItemParse,
    cursor: usize,
) -> Option<FieldKeyProbe> {
    for line in capture_language::split_physical_lines(raw_text) {
        if cursor < line.start || cursor > line.end {
            continue;
        }
        if line.start == item.line_start {
            return None;
        }
        if line.start < item.start || line.start > item.end {
            return None;
        }
        let (body_offset, body) = first_level_dash_body(line.text)?;
        let body_start = line.start + body_offset;
        if cursor < body_start {
            return None;
        }
        if body.contains("::") {
            return None;
        }
        let typed = raw_text.get(body_start..cursor).unwrap_or("");
        if !typed.bytes().all(|byte| {
            byte.is_ascii_alphanumeric()
                || matches!(byte, b'_' | b'-' | b' ' | b'\t')
        }) {
            return None;
        }
        let query = typed.trim().to_string();
        let body_end = body_start + body.trim_end().len();
        return Some(FieldKeyProbe {
            query,
            replace: body_start..body_end,
        });
    }
    None
}

fn first_level_dash_body(line_text: &str) -> Option<(usize, &str)> {
    let bytes = line_text.as_bytes();
    if bytes.first() != Some(&b'-') {
        return None;
    }
    let separator = *bytes.get(1)?;
    if !matches!(separator, b' ' | b'\t') {
        return None;
    }
    let mut end = 2;
    while end < bytes.len() && matches!(bytes[end], b' ' | b'\t') {
        end += 1;
    }
    Some((end, &line_text[end..]))
}

fn value_probe(
    request: &CompleteRequest<'_>,
    intent: &RecordNoteIntent,
    policy: &RecordRoutePolicy,
) -> Option<ValueProbe> {
    let cursor = request.cursor;
    let record_type = policy.usable_record_type_for_route(&intent.dir)?;
    for input in &intent.fields {
        let schema =
            record_type.field(&input.key).cloned().unwrap_or_else(|| {
                RecordField {
                    key: input.key.clone(),
                    label: input.key.clone(),
                    kind: FieldKind::Text,
                    required: false,
                    list: false,
                    default: None,
                    suggest: false,
                    min: None,
                    max: None,
                }
            });
        let value_start = input.value_range.0;
        let value_end = input.value_range.1;
        let colon_end = input.key_range.1 + 2;
        let in_value = cursor >= colon_end.min(value_start)
            && cursor <= value_end.max(colon_end);
        if !in_value {
            continue;
        }
        if schema.list {
            let (member_start, member_end) = list_member_at(
                request.raw_text,
                value_start,
                value_end,
                cursor,
            );
            let query = request
                .raw_text
                .get(member_start..cursor.min(member_end))
                .unwrap_or("")
                .trim_start()
                .to_string();
            return Some(ValueProbe {
                field: schema,
                query,
                replace: member_start..member_end,
                insert_bullet: None,
            });
        }
        let query_end = cursor.min(value_end);
        let query = request
            .raw_text
            .get(value_start..query_end)
            .unwrap_or("")
            .to_string();
        return Some(ValueProbe {
            field: schema,
            query,
            replace: value_start..value_end,
            insert_bullet: None,
        });
    }
    None
}

fn named_value_probe(
    request: &CompleteRequest<'_>,
    item: &EditorItemParse,
    intent: &RecordNoteIntent,
    field: &RecordField,
) -> ValueProbe {
    let authored: Vec<_> = intent
        .fields
        .iter()
        .filter(|input| input.key == field.key)
        .collect();
    if authored.is_empty() {
        let at = insert_point_after_fields(request.raw_text, item, intent);
        return ValueProbe {
            field: field.clone(),
            query: String::new(),
            replace: at..at,
            insert_bullet: Some(field.key.clone()),
        };
    }
    let selected = authored
        .iter()
        .copied()
        .find(|input| caret_in_field_value(request, input))
        .unwrap_or(authored[authored.len() - 1]);
    if field.list {
        let cursor = if caret_in_field_value(request, selected) {
            request.cursor
        } else {
            selected.value_range.1
        };
        let (member_start, member_end) = list_member_at(
            request.raw_text,
            selected.value_range.0,
            selected.value_range.1,
            cursor,
        );
        return ValueProbe {
            field: field.clone(),
            query: String::new(),
            replace: member_start..member_end,
            insert_bullet: None,
        };
    }
    ValueProbe {
        field: field.clone(),
        query: String::new(),
        replace: selected.value_range.0..selected.value_range.1,
        insert_bullet: None,
    }
}

fn caret_in_field_value(
    request: &CompleteRequest<'_>,
    input: &crate::native::capture_language::record::RecordFieldInput,
) -> bool {
    let value_start = input.value_range.0;
    let value_end = input.value_range.1;
    let colon_end = input.key_range.1 + 2;
    let cursor = request.cursor;
    cursor >= colon_end.min(value_start) && cursor <= value_end.max(colon_end)
}

fn record_edit_action(
    request: &CompleteRequest<'_>,
    item: &EditorItemParse,
    probe: &ValueProbe,
    entry: &RecordParseEntry,
    today: chrono::NaiveDate,
) -> Option<RecordEditAction> {
    let newline = item_newline(request.raw_text, item.start..item.end);
    if let Some(key) = probe.insert_bullet.as_deref() {
        // For updates, an empty optional value removes the key, so an
        // untouched field must pre-fill the snapshot's current value in
        // capture spelling. This is the update counterpart of materializing
        // a create default.
        if entry.action
            == super::super::capture_record::RecordParseAction::Update
        {
            let current = entry
                .update_properties
                .iter()
                .find(|property| property.key == key)
                .and_then(|property| property.current.clone());
            let Some(current) = current else {
                let replacement = value_insert_text(
                    request.raw_text,
                    &probe.replace,
                    "",
                    Some(key),
                    newline,
                );
                let cursor_after =
                    edit_value_cursor(probe.replace.start, &replacement, "");
                return Some(RecordEditAction {
                    replacement_range: Replacement {
                        start: probe.replace.start,
                        end: probe.replace.end,
                    },
                    replacement,
                    cursor_after,
                    continuation: CandidateContinuation {
                        cursor: cursor_after,
                        next_missing_field: false,
                    },
                });
            };
            let spelling =
                lossless_capture_spelling(&probe.field, &current, today)?;
            let replacement = value_insert_text(
                request.raw_text,
                &probe.replace,
                &spelling,
                Some(key),
                newline,
            );
            let cursor_after =
                edit_value_cursor(probe.replace.start, &replacement, &spelling);
            return Some(RecordEditAction {
                replacement_range: Replacement {
                    start: probe.replace.start,
                    end: probe.replace.end,
                },
                replacement,
                cursor_after,
                continuation: CandidateContinuation {
                    cursor: cursor_after,
                    next_missing_field: false,
                },
            });
        }
        let defaulted = entry.properties.iter().any(|property| {
            property.key == key
                && property.state == RecordPropertyState::Default
        });
        let spelling = if defaulted {
            probe
                .field
                .default
                .as_ref()
                .map(capture_value_spelling)
                .unwrap_or_default()
        } else {
            String::new()
        };
        let replacement = value_insert_text(
            request.raw_text,
            &probe.replace,
            &spelling,
            Some(key),
            newline,
        );
        let cursor_after =
            edit_value_cursor(probe.replace.start, &replacement, &spelling);
        return Some(RecordEditAction {
            replacement_range: Replacement {
                start: probe.replace.start,
                end: probe.replace.end,
            },
            replacement,
            cursor_after,
            continuation: CandidateContinuation {
                cursor: cursor_after,
                next_missing_field: false,
            },
        });
    }
    let current = request
        .raw_text
        .get(probe.replace.clone())
        .unwrap_or("")
        .to_string();
    Some(RecordEditAction {
        replacement_range: Replacement {
            start: probe.replace.start,
            end: probe.replace.end,
        },
        replacement: current,
        cursor_after: probe.replace.start,
        continuation: CandidateContinuation {
            cursor: probe.replace.start,
            next_missing_field: false,
        },
    })
}

/// Capture spelling of a snapshot current value when it round-trips
/// losslessly. Returns `None` for values that are invalid under the schema
/// and for list members containing `,`, which would split on resubmit.
fn lossless_capture_spelling(
    field: &RecordField,
    value: &RecordValue,
    today: chrono::NaiveDate,
) -> Option<String> {
    if let RecordValue::List(members) = value {
        for member in members {
            let spelling = match member {
                RecordScalar::Text(text)
                | RecordScalar::Choice(text)
                | RecordScalar::Link(text) => text.clone(),
                RecordScalar::Date(RecordDate::Resolved(date)) => {
                    date.format("%Y-%m-%d").to_string()
                }
                RecordScalar::Date(RecordDate::Today) => "today".to_owned(),
                RecordScalar::Date(RecordDate::Yesterday) => {
                    "yesterday".to_owned()
                }
                RecordScalar::Url(url) => url.to_string(),
                RecordScalar::Number(number) => number.to_string(),
            };
            if spelling.contains(',') {
                return None;
            }
        }
    }
    let spelling = capture_value_spelling(value);
    let validated = super::super::records::schema::validate_field_values(
        field,
        &[spelling.clone()],
        today,
    )
    .ok()?;
    let resolved = match &validated {
        RecordValue::Scalar(scalar) => match scalar {
            RecordScalar::Date(date) => {
                let resolved = date.resolve(today)?;
                RecordValue::Scalar(RecordScalar::Date(RecordDate::Resolved(
                    resolved,
                )))
            }
            _ => validated.clone(),
        },
        RecordValue::List(members) => {
            let mut resolved_members = Vec::new();
            for member in members {
                match member {
                    RecordScalar::Date(date) => {
                        let resolved = date.resolve(today)?;
                        resolved_members.push(RecordScalar::Date(
                            RecordDate::Resolved(resolved),
                        ));
                    }
                    other => resolved_members.push(other.clone()),
                }
            }
            RecordValue::List(resolved_members)
        }
    };
    let current_resolved = match value {
        RecordValue::Scalar(RecordScalar::Date(date)) => {
            let resolved = date.resolve(today)?;
            RecordValue::Scalar(RecordScalar::Date(RecordDate::Resolved(
                resolved,
            )))
        }
        RecordValue::List(members) => {
            let mut out = Vec::new();
            for member in members {
                match member {
                    RecordScalar::Date(date) => {
                        let resolved = date.resolve(today)?;
                        out.push(RecordScalar::Date(RecordDate::Resolved(
                            resolved,
                        )));
                    }
                    other => out.push(other.clone()),
                }
            }
            RecordValue::List(out)
        }
        other => other.clone(),
    };
    (resolved == current_resolved).then_some(spelling)
}

fn edit_value_cursor(replace_start: usize, insert: &str, value: &str) -> usize {
    let trimmed = insert.trim_end_matches(['\n', '\r']);
    replace_start + trimmed.len().saturating_sub(value.len())
}

fn capture_value_spelling(value: &RecordValue) -> String {
    let one = |scalar: &RecordScalar| match scalar {
        RecordScalar::Text(text)
        | RecordScalar::Choice(text)
        | RecordScalar::Link(text) => text.clone(),
        RecordScalar::Date(RecordDate::Resolved(date)) => {
            date.format("%Y-%m-%d").to_string()
        }
        RecordScalar::Date(RecordDate::Today) => "today".to_owned(),
        RecordScalar::Date(RecordDate::Yesterday) => "yesterday".to_owned(),
        RecordScalar::Url(url) => url.to_string(),
        RecordScalar::Number(number) => number.to_string(),
    };
    match value {
        RecordValue::Scalar(scalar) => one(scalar),
        RecordValue::List(values) => {
            values.iter().map(one).collect::<Vec<_>>().join(", ")
        }
    }
}

fn unknown_field_diagnostic(
    key: &str,
    record_type: &RecordType,
) -> CaptureCompleteDiagnostic {
    let fields = record_type
        .fields
        .iter()
        .map(|field| field.key.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    CaptureCompleteDiagnostic {
        severity: "error",
        code: "record_unknown_field",
        message: format!(
            "`{key}` is not a {} field; fields: {fields}",
            record_type.label
        ),
        range: None,
        field: Some(key.to_string()),
        authored: Some(key.to_string()),
        proposed: None,
    }
}

fn list_member_at(
    raw: &str,
    value_start: usize,
    value_end: usize,
    cursor: usize,
) -> (usize, usize) {
    let value = raw.get(value_start..value_end).unwrap_or("");
    let relative = cursor.saturating_sub(value_start).min(value.len());
    let mut start = 0;
    let mut in_wikilink = false;
    let bytes = value.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index..].starts_with(b"[[") {
            in_wikilink = true;
            index += 2;
            continue;
        }
        if bytes[index..].starts_with(b"]]") {
            in_wikilink = false;
            index += 2;
            continue;
        }
        if bytes[index] == b',' && !in_wikilink {
            if relative <= index {
                let member = &value[start..index];
                let trim_start = member.len() - member.trim_start().len();
                let trim_end = member.trim_end().len();
                let member_start = value_start + start + trim_start;
                let member_end = value_start + start + trim_end.max(trim_start);
                return (member_start, member_end);
            }
            start = index + 1;
        }
        index += value[index..]
            .chars()
            .next()
            .map(char::len_utf8)
            .unwrap_or(1);
    }
    let member = &value[start..];
    let trim_start = member.len() - member.trim_start().len();
    let trim_end = member.trim_end().len();
    let member_start = value_start + start + trim_start;
    let member_end = value_start + start + trim_end.max(trim_start);
    (member_start, member_end)
}

fn insert_point_after_fields(
    raw_text: &str,
    item: &EditorItemParse,
    intent: &RecordNoteIntent,
) -> usize {
    if let Some(last) = intent.fields.last() {
        let end = last.value_range.1.max(last.key_range.1);
        return after_line_containing(raw_text, end);
    }
    after_line_containing(raw_text, item.start)
}

fn after_line_containing(raw_text: &str, offset: usize) -> usize {
    for line in capture_language::split_physical_lines(raw_text) {
        if offset >= line.start && offset <= line.end {
            let mut index = line.end;
            if raw_text.as_bytes().get(index) == Some(&b'\r') {
                index += 1;
            }
            if raw_text.as_bytes().get(index) == Some(&b'\n') {
                index += 1;
            }
            return index;
        }
    }
    raw_text.len()
}

fn item_newline(raw_text: &str, range: Range<usize>) -> &'static str {
    if raw_text
        .get(range)
        .is_some_and(|slice| slice.contains("\r\n"))
    {
        "\r\n"
    } else {
        "\n"
    }
}

fn value_insert_text(
    raw: &str,
    replace: &Range<usize>,
    value: &str,
    insert_bullet: Option<&str>,
    newline: &str,
) -> String {
    if let Some(key) = insert_bullet {
        let bullet = format!("- {key}:: {value}");
        let at = replace.start;
        let at_end = at == raw.len();
        let preceded_by_nl = at > 0 && raw.as_bytes()[at - 1] == b'\n';
        if at_end && !preceded_by_nl {
            format!("{newline}{bullet}")
        } else if at_end {
            bullet
        } else {
            format!("{bullet}{newline}")
        }
    } else if replace.start == replace.end
        && replace.start > 0
        && !raw.as_bytes()[replace.start - 1].is_ascii_whitespace()
        && !value.is_empty()
    {
        format!(" {value}")
    } else {
        value.to_string()
    }
}

fn value_continuation(
    cursor: usize,
    remaining_after: bool,
    insert: &str,
) -> Option<CandidateContinuation> {
    if insert.is_empty() {
        return None;
    }
    remaining_after.then_some(CandidateContinuation {
        cursor,
        next_missing_field: true,
    })
}

fn remaining_missing(entry: &RecordParseEntry, filled: &str) -> bool {
    entry.missing.iter().any(|key| key != filled)
}

fn query_matches(query: &str, text: &str) -> bool {
    if query.is_empty() {
        return true;
    }
    text.to_lowercase().contains(&query.to_lowercase())
}

fn value_is_valid(
    field: &RecordField,
    value: &str,
    today: chrono::NaiveDate,
) -> bool {
    matches!(
        records::schema::validate_field_input(
            field,
            &FieldInputState::SuppliedValues(vec![value.to_string()]),
            today,
        ),
        Ok(ValidatedFieldInput::Value(_))
    )
}

fn link_suggestions(
    bob_dir: &Path,
    field: &RecordField,
    folder: &FolderIndex,
    query: &str,
    today: chrono::NaiveDate,
) -> Result<Vec<SuggestedValue>, CompleteError> {
    let mut rows = Vec::new();
    let mut seen = HashSet::new();
    for suggestion in folder.value_suggestions(&field.key) {
        if !value_is_valid(field, &suggestion.insert, today) {
            continue;
        }
        if !query_matches(query, &suggestion.insert)
            && !query_matches(query, &suggestion.display())
        {
            continue;
        }
        let fold = suggestion.display().to_lowercase();
        if !seen.insert(fold) {
            continue;
        }
        rows.push(SuggestedValue {
            display: suggestion.display(),
            count: Some(suggestion.count),
            insert: suggestion.insert,
        });
    }
    let index = crate::native::capture_links::NoteIndex::read(bob_dir)
        .map_err(CompleteError::io)?;
    for (insert_target, stem) in index.matching_note_inserts(query) {
        let insert = format!("[[{insert_target}]]");
        if !value_is_valid(field, &insert, today) {
            continue;
        }
        let fold = stem.to_lowercase();
        if !seen.insert(fold) {
            continue;
        }
        rows.push(SuggestedValue {
            insert,
            display: stem,
            count: None,
        });
    }
    Ok(rows)
}

fn near_diagnostics(
    raw_text: &str,
    hints: &[NearValueHint],
) -> Vec<CaptureCompleteDiagnostic> {
    hints
        .iter()
        .filter(|hint| hint.source_range.end <= raw_text.len())
        .map(|hint| CaptureCompleteDiagnostic {
            severity: "warning",
            code: "record_value_near",
            message: format!(
                "`{}` looks like more common `{}`",
                hint.authored, hint.proposed
            ),
            range: Some(Replacement {
                start: hint.source_range.start,
                end: hint.source_range.end,
            }),
            field: Some(hint.field.clone()),
            authored: Some(hint.authored.clone()),
            proposed: Some(hint.proposed.clone()),
        })
        .collect()
}

fn empty_entry(intent: &RecordNoteIntent) -> RecordParseEntry {
    RecordParseEntry {
        index: 0,
        intent: intent.clone(),
        action: match intent.action {
            super::super::capture_language::record::RecordAction::Create => {
                super::super::capture_record::RecordParseAction::Create
            }
            super::super::capture_language::record::RecordAction::Update => {
                super::super::capture_record::RecordParseAction::Update
            }
        },
        record_type: RecordType {
            id: intent.dir.clone(),
            dir: intent.dir.clone(),
            label: intent.dir.clone(),
            emoji: None,
            parent: None,
            dated_notes: false,
            fields: Vec::new(),
            legacy: Vec::new(),
        },
        source: "config",
        album: None,
        title: None,
        path: None,
        stem: None,
        stem_source: None,
        properties: Vec::new(),
        update_properties: Vec::new(),
        missing: Vec::new(),
        invalid: Vec::new(),
        warnings: Vec::new(),
        plan_issue: None,
        near_values: Vec::new(),
        duplicate_target_stem: None,
    }
}
