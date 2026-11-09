//! Native adapter between capture parsing and the records engine.
//!
//! Parse and execution build one immutable route policy through
//! [`record_policy_for_draft`]. Parse resolves claimed intents through the
//! existing planner and one invocation-local folder catalog; ordinary task
//! drafts avoid both config loading and record-index reads.

use serde_json::json;
use std::{
    collections::{HashMap, HashSet},
    ops::Range,
    path::{Path, PathBuf},
};

use super::capture_language::{
    candidate_collection_routes, candidate_dirs, DiagnosticRewrite,
    RecordRoutePolicy,
};
use super::{
    capture_language::record::RecordNoteIntent,
    config,
    records::{
        self, FieldInputState, PlannedRecordProperty, PlannedUpdateProperty,
        RecordCatalog, RecordDiagnostic, RecordFieldInput, RecordNoteInput,
        RecordPlanError, RecordPlanErrorCode, RecordStemSource, RecordType,
        ResolvedUpdateTarget,
    },
};

/// Native state shared by every record item in one capture invocation.
/// The immutable route policy owns the already-loaded schema; the catalog
/// is constructed on first record execution and indexes each folder lazily.
pub(crate) struct CaptureRecordContext {
    policy: RecordRoutePolicy,
    vault_root: PathBuf,
    catalog: Option<RecordCatalog>,
}

impl CaptureRecordContext {
    pub(crate) fn new(policy: RecordRoutePolicy, vault_root: &Path) -> Self {
        Self {
            policy,
            vault_root: vault_root.to_path_buf(),
            catalog: None,
        }
    }

    pub(crate) fn config_and_catalog(
        &mut self,
    ) -> Option<(&super::records::RecordsConfig, &mut RecordCatalog)> {
        let config = self.policy.records_config()?;
        let catalog = self
            .catalog
            .get_or_insert_with(|| RecordCatalog::new(&self.vault_root));
        Some((config, catalog))
    }

    #[cfg(test)]
    pub(crate) fn catalog_is_loaded(&self, directory: &str) -> bool {
        self.catalog
            .as_ref()
            .is_some_and(|catalog| catalog.is_loaded(directory))
    }

    #[cfg(test)]
    pub(crate) fn catalog_files_opened(&self) -> usize {
        self.catalog.as_ref().map_or(0, RecordCatalog::files_opened)
    }
}

/// A capture item's record metadata after the existing planner has resolved
/// its schema fields, defaults, stem, and destination.
#[derive(Debug, Clone)]
pub(crate) struct RecordParseEntry {
    pub(crate) index: usize,
    pub(crate) intent: RecordNoteIntent,
    pub(crate) action: RecordParseAction,
    pub(crate) record_type: RecordType,
    pub(crate) title: Option<String>,
    pub(crate) path: Option<PathBuf>,
    pub(crate) stem: Option<String>,
    pub(crate) stem_source: Option<RecordStemSource>,
    pub(crate) properties: Vec<PlannedRecordProperty>,
    pub(crate) update_properties: Vec<PlannedUpdateProperty>,
    pub(crate) missing: Vec<String>,
    pub(crate) invalid: Vec<RecordDiagnostic>,
    pub(crate) warnings: Vec<RecordDiagnostic>,
    pub(crate) plan_issue: Option<RecordParseIssue>,
    pub(crate) near_values: Vec<records::NearValueHint>,
    /// Exact existing or staged target for a safe `record_exists` rewrite.
    pub(crate) duplicate_target_stem: Option<String>,
}

