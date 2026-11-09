//! Native adapter between capture parsing and the records engine.
//!
//! Parse and execution build one immutable route policy through
//! [`record_policy_for_draft`]. Parse resolves claimed intents through the
//! existing planner and one invocation-local folder catalog; ordinary task
//! drafts avoid both config loading and record-index reads.

use std::{
    collections::HashSet,
    ops::Range,
    path::{Path, PathBuf},
};

use super::capture_language::{
    candidate_collection_routes, candidate_dirs, RecordRoutePolicy,
};
use super::{
    capture_language::record::RecordNoteIntent,
    config,
    records::{
        self, FieldInputState, PlannedRecordProperty, RecordCatalog,
        RecordDiagnostic, RecordFieldInput, RecordNoteInput, RecordPlanError,
        RecordPlanErrorCode, RecordStemSource, RecordType,
    },
};

/// A capture item's record metadata after the existing planner has resolved
/// its schema fields, defaults, stem, and destination.
#[derive(Debug, Clone)]
pub(crate) struct RecordParseEntry {
    pub(crate) index: usize,
    pub(crate) intent: RecordNoteIntent,
    pub(crate) record_type: RecordType,
    pub(crate) path: Option<PathBuf>,
    pub(crate) stem: Option<String>,
    pub(crate) stem_source: Option<RecordStemSource>,
    pub(crate) properties: Vec<PlannedRecordProperty>,
    pub(crate) missing: Vec<String>,
    pub(crate) invalid: Vec<RecordDiagnostic>,
    pub(crate) plan_issue: Option<RecordParseIssue>,
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
    let Some(config) = policy.records_config() else {
        return Vec::new();
    };
    let Some(vault_root) = policy.vault_root() else {
        return Vec::new();
    };

    let now = super::env::current_datetime();
    let timezone = super::day_timezone().ok().flatten();
    let today = now.date();
    let mut catalog = RecordCatalog::new(vault_root);
    let mut reserved_paths = Vec::new();
    let mut entries = Vec::with_capacity(intents.len());

    for (index, intent) in intents {
        let Some(record_type) = config.by_directory(&intent.dir) else {
            continue;
        };
        let field_inputs = intent
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
            .collect::<Vec<_>>();
        let no_notes: Vec<RecordNoteInput> = Vec::new();
        let request = records::RecordCreateRequest {
            config,
            record_type: &record_type.id,
            title: &intent.title,
            explicit_stem: intent.explicit_stem.as_deref(),
            fields: &field_inputs,
            notes: &no_notes,
            now,
            timezone,
            vault_root,
            reserved_paths: &reserved_paths,
        };
        match records::plan_create(request, &mut catalog) {
            Ok(plan) => {
                reserved_paths.push(plan.path.clone());
                entries.push(RecordParseEntry {
                    index: *index,
                    intent: intent.clone(),
                    record_type: record_type.clone(),
                    path: Some(plan.path),
                    stem: Some(plan.stem),
                    stem_source: Some(plan.stem_source),
                    properties: plan.properties,
                    missing: plan.missing,
                    invalid: plan.invalid,
                    plan_issue: None,
                });
            }
            Err(error) => {
                let field_plan =
                    records::plan_fields(record_type, &field_inputs, today);
                let (stem, stem_source, path) =
                    resolved_destination(intent, &record_type);
                if error.code != RecordPlanErrorCode::PathCollision
                    && let Some(path) = path.as_ref()
                {
                    reserved_paths.push(path.clone());
                }
                entries.push(RecordParseEntry {
                    index: *index,
                    intent: intent.clone(),
                    record_type: record_type.clone(),
                    path,
                    stem,
                    stem_source,
                    properties: field_plan.properties,
                    missing: field_plan.missing,
                    invalid: field_plan.invalid,
                    plan_issue: Some(plan_issue(error, intent, &record_type)),
                });
            }
        }
    }
    entries
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

fn plan_issue(
    error: RecordPlanError,
    intent: &RecordNoteIntent,
    record_type: &RecordType,
) -> RecordParseIssue {
    let code = error.code.diagnostic_code();
    let message = match error.code {
        RecordPlanErrorCode::PathCollision => {
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
                "record already exists: {path} (open it; adding to it with `@{}/{stem}` comes later)",
                record_type.dir,
            )
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
