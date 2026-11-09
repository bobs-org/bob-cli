//! Side-effect-free Record Note creation planning.
//!
//! Planning resolves the selected schema, field defaults and values, the
//! collision-checked path, and rendered Markdown. It may read the requested
//! folder through [`RecordCatalog`], but it never creates folders or writes
//! notes.

use std::{
    collections::{BTreeMap, HashSet},
    ops::Range,
    path::{Path, PathBuf},
};

use chrono::{
    Duration, LocalResult, NaiveDate, NaiveDateTime, Offset, TimeZone,
};
use chrono_tz::Tz;

use super::{
    catalog::{FolderIndex, RecordCatalog, StemLookup},
    render::{
        apply_record_update, render_record, RecordNoteInput,
        RecordRenderRequest, RecordUpdateRequest,
    },
    schema::{
        validate_field_input, FieldInputState, RecordDiagnostic,
        RecordDiagnosticCode, RecordField, RecordFieldInput, RecordScalar,
        RecordType, RecordValue, RecordsConfig, ValidatedFieldInput,
    },
    stem::{is_valid_explicit_stem, normalize_stem},
};

/// Inputs needed to plan one new Record Note. `now` is Bob's already-resolved
/// wall clock; `timezone` supplies its configured zone for the `created`
/// offset. `vault_root` and `catalog` must describe the same vault.
pub(crate) struct RecordCreateRequest<'a> {
    pub(crate) config: &'a RecordsConfig,
    pub(crate) record_type: &'a str,
    pub(crate) title: &'a str,
    pub(crate) explicit_stem: Option<&'a str>,
    pub(crate) fields: &'a [RecordFieldInput],
    pub(crate) notes: &'a [RecordNoteInput],
    pub(crate) now: NaiveDateTime,
    pub(crate) timezone: Option<Tz>,
    pub(crate) vault_root: &'a Path,
    /// Paths reserved by earlier items in the same caller-owned draft.
    pub(crate) reserved_paths: &'a [PathBuf],
}

/// How a property's value entered the plan.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RecordPropertySource {
    Typed,
    Default,
}

/// State of one property in a planned record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RecordPropertyState {
    Set,
    Default,
    Missing,
    Invalid,
    None,
}

impl RecordPropertyState {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Set => "set",
            Self::Default => "default",
            Self::Missing => "missing",
            Self::Invalid => "invalid",
            Self::None => "none",
        }
    }
}

/// Where the planned filename stem came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RecordStemSource {
    Title,
    Explicit,
}

/// One schema property as interpreted for this creation request.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct PlannedRecordProperty {
    pub(crate) key: String,
    pub(crate) label: String,
    pub(crate) kind: super::schema::FieldKind,
    pub(crate) required: bool,
    pub(crate) list: bool,
    pub(crate) state: RecordPropertyState,
    pub(crate) value: Option<RecordValue>,
    pub(crate) display: Option<String>,
    pub(crate) source: Option<RecordPropertySource>,
    pub(crate) source_ranges: Option<Vec<Range<usize>>>,
}

/// A complete or incomplete, collision-checked creation plan.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct RecordCreatePlan {
    pub(crate) record_type: String,
    pub(crate) title: String,
    pub(crate) stem: String,
    pub(crate) stem_source: RecordStemSource,
    /// Vault-relative path. The containing folder may not exist yet.
    pub(crate) path: PathBuf,
    pub(crate) properties: Vec<PlannedRecordProperty>,
    pub(crate) missing: Vec<String>,
    pub(crate) invalid: Vec<RecordDiagnostic>,
    pub(crate) catalog_diagnostics: Vec<RecordDiagnostic>,
    /// Present only when every required property is complete and every
    /// supplied value is valid.
    pub(crate) markdown: Option<String>,
    pub(crate) body: Option<String>,
}

/// Schema-ordered interpretation of the supplied fields, independent of a
/// title or destination path. Capture parsing uses this for incomplete title
/// drafts while sharing the planner's exact validation and defaults.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct RecordFieldPlan {
    pub(crate) properties: Vec<PlannedRecordProperty>,
    pub(crate) missing: Vec<String>,
    pub(crate) invalid: Vec<RecordDiagnostic>,
    values: BTreeMap<String, RecordValue>,
}

/// Structural errors that prevent a creation plan from being formed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RecordPlanErrorCode {
    UnknownRecordType,
    EmptyTitle,
    ExplicitStemRequired,
    InvalidExplicitStem,
    PathCollision,
    FolderUnavailable,
}

impl RecordPlanErrorCode {
    pub(crate) fn diagnostic_code(self) -> &'static str {
        match self {
            Self::EmptyTitle | Self::ExplicitStemRequired => "record_title",
            Self::PathCollision => "record_exists",
            Self::UnknownRecordType
            | Self::InvalidExplicitStem
            | Self::FolderUnavailable => "record_invalid_value",
        }
    }
}

/// A typed refusal with optional existing paths or did-you-mean choices.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RecordPlanError {
    pub(crate) code: RecordPlanErrorCode,
    pub(crate) message: String,
    pub(crate) paths: Vec<PathBuf>,
    pub(crate) suggestions: Vec<String>,
}

impl std::fmt::Display for RecordPlanError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for RecordPlanError {}