/// Build the item-scoped create-to-update fix shared by `record_exists` and
/// the similar-record picker. The splice covers only the create route and
/// title on the parent line, leaving bullets and surrounding draft bytes
/// untouched. An explicit-new route is a create too, so it is rewritten in
/// exactly the same way when its target is known.
pub(crate) fn create_to_update_rewrite(
    input: &str,
    intent: &RecordNoteIntent,
    target_dir: &str,
    target_stem: &str,
) -> Option<DiagnosticRewrite> {
    if intent.action != super::capture_language::record::RecordAction::Create
        || !intent.dir.eq_ignore_ascii_case(target_dir)
    {
        return None;
    }
    let title = intent.title_range?;
    let (start, end) = (
        intent.route_range.0.min(title.0),
        intent.route_range.1.max(title.1),
    );
    if start > end
        || end > input.len()
        || !input.is_char_boundary(start)
        || !input.is_char_boundary(end)
        || input[start..end].contains('\r')
        || input[start..end].contains('\n')
    {
        return None;
    }
    Some(DiagnosticRewrite {
        start,
        end,
        replacement: format!("@{target_dir}/{target_stem}"),
        label: "Add to it".to_owned(),
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RecordParseAction {
    Create,
    Update,
}

#[derive(Debug, Clone)]
pub(crate) struct RecordParseIssue {
    pub(crate) code: &'static str,
    pub(crate) message: String,
    pub(crate) range: Option<Range<usize>>,
}

/// Build the record route policy for one draft: lexical candidates
/// first, then config load, then `is_dir` stats for candidates only.
/// No candidates means no config or filesystem reads at all, so
/// ordinary task drafts pay nothing for record support.
pub(crate) fn record_policy_for_draft(
    vault_root: &Path,
    config_path: Option<&Path>,
    raw_text: &str,
) -> RecordRoutePolicy {
    let folder_candidates = candidate_dirs(raw_text);
    let mut candidates = folder_candidates.clone();
    for candidate in candidate_collection_routes(raw_text) {
        if !candidates.contains(&candidate) {
            candidates.push(candidate);
        }
    }
    if candidates.is_empty() {
        return RecordRoutePolicy::absent().with_vault_root(vault_root);
    }
    let path = config_path
        .map(Path::to_path_buf)
        .unwrap_or_else(config::config_path);
    let policy = match config::records::load_records_config_from(&path) {
        Ok(records) => RecordRoutePolicy::from_config(&records, Some(&path)),
        Err(diagnostic) => {
            RecordRoutePolicy::invalid(&path, diagnostic.message)
        }
    };
    let mut existing = HashSet::new();
    for dir in &folder_candidates {
        if policy.configured_dir(dir).is_some() {
            continue;
        }
        if vault_root.join(dir).is_dir() {
            existing.insert(dir.clone());
        }
    }
    policy.with_existing(existing).with_vault_root(vault_root)
}

/// Resolve claimed record items through the records engine. A single lazy
/// folder catalog is shared across this parse invocation, and earlier
/// planned paths reserve their stems for later items in the same draft.
/// Ordinary task drafts return before creating or reading a catalog.
pub(crate) fn resolve_record_intents(
    intents: &[(usize, RecordNoteIntent)],
    policy: Option<&RecordRoutePolicy>,
) -> Vec<RecordParseEntry> {
    if intents.is_empty() {
        return Vec::new();
    }
    let Some(policy) = policy else {
        return Vec::new();
    };
    let Some(vault_root) = policy.vault_root() else {
        return Vec::new();
    };

    let mut catalog = RecordCatalog::new(vault_root);
    resolve_record_intents_with_catalog(intents, Some(policy), &mut catalog)
}

/// Resolve record intents against a caller-owned catalog so a parse or
/// capture invocation can share the same lazy folder indexes.
pub(crate) fn resolve_record_intents_with_catalog(
    intents: &[(usize, RecordNoteIntent)],
    policy: Option<&RecordRoutePolicy>,
    catalog: &mut RecordCatalog,
) -> Vec<RecordParseEntry> {
    if intents.is_empty() {
        return Vec::new();
    }
    let Some(policy) = policy else {
        return Vec::new();
    };
    let Some(config) = policy.records_config() else {
        return Vec::new();
    };
    let Some(vault_root) = policy.vault_root() else {
        return Vec::new();
    };

    let now = super::env::current_datetime();
    let timezone = super::day_timezone().ok().flatten();
    let today = now.date();
    let mut reserved_paths = Vec::new();
    let mut overlay = HashMap::new();
    let mut entries = Vec::with_capacity(intents.len());

    for (index, intent) in intents {
        let Some(record_type) = config.by_directory(&intent.dir) else {
            continue;
        };
        let field_inputs = field_inputs_from_intent(intent);
        let notes = record_note_inputs(intent);
        let near_values =
            collect_near_value_hints(catalog, record_type, intent);
        match intent.action {
            super::capture_language::record::RecordAction::Create => {
                let request = records::RecordCreateRequest {
                    config,
                    record_type: &record_type.id,
                    title: &intent.title,
                    explicit_stem: intent.explicit_stem.as_deref(),
                    fields: &field_inputs,
                    notes: &notes,
                    now,
                    timezone,
                    vault_root,
                    reserved_paths: &reserved_paths,
                };
                match records::plan_create(request, catalog) {
                    Ok(plan) => {
                        reserved_paths.push(plan.path.clone());
                        if let Some(markdown) = plan.markdown.as_ref() {
                            overlay.insert(
                                overlay_key(&intent.dir, &plan.stem),
                                OverlaySnapshot {
                                    path: plan.path.clone(),
                                    stem: plan.stem.clone(),
                                    title: plan.title.clone(),
                                    markdown: markdown.clone(),
                                },
                            );
                        }
                        entries.push(RecordParseEntry {
                            index: *index,
                            intent: intent.clone(),
                            action: RecordParseAction::Create,
                            record_type: record_type.clone(),
                            title: Some(plan.title.clone()),
                            path: Some(plan.path),
                            stem: Some(plan.stem),
                            stem_source: Some(plan.stem_source),
                            properties: plan.properties,
                            update_properties: Vec::new(),
                            missing: plan.missing,
                            invalid: plan.invalid,
                            warnings: Vec::new(),
                            plan_issue: None,
                            near_values,
                            duplicate_target_stem: None,
                        });
                    }
                    Err(error) => {
                        let field_plan = records::plan_fields(
                            record_type,
                            &field_inputs,
                            today,
                        );
                        let (stem, stem_source, path) =
                            resolved_destination(intent, record_type);
                        let duplicate_target_stem = safe_duplicate_target_stem(
                            catalog,
                            record_type,
                            stem.as_deref(),
                            &error,
                            &overlay,
                        );
                        if error.code != RecordPlanErrorCode::PathCollision
                            && let Some(path) = path.as_ref()
                        {
                            reserved_paths.push(path.clone());
                        }
                        entries.push(RecordParseEntry {
                            index: *index,
                            intent: intent.clone(),
                            action: RecordParseAction::Create,
                            record_type: record_type.clone(),
                            title: Some(intent.title.clone()),
                            path,
                            stem,
                            stem_source,
                            properties: field_plan.properties,
                            update_properties: Vec::new(),
                            missing: field_plan.missing,
                            invalid: field_plan.invalid,
                            warnings: Vec::new(),
                            plan_issue: Some(plan_issue(
                                error,
                                intent,
                                record_type,
                            )),
                            near_values,
                            duplicate_target_stem,
                        });
                    }
                }
            }
            super::capture_language::record::RecordAction::Update => {
                let stem = intent.update_stem.as_deref().unwrap_or_default();
                let resolved = match resolve_update_snapshot(
                    config,
                    catalog,
                    record_type,
                    vault_root,
                    &overlay,
                    stem,
                ) {
                    Ok(resolved) => resolved,
                    Err(error) => {
                        entries.push(RecordParseEntry {
                            index: *index,
                            intent: intent.clone(),
                            action: RecordParseAction::Update,
                            record_type: record_type.clone(),
                            title: error.title,
                            path: error.path,
                            stem: Some(
                                error.stem.unwrap_or_else(|| stem.to_owned()),
                            ),
                            stem_source: Some(RecordStemSource::Existing),
                            properties: Vec::new(),
                            update_properties: Vec::new(),
                            missing: Vec::new(),
                            invalid: Vec::new(),
                            warnings: Vec::new(),
                            plan_issue: Some(RecordParseIssue {
                                code: error.code,
                                message: error.message,
                                range: intent
                                    .stem_range
                                    .map(|(start, end)| start..end),
                            }),
                            near_values,
                            duplicate_target_stem: None,
                        });
                        continue;
                    }
                };
                match records::plan_update(records::PlanUpdateRequest {
                    target: &resolved.target,
                    original: &resolved.original,
                    fields: &field_inputs,
                    notes: &notes,
                    today,
                }) {
                    Ok(plan) => {
                        if let Some(markdown) = plan.markdown.as_ref() {
                            overlay.insert(
                                overlay_key(&intent.dir, &plan.stem),
                                OverlaySnapshot {
                                    path: plan.path.clone(),
                                    stem: plan.stem.clone(),
                                    title: plan.title.clone(),
                                    markdown: markdown.clone(),
                                },
                            );
                        }
                        entries.push(RecordParseEntry {
                            index: *index,
                            intent: intent.clone(),
                            action: RecordParseAction::Update,
                            record_type: record_type.clone(),
                            title: Some(plan.title.clone()),
                            path: Some(plan.path),
                            stem: Some(plan.stem),
                            stem_source: Some(RecordStemSource::Existing),
                            properties: Vec::new(),
                            update_properties: plan.properties,
                            missing: plan.missing,
                            invalid: plan.invalid,
                            warnings: plan.warnings,
                            plan_issue: None,
                            near_values,
                            duplicate_target_stem: None,
                        });
                    }
                    Err(error) => {
                        let range = error
                            .field
                            .as_deref()
                            .and_then(|key| {
                                intent
                                    .fields
                                    .iter()
                                    .find(|field| field.key == key)
                                    .map(|field| field.value_range)
                            })
                            .or(intent.stem_range);
                        entries.push(RecordParseEntry {
                            index: *index,
                            intent: intent.clone(),
                            action: RecordParseAction::Update,
                            record_type: record_type.clone(),
                            title: Some(resolved.target.title),
                            path: Some(resolved.target.path),
                            stem: Some(resolved.target.stem),
                            stem_source: Some(RecordStemSource::Existing),
                            properties: Vec::new(),
                            update_properties: Vec::new(),
                            missing: Vec::new(),
                            invalid: Vec::new(),
                            warnings: Vec::new(),
                            plan_issue: Some(RecordParseIssue {
                                code: "record_invalid_value",
                                message: error.to_string(),
                                range: range.map(|(start, end)| start..end),
                            }),
                            near_values,
                            duplicate_target_stem: None,
                        });
                    }
                }
            }
        }
    }
    entries
}

#[derive(Clone)]
struct OverlaySnapshot {
    path: PathBuf,
    stem: String,
    title: String,
    markdown: String,
}

struct OverlayResolveError {
    code: &'static str,
    message: String,
    title: Option<String>,
    path: Option<PathBuf>,
    stem: Option<String>,
}

struct OverlayResolved {
    target: ResolvedUpdateTarget,
    original: String,
}

fn overlay_key(dir: &str, stem: &str) -> String {
    format!("{}/{}", dir.to_lowercase(), stem.to_lowercase())
}

fn safe_duplicate_target_stem(
    catalog: &mut RecordCatalog,
    record_type: &RecordType,
    stem: Option<&str>,
    error: &RecordPlanError,
    overlay: &HashMap<String, OverlaySnapshot>,
) -> Option<String> {
    if error.code != RecordPlanErrorCode::PathCollision {
        return None;
    }
    let [collision_path] = error.paths.as_slice() else {
        return None;
    };
    let stem = stem?;
    if let Some(snapshot) = overlay.get(&overlay_key(&record_type.dir, stem))
        && snapshot.path == *collision_path
    {
        return Some(snapshot.stem.clone());
    }
    match catalog.folder(&record_type.dir).lookup_stem(stem) {
        records::StemLookup::Exact(note)
            if note.relative_path == *collision_path =>
        {
            Some(note.stem.clone())
        }
        records::StemLookup::Exact(_)
        | records::StemLookup::Ambiguous(_)
        | records::StemLookup::None => None,
    }
}

fn resolve_update_snapshot(
    config: &records::RecordsConfig,
    catalog: &mut RecordCatalog,
    record_type: &RecordType,
    vault_root: &Path,
    overlay: &HashMap<String, OverlaySnapshot>,
    stem: &str,
) -> Result<OverlayResolved, OverlayResolveError> {
    if let Some(snapshot) = overlay.get(&overlay_key(&record_type.dir, stem)) {
        return Ok(OverlayResolved {
            target: ResolvedUpdateTarget {
                record_type: record_type.clone(),
                path: snapshot.path.clone(),
                stem: snapshot.stem.clone(),
                title: snapshot.title.clone(),
            },
            original: snapshot.markdown.clone(),
        });
    }
    let target = match records::resolve_update_target(
        config,
        catalog,
        &record_type.id,
        stem,
    ) {
        Ok(target) => target,
        Err(error) => {
            return Err(OverlayResolveError {
                code: error.code.diagnostic_code(),
                message: error.message,
                title: None,
                path: None,
                stem: Some(stem.to_owned()),
            });
        }
    };
    match std::fs::read_to_string(vault_root.join(&target.path)) {
        Ok(original) => Ok(OverlayResolved { target, original }),
        Err(error) => Err(OverlayResolveError {
            code: "record_invalid_value",
            message: format!(
                "cannot read existing record {}: {error}",
                target.path.display()
            ),
            title: Some(target.title),
            path: Some(target.path),
            stem: Some(target.stem),
        }),
    }
}

fn collect_near_value_hints(
    catalog: &mut RecordCatalog,
    record_type: &RecordType,
    intent: &RecordNoteIntent,
) -> Vec<records::NearValueHint> {
    let needs_index = record_type.fields.iter().any(|field| {
        field.suggest && matches!(field.kind, records::FieldKind::Text)
    });
    if !needs_index {
        return Vec::new();
    }
    let folder = catalog.folder(&intent.dir);
    let mut hints = Vec::new();
    for field in &intent.fields {
        let Some(schema) = record_type.field(&field.key) else {
            continue;
        };
        if !schema.suggest || !matches!(schema.kind, records::FieldKind::Text) {
            continue;
        }
        if field.value.is_empty() {
            continue;
        }
        if schema.list {
            for (member, range) in
                list_member_ranges(&field.value, field.value_range.0)
            {
                if let Some(hint) =
                    folder.near_value_hint(&field.key, &member, range)
                {
                    hints.push(hint);
                }
            }
        } else if let Some(hint) = folder.near_value_hint(
            &field.key,
            &field.value,
            field.value_range.0..field.value_range.1,
        ) {
            hints.push(hint);
        }
    }
    hints
}

fn list_member_ranges(
    value: &str,
    value_start: usize,
) -> Vec<(String, std::ops::Range<usize>)> {
    let Ok(members) = records::schema::split_list_members(value) else {
        return vec![(
            value.to_string(),
            value_start..value_start + value.len(),
        )];
    };
    let mut ranges = Vec::new();
    let mut search_from = 0;
    for member in members {
        let Some(relative) = value[search_from..].find(&member) else {
            continue;
        };
        let start = value_start + search_from + relative;
        ranges.push((member.clone(), start..start + member.len()));
        search_from += relative + member.len();
    }
    ranges
}

fn resolved_destination(
    intent: &RecordNoteIntent,
    record_type: &RecordType,
) -> (Option<String>, Option<RecordStemSource>, Option<PathBuf>) {
    let stem = if let Some(explicit) = intent.explicit_stem.as_deref() {
        records::stem::is_valid_explicit_stem(explicit)
            .then(|| explicit.to_owned())
    } else {
        records::stem::normalize_stem(&intent.title)
    };
    let source = stem.as_ref().map(|_| {
        if intent.explicit_stem.is_some() {
            RecordStemSource::Explicit
        } else {
            RecordStemSource::Title
        }
    });
    let path = stem
        .as_ref()
        .map(|stem| PathBuf::from(&record_type.dir).join(format!("{stem}.md")));
    (stem, source, path)
}

/// Convert parsed field bullets into engine inputs. Repeated list
/// bullets stay separate so `plan_create` can combine them.
pub(crate) fn field_inputs_from_intent(
    intent: &RecordNoteIntent,
) -> Vec<RecordFieldInput> {
    intent
        .fields
        .iter()
        .map(|field| RecordFieldInput {
            key: field.key.clone(),
            state: if field.value.is_empty() {
                FieldInputState::SuppliedEmpty
            } else {
                FieldInputState::SuppliedValues(vec![field.value.clone()])
            },
            source_range: Some(field.value_range.0..field.value_range.1),
        })
        .collect()
}

/// Convert grouped capture notes into engine note inputs. ALL-CAPS
/// sections become one section input with flattened children; ordinary
/// groups become one input per first-level line.
pub(crate) fn record_note_inputs(
    intent: &RecordNoteIntent,
) -> Vec<RecordNoteInput> {
    let mut inputs = Vec::new();
    if intent.action == super::capture_language::record::RecordAction::Update
        && !intent.title.is_empty()
    {
        inputs.push(RecordNoteInput {
            text: intent.title.clone(),
            children: Vec::new(),
        });
    }
    for group in &intent.notes {
        if let Some(section) = group.section.as_deref() {
            let mut children = Vec::new();
            for line in &group.lines {
                children.push(line.text.clone());
                children.extend(note_child_texts(&line.children));
            }
            inputs.push(RecordNoteInput::section(section, children));
        } else {
            for line in &group.lines {
                inputs.push(RecordNoteInput {
                    text: line.text.clone(),
                    children: note_child_texts(&line.children),
                });
            }
        }
    }
    inputs
}

/// Convert an engine value to its machine-readable JSON form. Parse output
/// and `capture` note_files share this so canonical values retain their
/// numeric, list, choice, date, and explicit-none representations.
pub(crate) fn record_value_json(
    value: &super::records::RecordValue,
) -> serde_json::Value {
    use super::records::{RecordDate, RecordScalar, RecordValue};

    fn scalar(value: &RecordScalar) -> serde_json::Value {
        match value {
            RecordScalar::Text(value) | RecordScalar::Choice(value) => {
                json!(value)
            }
            RecordScalar::Date(RecordDate::Resolved(date)) => {
                json!(date.format("%Y-%m-%d").to_string())
            }
            RecordScalar::Date(RecordDate::Today) => json!("today"),
            RecordScalar::Date(RecordDate::Yesterday) => json!("yesterday"),
            RecordScalar::Link(value) => json!(value),
            RecordScalar::Url(value) => json!(value.to_string()),
            RecordScalar::Number(value) => json!(value),
        }
    }

    match value {
        RecordValue::Scalar(value) => scalar(value),
        RecordValue::List(values) => {
            serde_json::Value::Array(values.iter().map(scalar).collect())
        }
    }
}

fn note_child_texts(
    lines: &[crate::native::capture_language::record::RecordNoteLine],
) -> Vec<String> {
    lines.iter().map(|line| line.text.clone()).collect()
}

/// Teaching copy for a path collision: name the existing file and the
/// recognized update spelling without enabling an update write.
pub(crate) fn collision_message(
    error: &RecordPlanError,
    intent: &RecordNoteIntent,
    record_type: &RecordType,
) -> String {
    let path = error
        .paths
        .first()
        .map(|path| path.display().to_string())
        .unwrap_or_else(|| "the requested path".to_string());
    let stem = error
        .paths
        .first()
        .and_then(|path| path.file_stem())
        .and_then(|stem| stem.to_str())
        .or(intent.explicit_stem.as_deref())
        .unwrap_or("<stem>");
    format!(
        "record already exists: {path} (open it; capture recognizes `@{}/{stem}` for updates, but update writes are not active yet)",
        record_type.dir,
    )
}

fn plan_issue(
    error: RecordPlanError,
    intent: &RecordNoteIntent,
    record_type: &RecordType,
) -> RecordParseIssue {
    let code = error.code.diagnostic_code();
    let message = match error.code {
        RecordPlanErrorCode::PathCollision => {
            collision_message(&error, intent, record_type)
        }
        _ => error.message,
    };
    let range = match error.code {
        RecordPlanErrorCode::PathCollision
        | RecordPlanErrorCode::ExplicitStemRequired => intent.title_range,
        RecordPlanErrorCode::EmptyTitle
        | RecordPlanErrorCode::FolderUnavailable => Some(intent.route_range),
        RecordPlanErrorCode::InvalidExplicitStem => intent.stem_range,
        RecordPlanErrorCode::UnknownRecordType => intent.route_range.into(),
    };
    RecordParseIssue {
        code,
        message,
        range: range.map(|(start, end)| start..end),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native::records::{FieldInputState, RecordCreateRequest};
    use chrono::NaiveDate;

    const CONFIG: &str = r#"records:
  restaurant:
    dir: eat
    label: Restaurant
    fields:
      status:
        required: true
        values: {liked: Liked}
"#;

    #[test]
    fn invocation_context_reuses_schema_and_folder_index() {
        let temp = tempfile::tempdir().expect("temporary vault");
        let root = temp.path();
        std::fs::create_dir_all(root.join("eat")).expect("record folder");
        std::fs::write(
            root.join("eat/existing.md"),
            "---\ntitle: Existing\nstatus: liked\n---\n# Existing\n",
        )
        .expect("existing record");
        let config_path = root.join("config.yml");
        std::fs::write(&config_path, CONFIG).expect("config");
        let config = crate::native::records::schema::parse_records_config(
            CONFIG,
            &config_path,
        )
        .expect("valid config");
        let policy =
            RecordRoutePolicy::from_config(&config, Some(&config_path))
                .with_vault_root(root);
        let mut context = CaptureRecordContext::new(policy, root);
        assert_eq!(context.catalog_files_opened(), 0);
        assert!(!context.catalog_is_loaded("eat"));

        let fields = vec![RecordFieldInput {
            key: "status".to_string(),
            state: FieldInputState::SuppliedValues(vec!["liked".to_string()]),
            source_range: None,
        }];
        let now = NaiveDate::from_ymd_opt(2026, 10, 7)
            .unwrap()
            .and_hms_opt(13, 40, 0)
            .unwrap();
        {
            let (config, catalog) = context.config_and_catalog().unwrap();
            let config_address = config as *const _;
            let catalog_address = catalog as *mut _;
            let first = records::plan_create(
                RecordCreateRequest {
                    config,
                    record_type: "restaurant",
                    title: "Ramen",
                    explicit_stem: None,
                    fields: &fields,
                    notes: &[],
                    now,
                    timezone: None,
                    vault_root: root,
                    reserved_paths: &[],
                },
                catalog,
            )
            .expect("first record plan");
            assert_eq!(first.path, PathBuf::from("eat/ramen.md"));
            assert_eq!(catalog.files_opened(), 1);
            assert!(catalog.is_loaded("eat"));

            let (same_config, same_catalog) =
                context.config_and_catalog().unwrap();
            assert_eq!(same_config as *const _, config_address);
            assert_eq!(same_catalog as *mut _, catalog_address);
        }
        {
            let (config, catalog) = context.config_and_catalog().unwrap();
            let second = records::plan_create(
                RecordCreateRequest {
                    config,
                    record_type: "restaurant",
                    title: "Udon",
                    explicit_stem: None,
                    fields: &fields,
                    notes: &[],
                    now,
                    timezone: None,
                    vault_root: root,
                    reserved_paths: &[],
                },
                catalog,
            )
            .expect("second record plan");
            assert_eq!(second.path, PathBuf::from("eat/udon.md"));
            assert_eq!(catalog.files_opened(), 1);
        }
        assert!(context.catalog_is_loaded("eat"));
        assert_eq!(context.catalog_files_opened(), 1);

        // An ordinary-task lexical preflight returns before config or index
        // reads, leaving its invocation context catalog completely lazy.
        let absent = record_policy_for_draft(
            root,
            Some(&root.join("missing-config.yml")),
            "Buy milk",
        );
        assert!(absent.records_config().is_none());
        let task_context = CaptureRecordContext::new(absent, root);
        assert!(!task_context.catalog_is_loaded("eat"));
        assert_eq!(task_context.catalog_files_opened(), 0);
    }

    #[test]
    fn create_to_update_rewrite_is_item_scoped_for_both_route_positions() {
        let cases = [
            (
                "@eat/^ninja_sushi Ninja Sushi\n- keep Café\n\nSibling",
                "@eat/^ninja_sushi",
                "Ninja Sushi",
                "ninja_sushi",
                "@eat/ninja_sushi\n- keep Café\n\nSibling",
            ),
            (
                "Café Nagi @eat/^nagi-shop\n- keep this bullet\n\nNext item",
                "@eat/^nagi-shop",
                "Café Nagi",
                "nagi-shop",
                "@eat/nagi-shop\n- keep this bullet\n\nNext item",
            ),
        ];
        for (input, route, title, target_stem, expected) in cases {
            let route_start = input.find(route).expect("route");
            let title_start = input.find(title).expect("title");
            let intent = RecordNoteIntent {
                action:
                    super::super::capture_language::record::RecordAction::Create,
                dir: "eat".to_owned(),
                title: title.to_owned(),
                explicit_stem: Some("new-stem".to_owned()),
                update_stem: None,
                fields: Vec::new(),
                notes: Vec::new(),
                route_range: (route_start, route_start + route.len()),
                title_range: Some((title_start, title_start + title.len())),
                stem_range: None,
            };
            let rewrite =
                create_to_update_rewrite(input, &intent, "eat", target_stem)
                    .expect("target rewrite");
            assert_eq!(rewrite.label, "Add to it");
            let result = format!(
                "{}{}{}",
                &input[..rewrite.start],
                rewrite.replacement,
                &input[rewrite.end..]
            );
            assert_eq!(result, expected);
        }
    }
}