/// Build a new record plan without writing anything to the vault.
pub(crate) fn plan_create(
    request: RecordCreateRequest<'_>,
    catalog: &mut RecordCatalog,
) -> Result<RecordCreatePlan, RecordPlanError> {
    let Some(record_type) = request.config.record_type(request.record_type)
    else {
        let suggestions =
            nearest_record_types(request.config, request.record_type);
        let message = if suggestions.is_empty() {
            format!("unknown record type `{}`", request.record_type)
        } else {
            format!(
                "unknown record type `{}`; did you mean {}?",
                request.record_type,
                suggestions
                    .iter()
                    .map(|value| format!("`{value}`"))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        };
        return Err(plan_error(
            RecordPlanErrorCode::UnknownRecordType,
            message,
            Vec::new(),
            suggestions,
        ));
    };

    if request.title.trim().is_empty() {
        return Err(plan_error(
            RecordPlanErrorCode::EmptyTitle,
            "record title must not be empty".to_owned(),
            Vec::new(),
            Vec::new(),
        ));
    }

    let stem = match request.explicit_stem {
        Some(stem) if is_valid_explicit_stem(stem) => stem.to_owned(),
        Some(stem) => {
            return Err(plan_error(
                RecordPlanErrorCode::InvalidExplicitStem,
                format!(
                    "explicit record stem `{stem}` must use only A-Z, a-z, 0-9, or `-`"
                ),
                Vec::new(),
                Vec::new(),
            ));
        }
        None => normalize_stem(request.title).ok_or_else(|| {
            plan_error(
                RecordPlanErrorCode::ExplicitStemRequired,
                "record title has no filename characters; provide an explicit stem".to_owned(),
                Vec::new(),
                Vec::new(),
            )
        })?,
    };
    let stem_source = if request.explicit_stem.is_some() {
        RecordStemSource::Explicit
    } else {
        RecordStemSource::Title
    };

    let folder = catalog.folder(&record_type.dir);
    if let Some(reason) = folder.create_refusal() {
        return Err(plan_error(
            RecordPlanErrorCode::FolderUnavailable,
            format!(
                "cannot safely plan a record in {}: {reason}",
                record_type.dir
            ),
            Vec::new(),
            Vec::new(),
        ));
    }
    if let Some(error) =
        collision_error(folder, &record_type.dir, &stem, request.reserved_paths)
    {
        return Err(error);
    }

    let today = request.now.date();
    let field_plan = plan_fields(record_type, request.fields, today);
    let RecordFieldPlan {
        properties,
        missing,
        invalid,
        values,
    } = field_plan;

    let (markdown, body) = if missing.is_empty() && invalid.is_empty() {
        let parent = resolve_parent(&request, record_type);
        let created = format_created_timestamp(request.now, request.timezone);
        let rendered = render_record(&RecordRenderRequest {
            record_type,
            title: request.title,
            parent: parent.as_deref(),
            created: &created,
            values: &values,
            notes: request.notes,
            note_date: Some(&today.format("%Y-%m-%d").to_string()),
            today,
        });
        (Some(rendered.markdown), Some(rendered.body))
    } else {
        (None, None)
    };

    Ok(RecordCreatePlan {
        record_type: record_type.id.clone(),
        title: request.title.to_owned(),
        stem: stem.clone(),
        stem_source,
        path: PathBuf::from(&record_type.dir).join(format!("{stem}.md")),
        properties,
        missing,
        invalid,
        catalog_diagnostics: folder.diagnostics().to_vec(),
        markdown,
        body,
    })
}

/// Validate inputs and apply defaults using the same path as
/// [`plan_create`], without resolving a title, stem, collision, or Markdown.
pub(crate) fn plan_fields(
    record_type: &RecordType,
    inputs: &[RecordFieldInput],
    today: chrono::NaiveDate,
) -> RecordFieldPlan {
    let mut properties = Vec::with_capacity(record_type.fields.len());
    let mut missing = Vec::new();
    let mut invalid = Vec::new();
    let mut values = BTreeMap::new();

    for field in &record_type.fields {
        let ranges = source_ranges_for(inputs, &field.key);
        let property =
            match combine_field_inputs(field, inputs, &record_type.id) {
                Ok(state) => build_property(
                    field,
                    state,
                    today,
                    &record_type.id,
                    ranges.clone(),
                    &mut missing,
                    &mut invalid,
                ),
                Err(mut diagnostic) => {
                    diagnostic.source_ranges = ranges.clone();
                    invalid.push(diagnostic);
                    empty_property(field, RecordPropertyState::Invalid, ranges)
                }
            };
        if let Some(value) = &property.value {
            values.insert(field.key.clone(), value.clone());
        }
        properties.push(property);
    }

    let mut unknown_seen = HashSet::new();
    for input in inputs {
        if record_type.field(&input.key).is_none()
            && unknown_seen.insert(input.key.as_str())
        {
            let allowed_keys = record_type
                .fields
                .iter()
                .map(|field| field.key.clone())
                .collect::<Vec<_>>();
            let diagnostic = RecordDiagnostic {
                code: RecordDiagnosticCode::UnknownField,
                config_path: None,
                record_type: Some(record_type.id.clone()),
                field: Some(input.key.clone()),
                message: format!(
                    "`{}` is not a {} field; fields: {}",
                    input.key,
                    record_type.label,
                    record_type
                        .fields
                        .iter()
                        .map(|field| field.key.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
                suggestions: nearest_field_keys(&allowed_keys, &input.key),
                allowed_keys,
                source_ranges: source_ranges_for(inputs, &input.key),
            };
            invalid.push(diagnostic);
        }
    }

    RecordFieldPlan {
        properties,
        missing,
        invalid,
        values,
    }
}

fn combine_field_inputs(
    field: &RecordField,
    inputs: &[RecordFieldInput],
    record_type: &str,
) -> Result<FieldInputState, RecordDiagnostic> {
    let matching: Vec<&RecordFieldInput> = inputs
        .iter()
        .filter(|input| input.key == field.key)
        .collect();
    let has_empty = matching
        .iter()
        .any(|input| input.state == FieldInputState::SuppliedEmpty);
    let mut values = Vec::new();
    let mut has_values = false;
    for input in matching {
        if let FieldInputState::SuppliedValues(items) = &input.state {
            has_values = true;
            values.extend(items.iter().cloned());
        }
    }

    if has_empty && has_values {
        return Err(RecordDiagnostic {
            code: RecordDiagnosticCode::InvalidValue,
            config_path: None,
            record_type: Some(record_type.to_owned()),
            field: Some(field.key.clone()),
            message:
                "cannot combine an explicitly empty field with supplied values"
                    .to_owned(),
            suggestions: Vec::new(),
            allowed_keys: Vec::new(),
            source_ranges: None,
        });
    }
    if has_values {
        Ok(FieldInputState::SuppliedValues(values))
    } else if has_empty {
        Ok(FieldInputState::SuppliedEmpty)
    } else {
        Ok(FieldInputState::Absent)
    }
}

fn build_property(
    field: &RecordField,
    input: FieldInputState,
    today: chrono::NaiveDate,
    record_type: &str,
    source_ranges: Option<Vec<Range<usize>>>,
    missing: &mut Vec<String>,
    invalid: &mut Vec<RecordDiagnostic>,
) -> PlannedRecordProperty {
    let mut state = RecordPropertyState::None;
    let mut value = None;
    let mut source = None;

    match validate_field_input(field, &input, today) {
        Ok(ValidatedFieldInput::Absent) => {
            if let Some(default) = &field.default {
                if is_empty_list(default) {
                    if field.required {
                        missing.push(field.key.clone());
                        state = RecordPropertyState::Missing;
                    }
                } else {
                    match resolve_value_dates(default, today) {
                        Ok(resolved) => {
                            value = Some(resolved);
                            source = Some(RecordPropertySource::Default);
                            state = RecordPropertyState::Default;
                        }
                        Err(message) => {
                            let mut diagnostic = RecordDiagnostic::value(
                                field,
                                RecordDiagnosticCode::InvalidDate,
                                message,
                            );
                            diagnostic.record_type =
                                Some(record_type.to_owned());
                            invalid.push(diagnostic);
                            state = RecordPropertyState::Invalid;
                        }
                    }
                }
            } else if field.required {
                missing.push(field.key.clone());
                state = RecordPropertyState::Missing;
            }
        }
        Ok(ValidatedFieldInput::Empty) => {
            if field.required {
                missing.push(field.key.clone());
                state = RecordPropertyState::Missing;
            }
        }
        Ok(ValidatedFieldInput::Value(validated)) => {
            match resolve_value_dates(&validated, today) {
                Ok(resolved) => {
                    value = Some(resolved);
                    source = Some(RecordPropertySource::Typed);
                    state = RecordPropertyState::Set;
                }
                Err(message) => {
                    let mut diagnostic = RecordDiagnostic::value(
                        field,
                        RecordDiagnosticCode::InvalidDate,
                        message,
                    );
                    diagnostic.record_type = Some(record_type.to_owned());
                    diagnostic.source_ranges = source_ranges.clone();
                    invalid.push(diagnostic);
                    state = RecordPropertyState::Invalid;
                }
            }
        }
        Err(mut diagnostic) => {
            diagnostic.record_type = Some(record_type.to_owned());
            diagnostic.source_ranges = source_ranges.clone();
            invalid.push(diagnostic);
            state = RecordPropertyState::Invalid;
        }
    }

    let display = value
        .as_ref()
        .map(|value| display_value(field, value, today));
    PlannedRecordProperty {
        key: field.key.clone(),
        label: field.label.clone(),
        kind: field.kind.clone(),
        required: field.required,
        list: field.list,
        state,
        value,
        display,
        source,
        source_ranges,
    }
}

fn empty_property(
    field: &RecordField,
    state: RecordPropertyState,
    source_ranges: Option<Vec<Range<usize>>>,
) -> PlannedRecordProperty {
    PlannedRecordProperty {
        key: field.key.clone(),
        label: field.label.clone(),
        kind: field.kind.clone(),
        required: field.required,
        list: field.list,
        state,
        value: None,
        display: None,
        source: None,
        source_ranges,
    }
}

fn is_empty_list(value: &RecordValue) -> bool {
    matches!(value, RecordValue::List(values) if values.is_empty())
}

fn resolve_value_dates(
    value: &RecordValue,
    today: chrono::NaiveDate,
) -> Result<RecordValue, String> {
    fn resolve_scalar(
        scalar: &RecordScalar,
        today: chrono::NaiveDate,
    ) -> Result<RecordScalar, String> {
        match scalar {
            RecordScalar::Date(date) => date
                .resolve(today)
                .map(|resolved| {
                    RecordScalar::Date(super::schema::RecordDate::Resolved(
                        resolved,
                    ))
                })
                .ok_or_else(|| {
                    "relative date default cannot be resolved at this date"
                        .to_owned()
                }),
            other => Ok(other.clone()),
        }
    }

    match value {
        RecordValue::Scalar(scalar) => {
            resolve_scalar(scalar, today).map(RecordValue::Scalar)
        }
        RecordValue::List(values) => values
            .iter()
            .map(|scalar| resolve_scalar(scalar, today))
            .collect::<Result<Vec<_>, _>>()
            .map(RecordValue::List),
    }
}

fn source_ranges_for(
    inputs: &[RecordFieldInput],
    key: &str,
) -> Option<Vec<Range<usize>>> {
    let ranges: Vec<Range<usize>> = inputs
        .iter()
        .filter(|input| input.key == key)
        .filter_map(|input| input.source_range.clone())
        .collect();
    (!ranges.is_empty()).then_some(ranges)
}

fn nearest_field_keys(allowed: &[String], query: &str) -> Vec<String> {
    use crate::native::note_tasks::bounded_levenshtein;

    let query = query.to_lowercase();
    let mut candidates: Vec<(usize, &str)> = allowed
        .iter()
        .filter_map(|key| {
            bounded_levenshtein(&query, &key.to_lowercase(), 2)
                .map(|distance| (distance, key.as_str()))
        })
        .collect();
    candidates.sort_by(|left, right| {
        left.0.cmp(&right.0).then_with(|| left.1.cmp(right.1))
    });
    candidates
        .into_iter()
        .take(3)
        .map(|(_, key)| key.to_owned())
        .collect()
}

pub(crate) fn display_value(
    field: &RecordField,
    value: &RecordValue,
    today: chrono::NaiveDate,
) -> String {
    match value {
        RecordValue::Scalar(scalar) => display_scalar(field, scalar, today),
        RecordValue::List(values) => values
            .iter()
            .map(|scalar| display_scalar(field, scalar, today))
            .collect::<Vec<_>>()
            .join(", "),
    }
}

pub(crate) fn display_scalar(
    field: &RecordField,
    scalar: &RecordScalar,
    today: chrono::NaiveDate,
) -> String {
    match scalar {
        RecordScalar::Text(text) => text.clone(),
        RecordScalar::Choice(token) => match &field.kind {
            super::schema::FieldKind::Choices(choices) => choices
                .iter()
                .find(|choice| choice.value == *token)
                .map(|choice| choice.label.clone())
                .unwrap_or_else(|| token.clone()),
            _ => token.clone(),
        },
        RecordScalar::Date(date) => date
            .resolve(today)
            .map(|date| date.format("%Y-%m-%d").to_string())
            .unwrap_or_default(),
        RecordScalar::Link(link) => link
            .strip_prefix("[[")
            .and_then(|inner| inner.strip_suffix("]]"))
            .map(crate::native::capture_block_ids::wikilink_phrase)
            .unwrap_or_else(|| link.clone()),
        RecordScalar::Url(url) => url.to_string(),
        RecordScalar::Number(number) => number.to_string(),
    }
}

fn resolve_parent(
    request: &RecordCreateRequest<'_>,
    record_type: &RecordType,
) -> Option<String> {
    if let Some(parent) = &record_type.parent {
        return Some(parent.clone());
    }
    request
        .vault_root
        .join(format!("{}.md", record_type.dir))
        .is_file()
        .then(|| format!("[[{}]]", record_type.dir))
}

fn collision_error(
    folder: &FolderIndex,
    directory: &str,
    stem: &str,
    reserved_paths: &[PathBuf],
) -> Option<RecordPlanError> {
    let (paths, suggestions) = match folder.lookup_stem(stem) {
        StemLookup::Exact(note) => (
            vec![note.relative_path.clone()],
            vec![format!("@{directory}/{}", note.stem)],
        ),
        StemLookup::Ambiguous(notes) => (
            notes
                .iter()
                .map(|note| note.relative_path.clone())
                .collect(),
            notes
                .iter()
                .map(|note| format!("@{directory}/{}", note.stem))
                .collect(),
        ),
        StemLookup::None => {
            let path = folder.colliding_path(stem, reserved_paths)?;
            let suggestion = path
                .file_stem()
                .and_then(|value| value.to_str())
                .map(|value| format!("@{directory}/{value}"))
                .into_iter()
                .collect();
            (vec![path], suggestion)
        }
    };
    let path_list = paths
        .iter()
        .map(|path| path.display().to_string())
        .collect::<Vec<_>>()
        .join(", ");
    Some(plan_error(
        RecordPlanErrorCode::PathCollision,
        format!("record path already exists: {path_list}"),
        paths,
        suggestions,
    ))
}

fn nearest_record_types(config: &RecordsConfig, query: &str) -> Vec<String> {
    use crate::native::note_tasks::bounded_levenshtein;

    let query = query.to_lowercase();
    let mut candidates: Vec<(usize, &str)> = config
        .types
        .iter()
        .filter_map(|record_type| {
            bounded_levenshtein(&query, &record_type.id.to_lowercase(), 2)
                .map(|distance| (distance, record_type.id.as_str()))
        })
        .collect();
    candidates.sort_by(|left, right| {
        left.0.cmp(&right.0).then_with(|| left.1.cmp(right.1))
    });
    candidates
        .into_iter()
        .take(3)
        .map(|(_, id)| id.to_owned())
        .collect()
}

fn plan_error(
    code: RecordPlanErrorCode,
    message: String,
    paths: Vec<PathBuf>,
    suggestions: Vec<String>,
) -> RecordPlanError {
    RecordPlanError {
        code,
        message,
        paths,
        suggestions,
    }
}

fn format_created_timestamp(
    now: NaiveDateTime,
    timezone: Option<Tz>,
) -> String {
    const FORMAT: &str = "%Y-%m-%dT%H:%M:%S%z";
    let Some(timezone) = timezone else {
        return super::super::capture_project_note::format_created_timestamp(
            now,
        );
    };

    match timezone.from_local_datetime(&now) {
        LocalResult::Single(datetime) => datetime.format(FORMAT).to_string(),
        LocalResult::Ambiguous(earliest, _) => {
            earliest.format(FORMAT).to_string()
        }
        LocalResult::None => {
            let later =
                now.checked_add_signed(Duration::hours(1)).unwrap_or(now);
            let offset = match timezone.from_local_datetime(&later) {
                LocalResult::Single(datetime) => Some(datetime.offset().fix()),
                LocalResult::Ambiguous(earliest, _) => {
                    Some(earliest.offset().fix())
                }
                LocalResult::None => None,
            };
            offset
                .and_then(|offset| offset.from_local_datetime(&now).single())
                .map(|datetime| datetime.format(FORMAT).to_string())
                .unwrap_or_else(|| {
                    format!("{}+0000", now.format("%Y-%m-%dT%H:%M:%S"))
                })
        }
    }
}

/// An existing record resolved through the lazy folder catalog.
///
/// The catalog owns identity: `path` and `stem` always use the on-disk
/// spelling, never the caller's casing. The caller supplies the note snapshot
/// separately so current values come from the same bytes that will be edited.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ResolvedUpdateTarget {
    pub(crate) record_type: RecordType,
    pub(crate) path: PathBuf,
    pub(crate) stem: String,
    pub(crate) title: String,
}

/// Structural errors that prevent an update plan from being formed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RecordUpdateTargetErrorCode {
    UnknownRecordType,
    UnknownStem,
    AmbiguousStem,
    FolderUnavailable,
    UnsafeTarget,
}

impl RecordUpdateTargetErrorCode {
    pub(crate) fn diagnostic_code(self) -> &'static str {
        match self {
            Self::UnknownStem | Self::AmbiguousStem => "record_not_found",
            Self::UnknownRecordType
            | Self::FolderUnavailable
            | Self::UnsafeTarget => "record_invalid_value",
        }
    }
}

/// A typed refusal with optional existing paths or did-you-mean choices.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RecordUpdateTargetError {
    pub(crate) code: RecordUpdateTargetErrorCode,
    pub(crate) message: String,
    pub(crate) paths: Vec<PathBuf>,
    pub(crate) suggestions: Vec<String>,
}

impl std::fmt::Display for RecordUpdateTargetError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for RecordUpdateTargetError {}

/// Resolve a configured record type plus an existing stem to its on-disk
/// identity. Fuzzy matches are suggestions only and are never returned as the
/// target. Unsafe stems refuse without touching the filesystem beyond the
/// already-loaded folder index.
pub(crate) fn resolve_update_target(
    config: &RecordsConfig,
    catalog: &mut RecordCatalog,
    record_type: &str,
    stem: &str,
) -> Result<ResolvedUpdateTarget, RecordUpdateTargetError> {
    let Some(record_type) = config.record_type(record_type) else {
        let suggestions = nearest_record_types(config, record_type);
        let message = if suggestions.is_empty() {
            format!("unknown record type `{record_type}`")
        } else {
            format!(
                "unknown record type `{record_type}`; did you mean {}?",
                suggestions
                    .iter()
                    .map(|value| format!("`{value}`"))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        };
        return Err(RecordUpdateTargetError {
            code: RecordUpdateTargetErrorCode::UnknownRecordType,
            message,
            paths: Vec::new(),
            suggestions,
        });
    };
    if !is_safe_update_stem(stem) {
        return Err(RecordUpdateTargetError {
            code: RecordUpdateTargetErrorCode::UnsafeTarget,
            message: format!(
                "unsafe record stem {stem:?}; update targets must name one existing note"
            ),
            paths: Vec::new(),
            suggestions: Vec::new(),
        });
    }
    let folder = catalog.folder(&record_type.dir);
    if let Some(reason) = folder.create_refusal() {
        return Err(RecordUpdateTargetError {
            code: RecordUpdateTargetErrorCode::FolderUnavailable,
            message: format!(
                "cannot safely update a record in {}: {reason}",
                record_type.dir
            ),
            paths: Vec::new(),
            suggestions: Vec::new(),
        });
    }
    match folder.lookup_stem(stem) {
        StemLookup::Exact(note) => Ok(ResolvedUpdateTarget {
            record_type: record_type.clone(),
            path: note.relative_path.clone(),
            stem: note.stem.clone(),
            title: note.title.clone().unwrap_or_else(|| note.stem.clone()),
        }),
        StemLookup::Ambiguous(notes) => {
            let paths = notes
                .iter()
                .map(|note| note.relative_path.clone())
                .collect::<Vec<_>>();
            let suggestions = notes
                .iter()
                .map(|note| format!("@{}/{}", record_type.dir, note.stem))
                .collect::<Vec<_>>();
            let path_list = paths
                .iter()
                .map(|path| path.display().to_string())
                .collect::<Vec<_>>()
                .join(", ");
            Err(RecordUpdateTargetError {
                code: RecordUpdateTargetErrorCode::AmbiguousStem,
                message: format!(
                    "record stem {stem:?} is ambiguous: {path_list}"
                ),
                paths,
                suggestions,
            })
        }
        StemLookup::None => {
            let nearby = folder.nearest_stems(stem);
            let suggestions = nearby
                .iter()
                .map(|note| format!("@{}/{}", record_type.dir, note.stem))
                .collect::<Vec<_>>();
            let message = if suggestions.is_empty() {
                format!("record `{stem}` does not exist in {}", record_type.dir)
            } else {
                format!(
                    "record `{stem}` does not exist in {}; did you mean {}?",
                    record_type.dir,
                    suggestions
                        .iter()
                        .map(|value| format!("`{value}`"))
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            };
            Err(RecordUpdateTargetError {
                code: RecordUpdateTargetErrorCode::UnknownStem,
                message,
                paths: Vec::new(),
                suggestions,
            })
        }
    }
}

fn is_safe_update_stem(stem: &str) -> bool {
    if stem.is_empty()
        || stem.contains('\0')
        || stem.contains('\n')
        || stem.contains('\r')
        || stem.contains('/')
        || stem.contains('\\')
    {
        return false;
    }
    if stem == "." || stem == ".." {
        return false;
    }
    let mut components = std::path::Path::new(stem).components();
    matches!(components.next(), Some(std::path::Component::Normal(_)))
        && components.next().is_none()
}

/// Inputs needed to plan one existing-record update. `target` is the
/// catalog-resolved identity; `original` is the caller-owned full-text
/// snapshot that will be edited; `today` is Bob's already-resolved wall-clock
/// date for typed validation and dated notes.
pub(crate) struct PlanUpdateRequest<'a> {
    pub(crate) target: &'a ResolvedUpdateTarget,
    pub(crate) original: &'a str,
    pub(crate) fields: &'a [RecordFieldInput],
    pub(crate) notes: &'a [RecordNoteInput],
    pub(crate) today: NaiveDate,
}

/// One schema property with its snapshot current value and its planned
/// effective value. Absent fields mirror the snapshot; only supplied fields
/// can be missing or invalid. Historical defects stay in plan warnings.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct PlannedUpdateProperty {
    pub(crate) key: String,
    pub(crate) label: String,
    pub(crate) kind: super::schema::FieldKind,
    pub(crate) required: bool,
    pub(crate) list: bool,
    pub(crate) state: RecordPropertyState,
    pub(crate) value: Option<RecordValue>,
    pub(crate) display: Option<String>,
    pub(crate) current: Option<RecordValue>,
    pub(crate) current_display: Option<String>,
    pub(crate) source: Option<RecordPropertySource>,
    pub(crate) source_ranges: Option<Vec<Range<usize>>>,
}

/// A complete or incomplete side-effect-free update plan. `markdown` and
/// `body` are present only when every supplied field is valid; historical
/// snapshot defects are reported in `warnings` and never block planning.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct RecordUpdatePlan {
    pub(crate) record_type: String,
    pub(crate) title: String,
    pub(crate) stem: String,
    pub(crate) path: PathBuf,
    pub(crate) original: String,
    pub(crate) markdown: Option<String>,
    pub(crate) body: Option<String>,
    pub(crate) changed: Vec<String>,
    pub(crate) properties: Vec<PlannedUpdateProperty>,
    pub(crate) missing: Vec<String>,
    pub(crate) invalid: Vec<RecordDiagnostic>,
    pub(crate) warnings: Vec<RecordDiagnostic>,
}

/// Plan an existing-record update without writing anything.
///
/// Only supplied fields are validated and no defaults apply. Current values
/// are read from `request.original`, never from a possibly stale catalog
/// value. Edits run through the shared byte-preserving renderer.
pub(crate) fn plan_update(
    request: PlanUpdateRequest<'_>,
) -> Result<RecordUpdatePlan, super::render::RecordUpdateError> {
    let record_type = &request.target.record_type;
    let snapshot = parse_update_snapshot(request.original);
    let title = snapshot
        .title
        .clone()
        .unwrap_or_else(|| request.target.title.clone());

    let mut missing = Vec::new();
    let mut invalid = Vec::new();
    let mut effective: BTreeMap<String, RecordValue> = BTreeMap::new();
    let mut removed: Vec<String> = Vec::new();
    let mut invalid_keys: Vec<String> = Vec::new();
    let mut missing_keys: Vec<String> = Vec::new();

    for field in &record_type.fields {
        let ranges = source_ranges_for(request.fields, &field.key);
        match combine_field_inputs(field, request.fields, &record_type.id) {
            Ok(FieldInputState::Absent) => {}
            Ok(FieldInputState::SuppliedEmpty) => {
                if field.required {
                    missing.push(field.key.clone());
                    missing_keys.push(field.key.clone());
                } else {
                    removed.push(field.key.clone());
                }
                let _ = ranges;
            }
            Ok(FieldInputState::SuppliedValues(_)) => {
                let combined = combine_field_inputs(
                    field,
                    request.fields,
                    &record_type.id,
                )
                .expect("combined state is deterministic");
                match validate_field_input(field, &combined, request.today) {
                    Ok(ValidatedFieldInput::Value(validated)) => {
                        match resolve_value_dates(&validated, request.today) {
                            Ok(resolved) => {
                                effective.insert(field.key.clone(), resolved);
                            }
                            Err(message) => {
                                let mut diagnostic = RecordDiagnostic::value(
                                    field,
                                    RecordDiagnosticCode::InvalidDate,
                                    message,
                                );
                                diagnostic.record_type =
                                    Some(record_type.id.clone());
                                diagnostic.source_ranges = ranges.clone();
                                invalid.push(diagnostic);
                                invalid_keys.push(field.key.clone());
                            }
                        }
                    }
                    Ok(ValidatedFieldInput::Empty) => {
                        if field.required {
                            missing.push(field.key.clone());
                            missing_keys.push(field.key.clone());
                        } else {
                            removed.push(field.key.clone());
                        }
                    }
                    Ok(ValidatedFieldInput::Absent) => {}
                    Err(mut diagnostic) => {
                        diagnostic.record_type = Some(record_type.id.clone());
                        diagnostic.source_ranges = ranges.clone();
                        invalid.push(diagnostic);
                        invalid_keys.push(field.key.clone());
                    }
                }
            }
            Err(mut diagnostic) => {
                diagnostic.record_type = Some(record_type.id.clone());
                diagnostic.source_ranges = ranges;
                invalid.push(diagnostic);
                invalid_keys.push(field.key.clone());
            }
        }
    }

    let mut unknown_seen = HashSet::new();
    for input in request.fields {
        if record_type.field(&input.key).is_none()
            && unknown_seen.insert(input.key.as_str())
        {
            let allowed_keys = record_type
                .fields
                .iter()
                .map(|field| field.key.clone())
                .collect::<Vec<_>>();
            let diagnostic = RecordDiagnostic {
                code: RecordDiagnosticCode::UnknownField,
                config_path: None,
                record_type: Some(record_type.id.clone()),
                field: Some(input.key.clone()),
                message: format!(
                    "`{}` is not a {} field; fields: {}",
                    input.key,
                    record_type.label,
                    record_type
                        .fields
                        .iter()
                        .map(|field| field.key.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
                suggestions: nearest_field_keys(&allowed_keys, &input.key),
                allowed_keys,
                source_ranges: source_ranges_for(request.fields, &input.key),
            };
            invalid.push(diagnostic);
            invalid_keys.push(input.key.clone());
        }
    }

    let currents: BTreeMap<String, Option<RecordValue>> =
        snapshot_currents(record_type, &snapshot.mapping, request.today);

    let properties = update_properties(
        record_type,
        request.fields,
        &currents,
        &effective,
        &removed,
        &missing_keys,
        &invalid_keys,
        request.today,
    );

    if !missing.is_empty() || !invalid.is_empty() {
        return Ok(RecordUpdatePlan {
            record_type: record_type.id.clone(),
            title,
            stem: request.target.stem.clone(),
            path: request.target.path.clone(),
            original: request.original.to_owned(),
            markdown: None,
            body: None,
            changed: Vec::new(),
            properties,
            missing,
            invalid,
            warnings: Vec::new(),
        });
    }

    let note_date = record_type
        .dated_notes
        .then(|| request.today.format("%Y-%m-%d").to_string());
    let applied = apply_record_update(&RecordUpdateRequest {
        record_type,
        original: request.original,
        fields: request.fields,
        notes: request.notes,
        note_date: note_date.as_deref(),
        today: request.today,
    })?;
    let body =
        super::super::highlights_ref::split_frontmatter(&applied.markdown)
            .map(|(_, body)| body)
            .unwrap_or_default();

    Ok(RecordUpdatePlan {
        record_type: record_type.id.clone(),
        title,
        stem: request.target.stem.clone(),
        path: request.target.path.clone(),
        original: request.original.to_owned(),
        markdown: Some(applied.markdown),
        body: Some(body),
        changed: applied.changed,
        properties,
        missing,
        invalid,
        warnings: applied.warnings,
    })
}

#[derive(Debug, Clone, Default)]
struct UpdateSnapshot {
    mapping: Option<serde_yaml::Mapping>,
    title: Option<String>,
}

fn parse_update_snapshot(original: &str) -> UpdateSnapshot {
    let Some((front, _)) =
        super::super::highlights_ref::split_frontmatter(original)
    else {
        return UpdateSnapshot::default();
    };
    let joined = front.join("\n");
    let Ok(value) = serde_yaml::from_str::<serde_yaml::Value>(&joined) else {
        return UpdateSnapshot::default();
    };
    let Some(mapping) = value.as_mapping().cloned() else {
        return UpdateSnapshot::default();
    };
    let title = mapping
        .get(serde_yaml::Value::String("title".to_owned()))
        .and_then(|value| value.as_str())
        .filter(|text| !text.is_empty())
        .map(str::to_owned);
    UpdateSnapshot {
        mapping: Some(mapping),
        title,
    }
}

fn snapshot_currents(
    record_type: &RecordType,
    mapping: &Option<serde_yaml::Mapping>,
    today: NaiveDate,
) -> BTreeMap<String, Option<RecordValue>> {
    let mut currents = BTreeMap::new();
    let Some(mapping) = mapping else {
        for field in &record_type.fields {
            currents.insert(field.key.clone(), None);
        }
        return currents;
    };
    for field in &record_type.fields {
        let key = serde_yaml::Value::String(field.key.clone());
        let Some(raw) = mapping.get(&key) else {
            currents.insert(field.key.clone(), None);
            continue;
        };
        currents.insert(field.key.clone(), yaml_to_current(field, raw, today));
    }
    currents
}

fn yaml_to_current(
    field: &RecordField,
    raw: &serde_yaml::Value,
    today: NaiveDate,
) -> Option<RecordValue> {
    let inputs = yaml_to_inputs(raw)?;
    if inputs.is_empty() {
        return None;
    }
    match super::schema::validate_field_values(field, &inputs, today) {
        Ok(value) => Some(value),
        Err(_) => Some(fallback_current(field, &inputs)),
    }
}

fn yaml_to_inputs(raw: &serde_yaml::Value) -> Option<Vec<String>> {
    match raw {
        serde_yaml::Value::String(text) => Some(vec![text.clone()]),
        serde_yaml::Value::Bool(flag) => Some(vec![flag.to_string()]),
        serde_yaml::Value::Number(number) => Some(vec![number.to_string()]),
        serde_yaml::Value::Null => None,
        serde_yaml::Value::Sequence(items) => {
            let mut members = Vec::new();
            for item in items {
                match item {
                    serde_yaml::Value::String(text) => {
                        members.push(text.clone());
                    }
                    serde_yaml::Value::Bool(flag) => {
                        members.push(flag.to_string());
                    }
                    serde_yaml::Value::Number(number) => {
                        members.push(number.to_string());
                    }
                    serde_yaml::Value::Null => {
                        members.push(String::new());
                    }
                    _ => {}
                }
            }
            Some(members)
        }
        _ => None,
    }
}

fn fallback_current(field: &RecordField, inputs: &[String]) -> RecordValue {
    if field.list {
        RecordValue::List(
            inputs
                .iter()
                .map(|member| RecordScalar::Text(member.clone()))
                .collect(),
        )
    } else {
        RecordValue::Scalar(RecordScalar::Text(
            inputs.first().cloned().unwrap_or_default(),
        ))
    }
}

#[allow(clippy::too_many_arguments)]
fn update_properties(
    record_type: &RecordType,
    inputs: &[RecordFieldInput],
    currents: &BTreeMap<String, Option<RecordValue>>,
    effective: &BTreeMap<String, RecordValue>,
    removed: &[String],
    missing_keys: &[String],
    invalid_keys: &[String],
    today: NaiveDate,
) -> Vec<PlannedUpdateProperty> {
    record_type
        .fields
        .iter()
        .map(|field| {
            let source_ranges = source_ranges_for(inputs, &field.key);
            let current = currents.get(&field.key).cloned().flatten();
            let current_display = current
                .as_ref()
                .map(|value| display_value(field, value, today));
            if missing_keys.iter().any(|key| key == &field.key) {
                return PlannedUpdateProperty {
                    key: field.key.clone(),
                    label: field.label.clone(),
                    kind: field.kind.clone(),
                    required: field.required,
                    list: field.list,
                    state: RecordPropertyState::Missing,
                    value: None,
                    display: None,
                    current,
                    current_display,
                    source: None,
                    source_ranges,
                };
            }
            if invalid_keys.iter().any(|key| key == &field.key) {
                return PlannedUpdateProperty {
                    key: field.key.clone(),
                    label: field.label.clone(),
                    kind: field.kind.clone(),
                    required: field.required,
                    list: field.list,
                    state: RecordPropertyState::Invalid,
                    value: None,
                    display: None,
                    current,
                    current_display,
                    source: None,
                    source_ranges,
                };
            }
            if removed.iter().any(|key| key == &field.key) {
                return PlannedUpdateProperty {
                    key: field.key.clone(),
                    label: field.label.clone(),
                    kind: field.kind.clone(),
                    required: field.required,
                    list: field.list,
                    state: RecordPropertyState::None,
                    value: None,
                    display: None,
                    current,
                    current_display,
                    source: None,
                    source_ranges,
                };
            }
            if let Some(value) = effective.get(&field.key) {
                let display = Some(display_value(field, value, today));
                return PlannedUpdateProperty {
                    key: field.key.clone(),
                    label: field.label.clone(),
                    kind: field.kind.clone(),
                    required: field.required,
                    list: field.list,
                    state: RecordPropertyState::Set,
                    value: Some(value.clone()),
                    display,
                    current,
                    current_display,
                    source: Some(RecordPropertySource::Typed),
                    source_ranges,
                };
            }
            let (state, value, display) = match &current {
                Some(value) => (
                    RecordPropertyState::Set,
                    Some(value.clone()),
                    Some(display_value(field, value, today)),
                ),
                None => (RecordPropertyState::None, None, None),
            };
            PlannedUpdateProperty {
                key: field.key.clone(),
                label: field.label.clone(),
                kind: field.kind.clone(),
                required: field.required,
                list: field.list,
                state,
                value,
                display,
                current,
                current_display,
                source: None,
                source_ranges,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        ops::Range,
        path::{Path, PathBuf},
        time::{SystemTime, UNIX_EPOCH},
    };

    use chrono::{NaiveDate, NaiveDateTime};

    use super::{
        plan_create, plan_update, resolve_update_target, PlanUpdateRequest,
        RecordCreateRequest, RecordPlanErrorCode, RecordPropertySource,
        RecordPropertyState, RecordStemSource, RecordUpdateTargetErrorCode,
    };
    use crate::native::records::{
        catalog::RecordCatalog,
        render::RecordNoteInput,
        schema::{
            parse_records_config, FieldInputState, RecordDate,
            RecordDiagnosticCode, RecordFieldInput, RecordScalar, RecordValue,
            RecordsConfig,
        },
    };

    const CONFIG: &str = r#"
records:
  restaurant:
    dir: eat
    label: Restaurant
    emoji: 🍽️
    dated_notes: true
    fields:
      status:
        required: true
        values: {to_try: To try, liked: Liked}
      category: {required: true, suggest: true}
      location: {required: true, suggest: true}
      companions: {values: link, list: true, default: ["[[kelly]]"]}
      visited: {values: date}
      score: {values: number}
      active: {}
"#;

    fn config() -> RecordsConfig {
        parse_records_config(CONFIG, Path::new("/config.yml"))
            .expect("test records config is valid")
    }

    fn input(key: &str, state: FieldInputState) -> RecordFieldInput {
        RecordFieldInput {
            key: key.to_owned(),
            state,
            source_range: None,
        }
    }

    fn ranged_input(
        key: &str,
        state: FieldInputState,
        range: Range<usize>,
    ) -> RecordFieldInput {
        RecordFieldInput {
            key: key.to_owned(),
            state,
            source_range: Some(range),
        }
    }

    fn temp_dir(label: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "bob-record-plan-{label}-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir_all(&path).unwrap();
        path
    }

    fn plan<'a>(
        root: &'a Path,
        config: &'a RecordsConfig,
        fields: &'a [RecordFieldInput],
        reserved_paths: &'a [PathBuf],
    ) -> Result<super::RecordCreatePlan, super::RecordPlanError> {
        let mut catalog = RecordCatalog::new(root);
        plan_create(
            RecordCreateRequest {
                config,
                record_type: "restaurant",
                title: "Crème & Co.",
                explicit_stem: None,
                fields,
                notes: &[RecordNoteInput::bullet("Great lunch")],
                now: NaiveDate::from_ymd_opt(2026, 11, 8)
                    .unwrap()
                    .and_hms_opt(12, 41, 7)
                    .unwrap(),
                timezone: Some(chrono_tz::America::New_York),
                vault_root: root,
                reserved_paths,
            },
            &mut catalog,
        )
    }

    fn required_fields() -> Vec<RecordFieldInput> {
        vec![
            input(
                "status",
                FieldInputState::SuppliedValues(vec!["liked".to_owned()]),
            ),
            input(
                "category",
                FieldInputState::SuppliedValues(vec!["Coffee".to_owned()]),
            ),
            input(
                "location",
                FieldInputState::SuppliedValues(vec!["Cranford".to_owned()]),
            ),
        ]
    }

    #[test]
    fn complete_plan_applies_defaults_and_renders_a_relative_path() {
        let root = temp_dir("complete");
        fs::write(root.join("eat.md"), "# Eat\n").unwrap();
        let config = config();
        let fields = required_fields();
        let result = plan(&root, &config, &fields, &[]).unwrap();
        assert_eq!(result.path, PathBuf::from("eat/creme_and_co.md"));
        assert!(result.missing.is_empty());
        assert!(result.invalid.is_empty());
        assert_eq!(result.stem_source, RecordStemSource::Title);
        assert_eq!(result.properties[0].state, RecordPropertyState::Set);
        assert_eq!(
            result.properties[3].source,
            Some(RecordPropertySource::Default)
        );
        assert_eq!(result.properties[3].state, RecordPropertyState::Default);
        assert_eq!(result.properties[3].display.as_deref(), Some("kelly"));
        assert_eq!(result.properties[4].state, RecordPropertyState::None);
        assert_eq!(result.properties[4].state.as_str(), "none");
        let markdown = result.markdown.expect("complete plan renders bytes");
        assert!(markdown.contains("parent: \"[[eat]]\"\n"));
        assert!(markdown.contains("created: 2026-11-08T12:41:07-0500\n"));
        assert!(markdown.contains("- 2026-11-08: Great lunch"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn missing_and_empty_required_fields_do_not_reapply_defaults() {
        let root = temp_dir("missing");
        let config = config();
        let mut fields = required_fields();
        fields[2] = input("location", FieldInputState::SuppliedEmpty);
        let result = plan(&root, &config, &fields, &[]).unwrap();
        assert_eq!(result.missing, ["location"]);
        assert_eq!(result.properties[2].state, RecordPropertyState::Missing);
        assert_eq!(
            result.properties[3].source,
            Some(RecordPropertySource::Default)
        );
        assert!(result.markdown.is_none());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn explicit_empty_optional_suppresses_its_default() {
        let root = temp_dir("clear-default");
        let config = config();
        let mut fields = required_fields();
        fields.push(ranged_input(
            "companions",
            FieldInputState::SuppliedEmpty,
            20..35,
        ));
        let result = plan(&root, &config, &fields, &[]).unwrap();
        let companions = &result.properties[3];
        assert_eq!(companions.state, RecordPropertyState::None);
        assert_eq!(companions.value, None);
        assert_eq!(companions.source, None);
        assert_eq!(companions.source_ranges, Some(vec![20..35]));
        assert!(!result.markdown.unwrap().contains("companions:"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn repeated_list_values_preserve_order_and_scalar_repeats_are_invalid() {
        let root = temp_dir("repeated");
        let config = config();
        let mut fields = required_fields();
        fields.extend([
            input(
                "companions",
                FieldInputState::SuppliedValues(vec!["[[Kelly]]".to_owned()]),
            ),
            ranged_input(
                "companions",
                FieldInputState::SuppliedValues(vec!["Sam".to_owned()]),
                40..43,
            ),
            input(
                "score",
                FieldInputState::SuppliedValues(vec!["0".to_owned()]),
            ),
            input(
                "active",
                FieldInputState::SuppliedValues(vec!["false".to_owned()]),
            ),
        ]);
        let result = plan(&root, &config, &fields, &[]).unwrap();
        assert!(result.invalid.is_empty());
        assert_eq!(
            result.properties[3].value,
            Some(RecordValue::List(vec![
                RecordScalar::Link("[[Kelly]]".to_owned()),
                RecordScalar::Link("[[Sam]]".to_owned()),
            ]))
        );
        assert_eq!(result.properties[3].source_ranges, Some(vec![40..43]));
        assert_eq!(result.properties[5].display.as_deref(), Some("0"));
        assert_eq!(result.properties[6].display.as_deref(), Some("false"));
        assert!(result.markdown.unwrap().contains("score: 0\n"));

        let mut repeated_scalar = required_fields();
        repeated_scalar.extend([
            ranged_input(
                "score",
                FieldInputState::SuppliedValues(vec!["1".to_owned()]),
                50..51,
            ),
            ranged_input(
                "score",
                FieldInputState::SuppliedValues(vec!["2".to_owned()]),
                60..61,
            ),
        ]);
        let result = plan(&root, &config, &repeated_scalar, &[]).unwrap();
        assert_eq!(result.invalid.len(), 1);
        assert_eq!(result.invalid[0].field.as_deref(), Some("score"));
        assert_eq!(result.invalid[0].source_ranges, Some(vec![50..51, 60..61]));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn unknown_fields_are_typed_diagnostics_and_block_rendering() {
        let root = temp_dir("unknown-field");
        let config = config();
        let mut fields = required_fields();
        fields[2] = ranged_input(
            "location",
            FieldInputState::SuppliedValues(vec!["Cranford".to_owned()]),
            3..11,
        );
        fields.push(ranged_input(
            "catagory",
            FieldInputState::SuppliedValues(vec!["Coffee".to_owned()]),
            12..20,
        ));
        let result = plan(&root, &config, &fields, &[]).unwrap();
        assert_eq!(result.invalid.len(), 1);
        assert_eq!(
            result.invalid[0].code,
            crate::native::records::schema::RecordDiagnosticCode::UnknownField
        );
        assert!(result.invalid[0].message.contains("category"));
        assert_eq!(result.invalid[0].diagnostic_code(), "record_unknown_field");
        assert_eq!(result.invalid[0].suggestions, ["category"]);
        assert_eq!(
            result.invalid[0].allowed_keys,
            [
                "status",
                "category",
                "location",
                "companions",
                "visited",
                "score",
                "active"
            ]
        );
        assert_eq!(result.invalid[0].source_ranges, Some(vec![12..20]));
        assert_eq!(result.properties[2].source_ranges, Some(vec![3..11]));
        assert!(result.markdown.is_none());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn on_disk_casefold_and_same_draft_paths_refuse_collisions() {
        let root = temp_dir("collision");
        fs::create_dir_all(root.join("eat")).unwrap();
        fs::write(root.join("eat/CREME_AND_CO.md"), "bad frontmatter\n")
            .unwrap();
        let config = config();
        let error = plan(&root, &config, &required_fields(), &[]).unwrap_err();
        assert_eq!(error.code, RecordPlanErrorCode::PathCollision);
        assert_eq!(error.code.diagnostic_code(), "record_exists");
        assert_eq!(error.paths, [PathBuf::from("eat/CREME_AND_CO.md")]);
        assert_eq!(error.suggestions, ["@eat/CREME_AND_CO"]);

        fs::remove_file(root.join("eat/CREME_AND_CO.md")).unwrap();
        let error = plan(
            &root,
            &config,
            &required_fields(),
            &[PathBuf::from("eat/Creme_And_Co.md")],
        )
        .unwrap_err();
        assert_eq!(error.code, RecordPlanErrorCode::PathCollision);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn punctuation_titles_require_a_valid_explicit_stem() {
        let root = temp_dir("stem");
        let config = config();
        let fields = required_fields();
        let mut catalog = RecordCatalog::new(&root);
        let error = plan_create(
            RecordCreateRequest {
                config: &config,
                record_type: "restaurant",
                title: "!!!",
                explicit_stem: None,
                fields: &fields,
                notes: &[],
                now: NaiveDateTime::default(),
                timezone: None,
                vault_root: &root,
                reserved_paths: &[],
            },
            &mut catalog,
        )
        .unwrap_err();
        assert_eq!(error.code, RecordPlanErrorCode::ExplicitStemRequired);
        assert_eq!(error.code.diagnostic_code(), "record_title");

        let mut catalog = RecordCatalog::new(&root);
        let explicit = plan_create(
            RecordCreateRequest {
                config: &config,
                record_type: "restaurant",
                title: "!!!",
                explicit_stem: Some("manual-stem"),
                fields: &fields,
                notes: &[],
                now: NaiveDateTime::default(),
                timezone: None,
                vault_root: &root,
                reserved_paths: &[],
            },
            &mut catalog,
        )
        .unwrap();
        assert_eq!(explicit.stem, "manual-stem");
        assert_eq!(explicit.stem_source, RecordStemSource::Explicit);

        let mut catalog = RecordCatalog::new(&root);
        let empty_title = plan_create(
            RecordCreateRequest {
                config: &config,
                record_type: "restaurant",
                title: "  ",
                explicit_stem: Some("manual-stem"),
                fields: &fields,
                notes: &[],
                now: NaiveDateTime::default(),
                timezone: None,
                vault_root: &root,
                reserved_paths: &[],
            },
            &mut catalog,
        )
        .unwrap_err();
        assert_eq!(empty_title.code, RecordPlanErrorCode::EmptyTitle);
        assert_eq!(empty_title.code.diagnostic_code(), "record_title");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn an_unreadable_folder_never_yields_an_installable_plan() {
        let root = temp_dir("folder-refusal");
        fs::write(root.join("eat"), "not a directory").unwrap();
        let config = config();
        let error = plan(&root, &config, &required_fields(), &[]).unwrap_err();
        assert_eq!(error.code, RecordPlanErrorCode::FolderUnavailable);
        assert!(error.message.contains("not a directory"));
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn an_escaping_folder_symlink_never_yields_an_installable_plan() {
        use std::os::unix::fs::symlink;

        let root = temp_dir("escaping-folder");
        let outside = temp_dir("outside-folder");
        symlink(outside.join("."), root.join("eat")).unwrap();
        let config = config();
        let error = plan(&root, &config, &required_fields(), &[]).unwrap_err();
        assert_eq!(error.code, RecordPlanErrorCode::FolderUnavailable);
        assert!(error.message.contains("resolves outside the vault"));
        fs::remove_dir_all(root).unwrap();
        fs::remove_dir_all(outside).unwrap();
    }

    #[test]
    fn unknown_record_type_returns_nearby_type_suggestion() {
        let root = temp_dir("unknown-type");
        let config = config();
        let mut catalog = RecordCatalog::new(&root);
        let error = plan_create(
            RecordCreateRequest {
                config: &config,
                record_type: "restarant",
                title: "Lunch",
                explicit_stem: None,
                fields: &[],
                notes: &[],
                now: NaiveDateTime::default(),
                timezone: None,
                vault_root: &root,
                reserved_paths: &[],
            },
            &mut catalog,
        )
        .unwrap_err();
        assert_eq!(error.code, RecordPlanErrorCode::UnknownRecordType);
        assert_eq!(error.suggestions, ["restaurant"]);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn configured_timezone_controls_created_offset() {
        let now = NaiveDate::from_ymd_opt(2026, 1, 15)
            .unwrap()
            .and_hms_opt(12, 0, 0)
            .unwrap();
        assert_eq!(
            super::format_created_timestamp(now, Some(chrono_tz::UTC)),
            "2026-01-15T12:00:00+0000"
        );
        assert_eq!(
            super::format_created_timestamp(
                now,
                Some(chrono_tz::America::New_York)
            ),
            "2026-01-15T12:00:00-0500"
        );
    }

    #[test]
    fn dynamic_scalar_and_list_defaults_match_typed_dates() {
        let root = temp_dir("dynamic-dates");
        let config = parse_records_config(
            r#"
records:
  restaurant:
    dir: eat
    label: Restaurant
    fields:
      status: {required: true, values: {liked: Liked}}
      category: {required: true}
      location: {required: true}
      visited: {values: date, default: yesterday}
      dates: {values: date, list: true, default: [today, yesterday]}
"#,
            Path::new("/config.yml"),
        )
        .unwrap();
        let today = NaiveDate::from_ymd_opt(2026, 11, 8).unwrap();
        let now = today.and_hms_opt(12, 0, 0).unwrap();
        let fields = required_fields();
        let mut default_catalog = RecordCatalog::new(&root);
        let defaults = plan_create(
            RecordCreateRequest {
                config: &config,
                record_type: "restaurant",
                title: "Date test",
                explicit_stem: None,
                fields: &fields,
                notes: &[],
                now,
                timezone: Some(chrono_tz::UTC),
                vault_root: &root,
                reserved_paths: &[],
            },
            &mut default_catalog,
        )
        .unwrap();

        let mut typed_fields = fields.clone();
        typed_fields.extend([
            input(
                "visited",
                FieldInputState::SuppliedValues(vec!["yesterday".to_owned()]),
            ),
            input(
                "dates",
                FieldInputState::SuppliedValues(vec![
                    "today, yesterday".to_owned()
                ]),
            ),
        ]);
        let mut typed_catalog = RecordCatalog::new(&root);
        let typed = plan_create(
            RecordCreateRequest {
                config: &config,
                record_type: "restaurant",
                title: "Date test",
                explicit_stem: None,
                fields: &typed_fields,
                notes: &[],
                now,
                timezone: Some(chrono_tz::UTC),
                vault_root: &root,
                reserved_paths: &[],
            },
            &mut typed_catalog,
        )
        .unwrap();

        let expected_scalar = RecordValue::Scalar(RecordScalar::Date(
            RecordDate::Resolved(NaiveDate::from_ymd_opt(2026, 11, 7).unwrap()),
        ));
        let expected_list = RecordValue::List(vec![
            RecordScalar::Date(RecordDate::Resolved(today)),
            RecordScalar::Date(RecordDate::Resolved(
                NaiveDate::from_ymd_opt(2026, 11, 7).unwrap(),
            )),
        ]);
        assert_eq!(defaults.properties[3].state, RecordPropertyState::Default);
        assert_eq!(defaults.properties[3].value, Some(expected_scalar.clone()));
        assert_eq!(defaults.properties[4].value, Some(expected_list.clone()));
        assert_eq!(typed.properties[3].state, RecordPropertyState::Set);
        assert_eq!(typed.properties[3].value, Some(expected_scalar));
        assert_eq!(typed.properties[4].value, Some(expected_list));
        assert_eq!(defaults.properties[3].display, typed.properties[3].display);
        assert_eq!(defaults.properties[4].display, typed.properties[4].display);
        assert_eq!(defaults.markdown, typed.markdown);
        assert!(defaults.markdown.unwrap().contains(
            "visited: 2026-11-07\ndates:\n  - 2026-11-08\n  - 2026-11-07\n"
        ));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn unresolved_date_defaults_are_invalid_instead_of_becoming_today() {
        let root = temp_dir("unresolved-date");
        let config = parse_records_config(
            "records:\n  restaurant:\n    dir: eat\n    label: Restaurant\n    fields:\n      status: {required: true, values: {liked: Liked}}\n      category: {required: true}\n      location: {required: true}\n      visited: {values: date, default: yesterday}\n",
            Path::new("/config.yml"),
        )
        .unwrap();
        let mut catalog = RecordCatalog::new(&root);
        let fields = required_fields();
        let now = NaiveDate::MIN.and_hms_opt(0, 0, 0).unwrap();
        let result = plan_create(
            RecordCreateRequest {
                config: &config,
                record_type: "restaurant",
                title: "Date test",
                explicit_stem: None,
                fields: &fields,
                notes: &[],
                now,
                timezone: Some(chrono_tz::UTC),
                vault_root: &root,
                reserved_paths: &[],
            },
            &mut catalog,
        )
        .unwrap();
        assert_eq!(result.properties[3].state, RecordPropertyState::Invalid);
        assert_eq!(result.invalid.len(), 1);
        assert_eq!(result.invalid[0].code, RecordDiagnosticCode::InvalidDate);
        assert_eq!(result.invalid[0].diagnostic_code(), "record_invalid_value");
        assert!(result.markdown.is_none());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn an_empty_optional_list_default_is_none_and_not_a_required_value() {
        let root = temp_dir("empty-list-default");
        let config = parse_records_config(
            "records:\n  restaurant:\n    dir: eat\n    label: Restaurant\n    fields:\n      status: {required: true, values: {liked: Liked}}\n      category: {required: true}\n      location: {required: true}\n      optional_tags: {list: true, default: []}\n      required_tags: {list: true, required: true, default: []}\n",
            Path::new("/config.yml"),
        )
        .unwrap();
        let mut catalog = RecordCatalog::new(&root);
        let result = plan_create(
            RecordCreateRequest {
                config: &config,
                record_type: "restaurant",
                title: "List test",
                explicit_stem: None,
                fields: &required_fields(),
                notes: &[],
                now: NaiveDateTime::default(),
                timezone: None,
                vault_root: &root,
                reserved_paths: &[],
            },
            &mut catalog,
        )
        .unwrap();
        assert_eq!(result.properties[3].state, RecordPropertyState::None);
        assert_eq!(result.properties[3].value, None);
        assert_eq!(result.properties[4].state, RecordPropertyState::Missing);
        assert_eq!(result.missing, ["required_tags"]);
        assert!(result.markdown.is_none());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn configured_zone_and_bob_now_drive_the_real_planning_path() {
        let root = temp_dir("bob-now-zone");
        let config_path = root.join("config.yml");
        fs::write(&config_path, "timezone: Pacific/Kiritimati\n").unwrap();
        crate::native::env::pin_tz_utc0_for_test();
        let result = crate::native::env::with_var(
            "BOB_CONFIG_FILE",
            config_path.as_os_str(),
            || {
                crate::native::env::with_var(
                    "BOB_NOW",
                    "2026-01-15 23:30:00",
                    || {
                        let now = crate::native::env::current_datetime();
                        let timezone =
                            crate::native::env::configured_timezone();
                        assert_eq!(now.to_string(), "2026-01-15 23:30:00");
                        assert_eq!(
                            timezone,
                            Some(chrono_tz::Pacific::Kiritimati)
                        );
                        let config = config();
                        let mut catalog = RecordCatalog::new(&root);
                        plan_create(
                            RecordCreateRequest {
                                config: &config,
                                record_type: "restaurant",
                                title: "Zone test",
                                explicit_stem: None,
                                fields: &required_fields(),
                                notes: &[],
                                now,
                                timezone,
                                vault_root: &root,
                                reserved_paths: &[],
                            },
                            &mut catalog,
                        )
                        .unwrap()
                    },
                )
            },
        );
        assert!(result
            .markdown
            .unwrap()
            .contains("created: 2026-01-15T23:30:00+1400\n"));
        fs::remove_dir_all(root).unwrap();
    }

    const UPDATE_TODAY: NaiveDate = match NaiveDate::from_ymd_opt(2026, 11, 8) {
        Some(date) => date,
        None => panic!("test date is valid"),
    };

    fn update_values(key: &str, values: &[&str]) -> RecordFieldInput {
        RecordFieldInput {
            key: key.to_owned(),
            state: FieldInputState::SuppliedValues(
                values.iter().map(|value| (*value).to_owned()).collect(),
            ),
            source_range: None,
        }
    }

    fn update_empty(key: &str) -> RecordFieldInput {
        RecordFieldInput {
            key: key.to_owned(),
            state: FieldInputState::SuppliedEmpty,
            source_range: None,
        }
    }

    fn write_update_note(root: &Path, relative: &str, contents: &str) {
        let path = root.join(relative);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, contents).unwrap();
    }

    #[test]
    fn update_resolves_unique_stem_case_insensitively_with_actual_spelling() {
        let root = temp_dir("update-resolve");
        write_update_note(
            &root,
            "eat/Ninja_Sushi.md",
            "---\ntitle: Ninja Sushi\nstatus: liked\ncategory: Coffee\nlocation: Clark\ncreated: 2026-11-08T12:00:00+0000\n---\n\n# Ninja Sushi\n",
        );
        let config = config();
        let mut catalog = RecordCatalog::new(&root);
        let target = resolve_update_target(
            &config,
            &mut catalog,
            "restaurant",
            "ninja_sushi",
        )
        .unwrap();
        assert_eq!(target.stem, "Ninja_Sushi");
        assert_eq!(target.path, PathBuf::from("eat/Ninja_Sushi.md"));
        assert_eq!(target.title, "Ninja Sushi");
        let mut catalog = RecordCatalog::new(&root);
        let upper = resolve_update_target(
            &config,
            &mut catalog,
            "restaurant",
            "NINJA_SUSHI",
        )
        .unwrap();
        assert_eq!(upper, target);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn update_missing_stem_reports_not_found_with_nearby_suggestions() {
        let root = temp_dir("update-missing");
        write_update_note(
            &root,
            "eat/ninja_sushi.md",
            "---\ntitle: Ninja Sushi\nstatus: liked\ncategory: Coffee\nlocation: Clark\ncreated: 2026-11-08T12:00:00+0000\n---\n\n# Ninja Sushi\n",
        );
        let config = config();
        let mut catalog = RecordCatalog::new(&root);
        let error = resolve_update_target(
            &config,
            &mut catalog,
            "restaurant",
            "ninja_sushy",
        )
        .unwrap_err();
        assert_eq!(error.code, RecordUpdateTargetErrorCode::UnknownStem);
        assert_eq!(error.code.diagnostic_code(), "record_not_found");
        assert!(
            error.suggestions.contains(&"@eat/ninja_sushi".to_owned()),
            "suggestions: {:?}",
            error.suggestions
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn update_ambiguous_stem_lists_candidates_as_not_found() {
        let root = temp_dir("update-ambiguous");
        let note = |title: &str| {
            format!(
                "---\ntitle: {title}\nstatus: liked\ncategory: Coffee\nlocation: Clark\ncreated: 2026-11-08T12:00:00+0000\n---\n\n# {title}\n"
            )
        };
        write_update_note(&root, "eat/Alpha.md", &note("Alpha"));
        write_update_note(&root, "eat/ALPHA.md", &note("ALPHA"));
        let entries = fs::read_dir(root.join("eat")).unwrap().count();
        let config = config();
        if entries < 2 {
            let mut catalog = RecordCatalog::new(&root);
            let target = resolve_update_target(
                &config,
                &mut catalog,
                "restaurant",
                "alpha",
            )
            .unwrap();
            assert!(
                target.path.as_path() == Path::new("eat/Alpha.md")
                    || target.path.as_path() == Path::new("eat/ALPHA.md")
            );
            fs::remove_dir_all(root).unwrap();
            return;
        }
        let mut catalog = RecordCatalog::new(&root);
        let error =
            resolve_update_target(&config, &mut catalog, "restaurant", "alpha")
                .unwrap_err();
        assert_eq!(error.code, RecordUpdateTargetErrorCode::AmbiguousStem);
        assert_eq!(error.code.diagnostic_code(), "record_not_found");
        assert_eq!(error.paths.len(), 2);
        assert_eq!(error.suggestions.len(), 2);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn update_unsafe_stems_refuse_before_folder_access() {
        let root = temp_dir("update-unsafe");
        let config = config();
        for stem in ["../escape", "a/b", "", ".", "..", "a\\b"] {
            let mut catalog = RecordCatalog::new(&root);
            let error = resolve_update_target(
                &config,
                &mut catalog,
                "restaurant",
                stem,
            )
            .unwrap_err();
            assert_eq!(
                error.code,
                RecordUpdateTargetErrorCode::UnsafeTarget,
                "stem {stem:?}"
            );
            assert_eq!(error.code.diagnostic_code(), "record_invalid_value");
        }
        let mut catalog = RecordCatalog::new(&root);
        let error = resolve_update_target(&config, &mut catalog, "nope", "x")
            .unwrap_err();
        assert_eq!(error.code, RecordUpdateTargetErrorCode::UnknownRecordType);
        fs::write(root.join("eat"), "not a directory").unwrap();
        let mut catalog = RecordCatalog::new(&root);
        let error =
            resolve_update_target(&config, &mut catalog, "restaurant", "x")
                .unwrap_err();
        assert_eq!(error.code, RecordUpdateTargetErrorCode::FolderUnavailable);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn update_absent_fields_never_apply_creation_defaults() {
        let root = temp_dir("update-no-defaults");
        let original = "---\ntitle: No Default\nstatus: liked\ncategory: Coffee\nlocation: Clark\ncreated: 2026-11-08T12:00:00+0000\n---\n\n# No Default\n\n## Notes\n";
        write_update_note(&root, "eat/no_default.md", original);
        let config = config();
        let mut catalog = RecordCatalog::new(&root);
        let target = resolve_update_target(
            &config,
            &mut catalog,
            "restaurant",
            "no_default",
        )
        .unwrap();
        let plan = plan_update(PlanUpdateRequest {
            target: &target,
            original,
            fields: &[],
            notes: &[],
            today: UPDATE_TODAY,
        })
        .unwrap();
        assert!(plan.missing.is_empty());
        assert!(plan.invalid.is_empty());
        assert_eq!(plan.markdown.as_deref(), Some(original));
        assert!(plan.changed.is_empty());
        let companions = plan
            .properties
            .iter()
            .find(|property| property.key == "companions")
            .expect("companions property");
        assert_eq!(companions.state, RecordPropertyState::None);
        assert_eq!(companions.value, None);
        assert_eq!(companions.source, None);
        assert!(plan
            .properties
            .iter()
            .all(|property| property.source
                != Some(RecordPropertySource::Default)));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn update_optional_empty_removes_key_and_required_empty_is_missing() {
        let root = temp_dir("update-empty");
        let original = "---\ntitle: Empty\nstatus: liked\ncategory: Coffee\nlocation: Clark\ncompanions:\n  - \"[[kelly]]\"\ncreated: 2026-11-08T12:00:00+0000\n---\n\n# Empty\n\n## Notes\n";
        write_update_note(&root, "eat/empty.md", original);
        let config = config();
        let mut catalog = RecordCatalog::new(&root);
        let target =
            resolve_update_target(&config, &mut catalog, "restaurant", "empty")
                .unwrap();
        let plan = plan_update(PlanUpdateRequest {
            target: &target,
            original,
            fields: &[update_empty("companions")],
            notes: &[],
            today: UPDATE_TODAY,
        })
        .unwrap();
        assert!(plan.missing.is_empty());
        assert!(plan.invalid.is_empty());
        let markdown = plan.markdown.expect("removal completes");
        assert!(!markdown.contains("companions"));
        assert_eq!(plan.changed, vec!["companions".to_owned()]);
        let missing = plan_update(PlanUpdateRequest {
            target: &target,
            original,
            fields: &[update_empty("status")],
            notes: &[],
            today: UPDATE_TODAY,
        })
        .unwrap();
        assert_eq!(missing.missing, vec!["status".to_owned()]);
        assert!(missing.markdown.is_none());
        assert_eq!(
            missing
                .properties
                .iter()
                .find(|property| property.key == "status")
                .expect("status property")
                .state,
            RecordPropertyState::Missing
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn update_replaces_whole_lists_in_request_order() {
        let root = temp_dir("update-list");
        let original = "---\ntitle: Lists\nstatus: liked\ncategory: Coffee\nlocation: Clark\ncompanions:\n  - \"[[kelly]]\"\ncreated: 2026-11-08T12:00:00+0000\n---\n\n# Lists\n\n## Notes\n";
        write_update_note(&root, "eat/lists.md", original);
        let config = config();
        let mut catalog = RecordCatalog::new(&root);
        let target =
            resolve_update_target(&config, &mut catalog, "restaurant", "lists")
                .unwrap();
        let fields = vec![
            update_values("companions", &["[[bob]]"]),
            update_values("companions", &["[[alice]]"]),
        ];
        let plan = plan_update(PlanUpdateRequest {
            target: &target,
            original,
            fields: &fields,
            notes: &[],
            today: UPDATE_TODAY,
        })
        .unwrap();
        let markdown = plan.markdown.expect("list replace completes");
        let bob_at = markdown.find("[[bob]]").expect("bob");
        let alice_at = markdown.find("[[alice]]").expect("alice");
        assert!(bob_at < alice_at);
        assert!(!markdown.contains("[[kelly]]"));
        assert_eq!(plan.changed, vec!["companions".to_owned()]);
        assert_eq!(
            plan.properties
                .iter()
                .find(|property| property.key == "companions")
                .expect("companions property")
                .value,
            Some(RecordValue::List(vec![
                RecordScalar::Link("[[bob]]".to_owned()),
                RecordScalar::Link("[[alice]]".to_owned()),
            ]))
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn update_supplied_invalid_values_are_invalid_not_warnings() {
        let root = temp_dir("update-invalid");
        let original = "---\ntitle: Invalid\nstatus: liked\ncategory: Coffee\nlocation: Clark\ncreated: 2026-11-08T12:00:00+0000\n---\n\n# Invalid\n\n## Notes\n";
        write_update_note(&root, "eat/invalid.md", original);
        let config = config();
        let mut catalog = RecordCatalog::new(&root);
        let target = resolve_update_target(
            &config,
            &mut catalog,
            "restaurant",
            "invalid",
        )
        .unwrap();
        let plan = plan_update(PlanUpdateRequest {
            target: &target,
            original,
            fields: &[update_values("status", &["bogus"])],
            notes: &[],
            today: UPDATE_TODAY,
        })
        .unwrap();
        assert!(plan.markdown.is_none());
        assert!(plan.missing.is_empty());
        assert_eq!(plan.invalid.len(), 1);
        assert!(plan.warnings.is_empty());
        assert_eq!(
            plan.properties
                .iter()
                .find(|property| property.key == "status")
                .expect("status property")
                .state,
            RecordPropertyState::Invalid
        );
        let unknown = plan_update(PlanUpdateRequest {
            target: &target,
            original,
            fields: &[update_values("nope", &["x"])],
            notes: &[],
            today: UPDATE_TODAY,
        })
        .unwrap();
        assert!(unknown.markdown.is_none());
        assert!(unknown
            .invalid
            .iter()
            .any(|diagnostic| diagnostic.code
                == RecordDiagnosticCode::UnknownField));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn update_historical_defects_are_warnings_with_snapshot_currents() {
        let root = temp_dir("update-warnings");
        let original = "---\ntitle: Warnings\nstatus: bogus\ncategory: Coffee\nlocation: Clark\ncreated: 2026-11-08T12:00:00+0000\n---\n\n# Warnings\n\n## Notes\n";
        write_update_note(&root, "eat/warnings.md", original);
        let config = config();
        let mut catalog = RecordCatalog::new(&root);
        let target = resolve_update_target(
            &config,
            &mut catalog,
            "restaurant",
            "warnings",
        )
        .unwrap();
        let plan = plan_update(PlanUpdateRequest {
            target: &target,
            original,
            fields: &[update_values("location", &["Westfield"])],
            notes: &[],
            today: UPDATE_TODAY,
        })
        .unwrap();
        assert!(plan.missing.is_empty());
        assert!(plan.invalid.is_empty());
        assert!(plan.markdown.is_some());
        assert_eq!(plan.changed, vec!["location".to_owned()]);
        assert!(plan
            .warnings
            .iter()
            .any(|warning| warning.field.as_deref() == Some("status")));
        let status = plan
            .properties
            .iter()
            .find(|property| property.key == "status")
            .expect("status property");
        assert_eq!(status.current_display.as_deref(), Some("bogus"));
        assert_eq!(status.state, RecordPropertyState::Set);
        assert_eq!(status.value, status.current);
        let location = plan
            .properties
            .iter()
            .find(|property| property.key == "location")
            .expect("location property");
        assert_eq!(location.current_display.as_deref(), Some("Clark"));
        assert_eq!(location.display.as_deref(), Some("Westfield"));

        let missing_original = "---\ntitle: M\nstatus: liked\ncategory: Coffee\ncreated: 2026-11-08T12:00:00+0000\n---\n\n# M\n\n## Notes\n";
        write_update_note(&root, "eat/missing_req.md", missing_original);
        let mut catalog = RecordCatalog::new(&root);
        let target = resolve_update_target(
            &config,
            &mut catalog,
            "restaurant",
            "missing_req",
        )
        .unwrap();
        let plan = plan_update(PlanUpdateRequest {
            target: &target,
            original: missing_original,
            fields: &[],
            notes: &[],
            today: UPDATE_TODAY,
        })
        .unwrap();
        assert!(plan.markdown.is_some());
        assert!(plan.missing.is_empty());
        assert!(plan
            .warnings
            .iter()
            .any(|warning| warning.field.as_deref() == Some("location")));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn update_current_values_come_from_the_supplied_snapshot() {
        let root = temp_dir("update-snapshot");
        let on_disk = "---\ntitle: Snapshot\nstatus: liked\ncategory: Coffee\nlocation: Clark\ncreated: 2026-11-08T12:00:00+0000\n---\n\n# Snapshot\n\n## Notes\n";
        write_update_note(&root, "eat/snapshot.md", on_disk);
        let snapshot = on_disk.replace("Clark", "Westfield");
        let config = config();
        let mut catalog = RecordCatalog::new(&root);
        let target = resolve_update_target(
            &config,
            &mut catalog,
            "restaurant",
            "snapshot",
        )
        .unwrap();
        let plan = plan_update(PlanUpdateRequest {
            target: &target,
            original: &snapshot,
            fields: &[],
            notes: &[],
            today: UPDATE_TODAY,
        })
        .unwrap();
        assert_eq!(plan.original, snapshot);
        assert_eq!(
            plan.properties
                .iter()
                .find(|property| property.key == "location")
                .expect("location property")
                .current_display
                .as_deref(),
            Some("Westfield")
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn update_structural_snapshot_defects_refuse_with_unchanged_bytes() {
        let root = temp_dir("update-refusals");
        let config = config();
        let malformed =
            "---\ntitle: Bad\nstatus: \"unterminated\ncategory: Coffee\n";
        write_update_note(&root, "eat/bad.md", malformed);
        let mut catalog = RecordCatalog::new(&root);
        let target =
            resolve_update_target(&config, &mut catalog, "restaurant", "bad")
                .unwrap();
        let error = plan_update(PlanUpdateRequest {
            target: &target,
            original: malformed,
            fields: &[update_values("location", &["Clark"])],
            notes: &[],
            today: UPDATE_TODAY,
        })
        .unwrap_err();
        assert_eq!(
            error.code,
            crate::native::records::render::RecordUpdateErrorCode::MalformedFrontmatter
        );
        assert_eq!(error.unchanged, malformed);

        let duplicate = "---\ntitle: Dup\nstatus: liked\nstatus: not_liked\ncategory: Coffee\nlocation: Clark\ncreated: 2026-11-08T12:00:00+0000\n---\n\n# Dup\n";
        write_update_note(&root, "eat/dup.md", duplicate);
        let mut catalog = RecordCatalog::new(&root);
        let target =
            resolve_update_target(&config, &mut catalog, "restaurant", "dup")
                .unwrap();
        let error = plan_update(PlanUpdateRequest {
            target: &target,
            original: duplicate,
            fields: &[update_values("status", &["liked"])],
            notes: &[],
            today: UPDATE_TODAY,
        })
        .unwrap_err();
        assert_eq!(
            error.code,
            crate::native::records::render::RecordUpdateErrorCode::DuplicateKey
        );
        assert_eq!(error.unchanged, duplicate);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn update_real_note_goldens_match_complete_expected_bytes() {
        let ahrres = include_str!(
            "../../../tests/fixtures/records/source/eat/ahrres.md"
        );
        let talk_python = include_str!(
            "../../../tests/fixtures/records/source/podcasts/talk_python.md"
        );
        let root = temp_dir("update-goldens");
        write_update_note(&root, "eat/ahrres.md", ahrres);
        write_update_note(&root, "podcasts/talk_python.md", talk_python);
        let config = parse_records_config(
            r#"
records:
  restaurant:
    dir: eat
    label: Restaurant
    dated_notes: true
    fields:
      status: {required: true, values: {liked: Liked, to_try: To try, not_liked: Not liked}}
      category: {required: true}
      location: {required: true}
      companions: {values: link, list: true}
      visited: {values: date}
      url: {values: url}
  podcast:
    dir: podcasts
    label: Podcast
    dated_notes: false
    fields:
      status: {required: true, values: {subscribed: Subscribed, to_try: To try, unsubscribed: Unsubscribed}}
      category: {required: true}
      url: {values: url}
"#,
            Path::new("/config.yml"),
        )
        .unwrap();

        let mut catalog = RecordCatalog::new(&root);
        let target = resolve_update_target(
            &config,
            &mut catalog,
            "restaurant",
            "ahrres",
        )
        .unwrap();
        assert_eq!(target.stem, "ahrres");
        assert_eq!(target.path, PathBuf::from("eat/ahrres.md"));
        assert_eq!(target.title, "Ahrres");
        let plan = plan_update(PlanUpdateRequest {
            target: &target,
            original: ahrres,
            fields: &[update_values("location", &["Clark"])],
            notes: &[RecordNoteInput::bullet("Revisit was great")],
            today: UPDATE_TODAY,
        })
        .unwrap();
        let expected = ahrres
            .replacen("location: \"Westfield\"\n", "location: \"Clark\"\n", 1)
            .replacen(
                "## Notes\n\n## Source",
                "## Notes\n\n- 2026-11-08: Revisit was great\n\n## Source",
                1,
            );
        assert_eq!(plan.markdown.as_deref(), Some(expected.as_str()));
        assert_eq!(plan.changed, vec!["location".to_owned()]);
        assert!(plan.warnings.is_empty());
        let expected_body =
            crate::native::highlights_ref::split_frontmatter(&expected)
                .map(|(_, body)| body)
                .unwrap_or_default();
        assert_eq!(plan.body.as_deref(), Some(expected_body.as_str()));
        let location = plan
            .properties
            .iter()
            .find(|property| property.key == "location")
            .expect("location property");
        assert_eq!(location.current_display.as_deref(), Some("Westfield"));
        assert_eq!(location.display.as_deref(), Some("Clark"));

        let mut catalog = RecordCatalog::new(&root);
        let target = resolve_update_target(
            &config,
            &mut catalog,
            "podcast",
            "talk_python",
        )
        .unwrap();
        let plan = plan_update(PlanUpdateRequest {
            target: &target,
            original: talk_python,
            fields: &[update_values("category", &["rust"])],
            notes: &[RecordNoteInput::bullet("Listened to the async episode")],
            today: UPDATE_TODAY,
        })
        .unwrap();
        let expected = talk_python
            .replacen("category: python\n", "category: \"rust\"\n", 1)
            .replacen(
                "## Notes\n\n## Source",
                "## Notes\n\n- Listened to the async episode\n\n## Source",
                1,
            );
        assert_eq!(plan.markdown.as_deref(), Some(expected.as_str()));
        assert_eq!(plan.changed, vec!["category".to_owned()]);
        let category = plan
            .properties
            .iter()
            .find(|property| property.key == "category")
            .expect("category property");
        assert_eq!(category.current_display.as_deref(), Some("python"));
        assert_eq!(category.display.as_deref(), Some("rust"));
        fs::remove_dir_all(root).unwrap();
    }
}
