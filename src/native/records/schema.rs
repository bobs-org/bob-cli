use std::{
    collections::HashMap,
    fmt,
    ops::Range,
    path::{Path, PathBuf},
};

use chrono::NaiveDate;
use serde_json::{json, Number as JsonNumber, Value as JsonValue};
use serde_yaml::{Mapping, Value};
use sha2::{Digest, Sha256};
use url::Url;

const RESERVED_RECORD_IDS: &[&str] = &[
    "project", "area", "ref", "day", "done", "mail", "article", "type",
    "notebook",
];
const ALBUM_RESERVED_RECORD_IDS: &[&str] = &[
    "project", "area", "ref", "day", "done", "mail", "article", "type",
    "notebook", "album", "record",
];
const ALBUM_RECORD_KEYS: &[&str] =
    &["noun", "label", "plural", "dated_notes", "fields", "legacy"];
const RESERVED_FIELD_NAMES: &[&str] = &["parent", "type", "title", "created"];
const RECORD_KEYS: &[&str] = &[
    "dir",
    "label",
    "emoji",
    "parent",
    "dated_notes",
    "fields",
    "legacy",
];
const FIELD_KEYS: &[&str] = &[
    "required", "values", "list", "default", "suggest", "label", "min", "max",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum RecordDiagnosticCode {
    ConfigRead,
    InvalidYaml,
    InvalidConfigRoot,
    InvalidRecordsBlock,
    InvalidRecordType,
    InvalidRecordId,
    ReservedRecordId,
    MissingRecordProperty,
    InvalidDirectory,
    DuplicateDirectory,
    InvalidFieldName,
    ReservedFieldName,
    InvalidField,
    InvalidFieldOption,
    InvalidFieldKind,
    InvalidChoice,
    InvalidDefault,
    InvalidNumberBound,
    IncompatiblePropertyStorage,
    UnknownField,
    InvalidValue,
    InvalidDate,
    InvalidLink,
    InvalidUrl,
    InvalidNumber,
    NumberOutOfRange,
    InvalidList,
    MissingTitle,
    NoteRead,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RecordDiagnostic {
    pub(crate) code: RecordDiagnosticCode,
    pub(crate) config_path: Option<PathBuf>,
    pub(crate) record_type: Option<String>,
    pub(crate) field: Option<String>,
    pub(crate) message: String,
    /// Bounded suggestions for caller-supplied keys such as an unknown field.
    pub(crate) suggestions: Vec<String>,
    /// Allowed keys for structured correction UI.
    pub(crate) allowed_keys: Vec<String>,
    /// Caller-provided byte ranges; the engine never invents ranges.
    pub(crate) source_ranges: Option<Vec<Range<usize>>>,
    /// Album declarations use file/line/key paths instead of config provenance.
    pub(crate) file_path: Option<PathBuf>,
    pub(crate) line_number: Option<usize>,
    pub(crate) key_path: Option<String>,
}

impl RecordDiagnostic {
    pub(crate) fn config(
        path: &Path,
        record_type: Option<&str>,
        field: Option<&str>,
        code: RecordDiagnosticCode,
        message: impl Into<String>,
    ) -> Self {
        Self {
            code,
            config_path: Some(path.to_path_buf()),
            record_type: record_type.map(str::to_owned),
            field: field.map(str::to_owned),
            message: message.into(),
            suggestions: Vec::new(),
            allowed_keys: Vec::new(),
            source_ranges: None,
            file_path: None,
            line_number: None,
            key_path: None,
        }
    }

    pub(crate) fn note(
        field: Option<&str>,
        code: RecordDiagnosticCode,
        message: impl Into<String>,
    ) -> Self {
        Self {
            code,
            config_path: None,
            record_type: None,
            field: field.map(str::to_owned),
            message: message.into(),
            suggestions: Vec::new(),
            allowed_keys: Vec::new(),
            source_ranges: None,
            file_path: None,
            line_number: None,
            key_path: None,
        }
    }

    pub(crate) fn value(
        field: &RecordField,
        code: RecordDiagnosticCode,
        message: impl Into<String>,
    ) -> Self {
        Self {
            code,
            config_path: None,
            record_type: None,
            field: Some(field.key.clone()),
            message: message.into(),
            suggestions: Vec::new(),
            allowed_keys: Vec::new(),
            source_ranges: None,
            file_path: None,
            line_number: None,
            key_path: None,
        }
    }

    pub(crate) fn album(
        path: &Path,
        line_number: usize,
        key_path: impl Into<String>,
        record_type: Option<&str>,
        code: RecordDiagnosticCode,
        message: impl Into<String>,
    ) -> Self {
        Self {
            code,
            config_path: None,
            record_type: record_type.map(str::to_owned),
            field: None,
            message: message.into(),
            suggestions: Vec::new(),
            allowed_keys: Vec::new(),
            source_ranges: None,
            file_path: Some(path.to_path_buf()),
            line_number: Some(line_number),
            key_path: Some(key_path.into()),
        }
    }

    pub(crate) fn diagnostic_code(&self) -> &'static str {
        match self.code {
            RecordDiagnosticCode::UnknownField => "record_unknown_field",
            RecordDiagnosticCode::MissingTitle => "record_title",
            RecordDiagnosticCode::InvalidValue
            | RecordDiagnosticCode::InvalidDate
            | RecordDiagnosticCode::InvalidLink
            | RecordDiagnosticCode::InvalidUrl
            | RecordDiagnosticCode::InvalidNumber
            | RecordDiagnosticCode::NumberOutOfRange
            | RecordDiagnosticCode::InvalidList
            | RecordDiagnosticCode::InvalidChoice => "record_invalid_value",
            _ => "record_invalid_value",
        }
    }
}

impl fmt::Display for RecordDiagnostic {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let album_location = self.file_path.is_some()
            && self.line_number.is_some()
            && self.key_path.is_some();
        if let Some(path) = &self.config_path {
            write!(formatter, "records config in {}", path.display())?;
        } else if let (Some(path), Some(line), Some(key_path)) =
            (&self.file_path, self.line_number, &self.key_path)
        {
            write!(formatter, "{}:{line} {key_path}", path.display())?;
        } else {
            formatter.write_str("record value")?;
        }
        if !album_location {
            if let Some(record_type) = &self.record_type {
                write!(formatter, ", type {record_type}")?;
            }
            if let Some(field) = &self.field {
                write!(formatter, ", field {field}")?;
            }
        }
        write!(formatter, ": {}", self.message)
    }
}

#[derive(Clone, Copy)]
enum SchemaSourceKind<'a> {
    Config,
    Album {
        line_by_path: &'a HashMap<String, usize>,
        record_line: usize,
    },
}

/// Carries the location/provenance policy through the shared field-schema
/// validators. Config callers retain their existing structured diagnostics;
/// album callers get a physical line and a complete YAML key path.
#[derive(Clone)]
struct RecordSchemaSource<'a> {
    path: &'a Path,
    record_type: &'a str,
    field: Option<String>,
    key_path: Vec<String>,
    kind: SchemaSourceKind<'a>,
}

impl<'a> RecordSchemaSource<'a> {
    fn config(path: &'a Path, record_type: &'a str) -> Self {
        Self {
            path,
            record_type,
            field: None,
            key_path: Vec::new(),
            kind: SchemaSourceKind::Config,
        }
    }

    fn album(
        path: &'a Path,
        record_type: &'a str,
        line_by_path: &'a HashMap<String, usize>,
        record_line: usize,
    ) -> Self {
        Self {
            path,
            record_type,
            field: None,
            key_path: vec!["record".to_owned()],
            kind: SchemaSourceKind::Album {
                line_by_path,
                record_line,
            },
        }
    }

    fn field(&self, key: &str) -> Self {
        let mut source = self.clone();
        source.field = Some(key.to_owned());
        if let SchemaSourceKind::Album { .. } = source.kind {
            source.key_path.push("fields".to_owned());
            source.key_path.push(key.to_owned());
        }
        source
    }

    fn at(&self, key: &str) -> Self {
        let mut source = self.clone();
        if let SchemaSourceKind::Album { .. } = source.kind {
            source.key_path.push(key.to_owned());
        }
        source
    }

    fn diagnostic(
        &self,
        code: RecordDiagnosticCode,
        message: impl Into<String>,
    ) -> RecordDiagnostic {
        match self.kind {
            SchemaSourceKind::Config => RecordDiagnostic::config(
                self.path,
                Some(self.record_type),
                self.field.as_deref(),
                code,
                message,
            ),
            SchemaSourceKind::Album {
                line_by_path,
                record_line,
            } => {
                let key_path = self.key_path.join(".");
                let line =
                    line_by_path.get(&key_path).copied().unwrap_or(record_line);
                let mut diagnostic = RecordDiagnostic::album(
                    self.path,
                    line,
                    key_path,
                    Some(self.record_type),
                    code,
                    message,
                );
                diagnostic.field = self.field.clone();
                diagnostic
            }
        }
    }
}

impl std::error::Error for RecordDiagnostic {}

#[derive(Debug, Clone, PartialEq, Default)]
pub(crate) struct RecordsConfig {
    pub(crate) types: Vec<RecordType>,
}

impl RecordsConfig {
    pub(crate) fn record_type(&self, id: &str) -> Option<&RecordType> {
        self.types.iter().find(|record_type| record_type.id == id)
    }

    pub(crate) fn by_directory(&self, directory: &str) -> Option<&RecordType> {
        self.types.iter().find(|record_type| {
            record_type.dir.to_lowercase() == directory.to_lowercase()
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct RecordType {
    pub(crate) id: String,
    pub(crate) dir: String,
    pub(crate) label: String,
    pub(crate) emoji: Option<String>,
    pub(crate) parent: Option<String>,
    pub(crate) dated_notes: bool,
    pub(crate) fields: Vec<RecordField>,
    pub(crate) legacy: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct AlbumRecordSchemaParse {
    /// Preserved even when another part of the declaration is invalid.
    pub(crate) noun_claim: Option<String>,
    pub(crate) label: Option<String>,
    pub(crate) plural: Option<String>,
    pub(crate) record_type: Option<RecordType>,
    pub(crate) diagnostic: Option<RecordDiagnostic>,
}

impl RecordType {
    pub(crate) fn field(&self, key: &str) -> Option<&RecordField> {
        self.fields.iter().find(|field| field.key == key)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct RecordField {
    pub(crate) key: String,
    pub(crate) label: String,
    pub(crate) kind: FieldKind,
    pub(crate) required: bool,
    pub(crate) list: bool,
    pub(crate) default: Option<RecordValue>,
    pub(crate) suggest: bool,
    pub(crate) min: Option<f64>,
    pub(crate) max: Option<f64>,
}

impl RecordField {
    /// JSON shape shared by album reports and `bob records show`.
    pub(crate) fn schema_json(
        &self,
        line: Option<usize>,
        filled: usize,
        choice_counts: &[usize],
    ) -> JsonValue {
        let choices = match &self.kind {
            FieldKind::Choices(choices) => choices
                .iter()
                .enumerate()
                .map(|(index, choice)| {
                    json!({
                        "value": choice.value,
                        "label": choice.label,
                        "count": choice_counts.get(index).copied().unwrap_or(0),
                    })
                })
                .collect::<Vec<_>>(),
            _ => Vec::new(),
        };
        let mut field = json!({
            "key": self.key,
            "label": self.label,
            "kind": self.kind.kind_name(),
            "required": self.required,
            "list": self.list,
            "suggest": self.suggest,
            "line": line,
            "filled": filled,
            "choices": choices,
        });
        if let Some(default) = &self.default {
            field["default"] = record_value_json(default);
        }
        if let Some(min) = self.min {
            field["min"] = JsonNumber::from_f64(min)
                .map(JsonValue::Number)
                .unwrap_or(JsonValue::Null);
        }
        if let Some(max) = self.max {
            field["max"] = JsonNumber::from_f64(max)
                .map(JsonValue::Number)
                .unwrap_or(JsonValue::Null);
        }
        field
    }
}

fn record_value_json(value: &RecordValue) -> JsonValue {
    match value {
        RecordValue::Scalar(value) => record_scalar_json(value),
        RecordValue::List(values) => {
            JsonValue::Array(values.iter().map(record_scalar_json).collect())
        }
    }
}

fn record_scalar_json(value: &RecordScalar) -> JsonValue {
    match value {
        RecordScalar::Text(value)
        | RecordScalar::Choice(value)
        | RecordScalar::Link(value) => JsonValue::String(value.clone()),
        RecordScalar::Date(RecordDate::Resolved(value)) => {
            JsonValue::String(value.format("%Y-%m-%d").to_string())
        }
        RecordScalar::Date(RecordDate::Today) => {
            JsonValue::String("today".to_owned())
        }
        RecordScalar::Date(RecordDate::Yesterday) => {
            JsonValue::String("yesterday".to_owned())
        }
        RecordScalar::Url(value) => JsonValue::String(value.to_string()),
        RecordScalar::Number(value) => JsonNumber::from_f64(*value)
            .map(JsonValue::Number)
            .unwrap_or(JsonValue::Null),
    }
}

/// Fingerprint the parsed, semantic album schema. Source formatting and
/// source line numbers are deliberately absent; all ordered schema content
/// and typed attributes are included for reuse by the guarded writer.
pub(crate) fn album_schema_fingerprint(
    noun: &str,
    label: &str,
    plural: &str,
    record_type: &RecordType,
) -> String {
    let fields = record_type
        .fields
        .iter()
        .map(|field| {
            json!([
                field.key,
                field.label,
                field_kind_semantic_json(&field.kind),
                field.required,
                field.list,
                field.suggest,
                field.default.as_ref().map(record_value_semantic_json),
                float_semantic_json(field.min),
                float_semantic_json(field.max),
            ])
        })
        .collect::<Vec<_>>();
    let semantic = json!([
        noun,
        label,
        plural,
        record_type.dated_notes,
        record_type.legacy,
        fields,
    ]);
    let bytes = serde_json::to_vec(&semantic)
        .expect("album schema semantic model is serializable");
    format!("sha256:{}", hex::encode(Sha256::digest(bytes)))
}

fn field_kind_semantic_json(kind: &FieldKind) -> JsonValue {
    match kind {
        FieldKind::Text => json!(["text"]),
        FieldKind::Choices(choices) => json!([
            "choices",
            choices
                .iter()
                .map(|choice| json!([choice.value, choice.label]))
                .collect::<Vec<_>>(),
        ]),
        FieldKind::Date => json!(["date"]),
        FieldKind::Link => json!(["link"]),
        FieldKind::Url => json!(["url"]),
        FieldKind::Number => json!(["number"]),
    }
}

fn record_value_semantic_json(value: &RecordValue) -> JsonValue {
    match value {
        RecordValue::Scalar(value) => {
            json!(["scalar", record_scalar_semantic_json(value)])
        }
        RecordValue::List(values) => json!([
            "list",
            values
                .iter()
                .map(record_scalar_semantic_json)
                .collect::<Vec<_>>(),
        ]),
    }
}

fn record_scalar_semantic_json(value: &RecordScalar) -> JsonValue {
    match value {
        RecordScalar::Text(value) => json!(["text", value]),
        RecordScalar::Choice(value) => json!(["choice", value]),
        RecordScalar::Date(RecordDate::Resolved(value)) => {
            json!(["date", value.format("%Y-%m-%d").to_string()])
        }
        RecordScalar::Date(RecordDate::Today) => json!(["date", "today"]),
        RecordScalar::Date(RecordDate::Yesterday) => {
            json!(["date", "yesterday"])
        }
        RecordScalar::Link(value) => json!(["link", value]),
        RecordScalar::Url(value) => json!(["url", value.as_str()]),
        RecordScalar::Number(value) => {
            json!(["number", format!("{:016x}", value.to_bits())])
        }
    }
}

fn float_semantic_json(value: Option<f64>) -> JsonValue {
    value
        .map(|number| json!(format!("{:016x}", number.to_bits())))
        .unwrap_or(JsonValue::Null)
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum FieldKind {
    Text,
    Choices(Vec<RecordChoice>),
    Date,
    Link,
    Url,
    Number,
}

impl FieldKind {
    pub(crate) fn storage_kind(&self) -> PropertyStorageKind {
        match self {
            Self::Text | Self::Choices(_) | Self::Link | Self::Url => {
                PropertyStorageKind::String
            }
            Self::Date => PropertyStorageKind::Date,
            Self::Number => PropertyStorageKind::Number,
        }
    }

    pub(crate) fn kind_name(&self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Choices(_) => "choices",
            Self::Date => "date",
            Self::Link => "link",
            Self::Url => "url",
            Self::Number => "number",
        }
    }

    fn display_name(&self) -> &'static str {
        self.kind_name()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum PropertyStorageKind {
    String,
    Date,
    Number,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RecordChoice {
    pub(crate) value: String,
    pub(crate) label: String,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum RecordValue {
    Scalar(RecordScalar),
    List(Vec<RecordScalar>),
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum RecordScalar {
    Text(String),
    Choice(String),
    Date(RecordDate),
    Link(String),
    Url(Url),
    Number(f64),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RecordDate {
    Resolved(NaiveDate),
    Today,
    Yesterday,
}

impl RecordDate {
    pub(crate) fn resolve(self, today: NaiveDate) -> Option<NaiveDate> {
        match self {
            Self::Resolved(date) => Some(date),
            Self::Today => Some(today),
            Self::Yesterday => today.pred_opt(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RecordFieldInput {
    pub(crate) key: String,
    pub(crate) state: FieldInputState,
    pub(crate) source_range: Option<Range<usize>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum FieldInputState {
    Absent,
    SuppliedEmpty,
    /// Each item is one occurrence from the caller. List fields also accept
    /// comma-separated members inside each occurrence.
    SuppliedValues(Vec<String>),
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum ValidatedFieldInput {
    Absent,
    Empty,
    Value(RecordValue),
}

pub(crate) fn validate_field_input(
    field: &RecordField,
    input: &FieldInputState,
    today: NaiveDate,
) -> Result<ValidatedFieldInput, RecordDiagnostic> {
    match input {
        FieldInputState::Absent => Ok(ValidatedFieldInput::Absent),
        FieldInputState::SuppliedEmpty => Ok(ValidatedFieldInput::Empty),
        FieldInputState::SuppliedValues(values) => {
            validate_field_values(field, values, today)
                .map(ValidatedFieldInput::Value)
        }
    }
}

pub(crate) fn validate_field_values(
    field: &RecordField,
    raw_values: &[String],
    today: NaiveDate,
) -> Result<RecordValue, RecordDiagnostic> {
    if raw_values.is_empty() {
        return Err(RecordDiagnostic::value(
            field,
            RecordDiagnosticCode::InvalidValue,
            "expected at least one supplied value",
        ));
    }

    if !field.list {
        if raw_values.len() != 1 {
            return Err(RecordDiagnostic::value(
                field,
                RecordDiagnosticCode::InvalidValue,
                "scalar fields accept exactly one value",
            ));
        }
        return validate_scalar(field, &raw_values[0], today)
            .map(RecordValue::Scalar);
    }

    let mut members = Vec::new();
    for raw in raw_values {
        members.extend(split_list_members(raw).map_err(|message| {
            RecordDiagnostic::value(
                field,
                RecordDiagnosticCode::InvalidList,
                message,
            )
        })?);
    }
    if members.is_empty() {
        return Err(RecordDiagnostic::value(
            field,
            RecordDiagnosticCode::InvalidList,
            "a supplied list must contain at least one value",
        ));
    }
    let values = members
        .iter()
        .map(|member| validate_scalar(field, member, today))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(RecordValue::List(values))
}

/// Parse the ordered `records:` config block without adding strict record
/// validation to shared `RawConfig` consumers.
pub(crate) fn parse_records_config(
    text: &str,
    path: &Path,
) -> Result<RecordsConfig, RecordDiagnostic> {
    let root: Value = serde_yaml::from_str(text).map_err(|error| {
        RecordDiagnostic::config(
            path,
            None,
            None,
            RecordDiagnosticCode::InvalidYaml,
            format!("could not parse YAML: {error}"),
        )
    })?;
    let Some(root_mapping) = root.as_mapping() else {
        return Err(RecordDiagnostic::config(
            path,
            None,
            None,
            RecordDiagnosticCode::InvalidConfigRoot,
            "configuration root must be a mapping",
        ));
    };
    let Some(records_value) = mapping_get(root_mapping, "records") else {
        return Ok(RecordsConfig::default());
    };
    let Some(record_mapping) = records_value.as_mapping() else {
        return Err(RecordDiagnostic::config(
            path,
            None,
            None,
            RecordDiagnosticCode::InvalidRecordsBlock,
            format!(
                "records must be a mapping; got {}",
                yaml_kind(records_value)
            ),
        ));
    };

    let mut types = Vec::with_capacity(record_mapping.len());
    let mut directories = std::collections::HashMap::<String, String>::new();
    let mut property_shapes = std::collections::HashMap::<
        String,
        (PropertyStorageKind, bool, String),
    >::new();

    for (id_value, type_value) in record_mapping {
        let Some(id) = id_value.as_str() else {
            return Err(RecordDiagnostic::config(
                path,
                None,
                None,
                RecordDiagnosticCode::InvalidRecordType,
                format!(
                    "record type ids must be strings; got {}",
                    yaml_kind(id_value)
                ),
            ));
        };
        if !valid_identifier(id) {
            return Err(RecordDiagnostic::config(
                path,
                Some(id),
                None,
                RecordDiagnosticCode::InvalidRecordId,
                "record type id must match [a-z][a-z0-9_]*",
            ));
        }
        if RESERVED_RECORD_IDS.contains(&id) {
            return Err(RecordDiagnostic::config(
                path,
                Some(id),
                None,
                RecordDiagnosticCode::ReservedRecordId,
                "record type id is reserved for a built-in note type",
            ));
        }
        let type_map = expect_mapping(
            type_value,
            path,
            id,
            None,
            RecordDiagnosticCode::InvalidRecordType,
            "record type definition",
        )?;
        let source = RecordSchemaSource::config(path, id);
        reject_unknown_keys(type_map, RECORD_KEYS, &source, "record type")?;

        let dir = required_string(type_map, "dir", path, id, None)?;
        if !valid_directory(&dir) {
            return Err(RecordDiagnostic::config(
                path,
                Some(id),
                None,
                RecordDiagnosticCode::InvalidDirectory,
                format!("dir must be a safe, relative, single folder segment; got {dir:?}"),
            ));
        }
        let folded_dir = dir.to_lowercase();
        if let Some(previous) = directories.insert(folded_dir, id.to_owned()) {
            return Err(RecordDiagnostic::config(
                path,
                Some(id),
                None,
                RecordDiagnosticCode::DuplicateDirectory,
                format!("dir {dir:?} duplicates the case-insensitive directory for record type {previous}"),
            ));
        }
        let label = required_string(type_map, "label", path, id, None)?;
        let emoji = optional_string(type_map, "emoji", path, id, None)?;
        let parent = optional_string(type_map, "parent", path, id, None)?;
        let dated_notes =
            optional_bool(type_map, "dated_notes", false, path, id, None)?;
        let fields_value =
            mapping_get(type_map, "fields").ok_or_else(|| {
                RecordDiagnostic::config(
                    path,
                    Some(id),
                    None,
                    RecordDiagnosticCode::MissingRecordProperty,
                    "fields is required and must be an ordered mapping",
                )
            })?;
        let field_map = expect_mapping(
            fields_value,
            path,
            id,
            None,
            RecordDiagnosticCode::InvalidField,
            "fields",
        )?;
        let mut fields = Vec::with_capacity(field_map.len());
        for (field_key_value, field_value) in field_map {
            let Some(key) = field_key_value.as_str() else {
                return Err(RecordDiagnostic::config(
                    path,
                    Some(id),
                    None,
                    RecordDiagnosticCode::InvalidField,
                    format!(
                        "field keys must be strings; got {}",
                        yaml_kind(field_key_value)
                    ),
                ));
            };
            if !valid_identifier(key) {
                return Err(RecordDiagnostic::config(
                    path,
                    Some(id),
                    Some(key),
                    RecordDiagnosticCode::InvalidFieldName,
                    "field name must match [a-z][a-z0-9_]*",
                ));
            }
            if RESERVED_FIELD_NAMES.contains(&key) {
                return Err(RecordDiagnostic::config(
                    path,
                    Some(id),
                    Some(key),
                    RecordDiagnosticCode::ReservedFieldName,
                    "field name is reserved by record-note frontmatter",
                ));
            }
            let field = parse_field(field_value, &source.field(key), key)?;
            if let Some((previous_kind, previous_list, previous_type)) =
                property_shapes.get(key)
            {
                if *previous_kind != field.kind.storage_kind()
                    || *previous_list != field.list
                {
                    return Err(RecordDiagnostic::config(
                        path,
                        Some(id),
                        Some(key),
                        RecordDiagnosticCode::IncompatiblePropertyStorage,
                        format!(
                            "property storage must agree across record types; {previous_type}.{key} is {}{} but {id}.{key} is {}{}",
                            previous_kind.display(),
                            if *previous_list { " list" } else { " scalar" },
                            field.kind.storage_kind().display(),
                            if field.list { " list" } else { " scalar" },
                        ),
                    ));
                }
            } else {
                property_shapes.insert(
                    key.to_owned(),
                    (field.kind.storage_kind(), field.list, id.to_owned()),
                );
            }
            fields.push(field);
        }
        let legacy = parse_legacy(type_map, &source.at("legacy"))?;
        types.push(RecordType {
            id: id.to_owned(),
            dir,
            label,
            emoji,
            parent,
            dated_notes,
            fields,
            legacy,
        });
    }

    Ok(RecordsConfig { types })
}

/// Parse the nested schema owned by an album note. Config parsing remains a
/// separate caller with its historical required `dir`, `label`, and `fields`
/// contract; only the field validators and value rules are shared.
pub(crate) fn parse_album_record_schema(
    value: Option<&Value>,
    path: &Path,
    stem: &str,
    emoji: Option<String>,
    line_by_path: &HashMap<String, usize>,
    record_line: usize,
) -> AlbumRecordSchemaParse {
    let mapping = match value {
        Some(Value::Mapping(mapping)) => mapping,
        Some(value) => {
            let diagnostic = RecordDiagnostic::album(
                path,
                record_line,
                "record",
                None,
                RecordDiagnosticCode::InvalidRecordType,
                format!("record must be a mapping; got {}", yaml_kind(value)),
            );
            return AlbumRecordSchemaParse {
                noun_claim: recover_album_noun(value),
                label: None,
                plural: None,
                record_type: None,
                diagnostic: Some(diagnostic),
            };
        }
        None => {
            let diagnostic = RecordDiagnostic::album(
                path,
                record_line,
                "record.noun",
                None,
                RecordDiagnosticCode::MissingRecordProperty,
                "record is required and must define noun",
            );
            return AlbumRecordSchemaParse {
                noun_claim: None,
                label: None,
                plural: None,
                record_type: None,
                diagnostic: Some(diagnostic),
            };
        }
    };

    let noun_claim = mapping_get(mapping, "noun")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let display_label = noun_claim.as_deref().map(|noun| {
        mapping_get(mapping, "label")
            .and_then(Value::as_str)
            .filter(|label| !label.trim().is_empty())
            .map(str::to_owned)
            .unwrap_or_else(|| album_label(noun))
    });
    let display_plural = mapping_get(mapping, "plural")
        .and_then(Value::as_str)
        .filter(|plural| !plural.trim().is_empty())
        .map(str::to_owned)
        .or_else(|| display_label.as_deref().map(album_plural));
    let source_type = noun_claim.as_deref().unwrap_or("album");
    let source =
        RecordSchemaSource::album(path, source_type, line_by_path, record_line);

    let result = (|| {
        reject_unknown_keys(mapping, ALBUM_RECORD_KEYS, &source, "record")?;

        let noun = mapping_get(mapping, "noun")
            .ok_or_else(|| {
                source.at("noun").diagnostic(
                    RecordDiagnosticCode::MissingRecordProperty,
                    "noun is required",
                )
            })?
            .as_str()
            .filter(|noun| !noun.trim().is_empty())
            .ok_or_else(|| {
                source.at("noun").diagnostic(
                    RecordDiagnosticCode::InvalidRecordId,
                    "noun must be a non-empty string matching [a-z][a-z0-9_]*",
                )
            })?;
        if !valid_identifier(noun) {
            return Err(source.at("noun").diagnostic(
                RecordDiagnosticCode::InvalidRecordId,
                "noun must match [a-z][a-z0-9_]*",
            ));
        }
        if ALBUM_RESERVED_RECORD_IDS.contains(&noun) {
            return Err(source.at("noun").diagnostic(
                RecordDiagnosticCode::ReservedRecordId,
                "record noun is reserved for a built-in note type",
            ));
        }

        let label = album_optional_string(mapping, "label", &source)?
            .unwrap_or_else(|| album_label(noun));
        let _plural = album_optional_string(mapping, "plural", &source)?
            .unwrap_or_else(|| album_plural(&label));
        let dated_notes =
            source_optional_bool(mapping, "dated_notes", false, &source)?;
        let fields_value = mapping_get(mapping, "fields");
        let mut fields = Vec::new();
        if let Some(fields_value) = fields_value {
            let Some(field_map) = fields_value.as_mapping() else {
                return Err(source.at("fields").diagnostic(
                    RecordDiagnosticCode::InvalidField,
                    format!(
                        "fields must be an ordered mapping; got {}",
                        yaml_kind(fields_value)
                    ),
                ));
            };
            fields.reserve(field_map.len());
            for (field_key_value, field_value) in field_map {
                let Some(key) = field_key_value.as_str() else {
                    return Err(source.at("fields").diagnostic(
                        RecordDiagnosticCode::InvalidField,
                        format!(
                            "field keys must be strings; got {}",
                            yaml_kind(field_key_value)
                        ),
                    ));
                };
                let field_source = source.field(key);
                if !valid_identifier(key) {
                    return Err(field_source.diagnostic(
                        RecordDiagnosticCode::InvalidFieldName,
                        "field name must match [a-z][a-z0-9_]*",
                    ));
                }
                if RESERVED_FIELD_NAMES.contains(&key) {
                    return Err(field_source.diagnostic(
                        RecordDiagnosticCode::ReservedFieldName,
                        "field name is reserved by record-note frontmatter",
                    ));
                }
                fields.push(parse_field(field_value, &field_source, key)?);
            }
        }
        let legacy = parse_legacy(mapping, &source.at("legacy"))?;
        Ok(RecordType {
            id: noun.to_owned(),
            dir: stem.to_owned(),
            label,
            emoji,
            parent: Some(format!("[[{stem}]]")),
            dated_notes,
            fields,
            legacy,
        })
    })();

    match result {
        Ok(record_type) => AlbumRecordSchemaParse {
            noun_claim: Some(record_type.id.clone()),
            label: Some(record_type.label.clone()),
            plural: display_plural,
            record_type: Some(record_type),
            diagnostic: None,
        },
        Err(diagnostic) => AlbumRecordSchemaParse {
            noun_claim,
            label: display_label,
            plural: display_plural,
            record_type: None,
            diagnostic: Some(diagnostic),
        },
    }
}

fn album_optional_string(
    mapping: &Mapping,
    name: &str,
    source: &RecordSchemaSource<'_>,
) -> Result<Option<String>, RecordDiagnostic> {
    let Some(value) = mapping_get(mapping, name) else {
        return Ok(None);
    };
    let Some(text) = value.as_str() else {
        return Err(source.at(name).diagnostic(
            RecordDiagnosticCode::InvalidFieldOption,
            format!(
                "{name} must be a non-empty string; got {}",
                yaml_kind(value)
            ),
        ));
    };
    if text.trim().is_empty() {
        return Err(source.at(name).diagnostic(
            RecordDiagnosticCode::InvalidFieldOption,
            format!("{name} must be a non-empty string"),
        ));
    }
    Ok(Some(text.to_owned()))
}

/// The registry's noun-derived label rule, shared with album creation.
pub(crate) fn album_label(noun: &str) -> String {
    let mut label = noun.replace('_', " ");
    if let Some(first) = label.get_mut(..1) {
        first.make_ascii_uppercase();
    }
    label
}

fn album_plural(label: &str) -> String {
    let lowered = if label
        .as_bytes()
        .get(0..2)
        .is_some_and(|prefix| prefix.iter().all(u8::is_ascii_uppercase))
    {
        label.to_owned()
    } else {
        let mut chars = label.chars();
        chars
            .next()
            .map(|first| first.to_lowercase().chain(chars).collect())
            .unwrap_or_default()
    };
    format!("{lowered}s")
}

fn recover_album_noun(value: &Value) -> Option<String> {
    let Value::String(raw) = value else {
        return None;
    };
    let inner = raw
        .trim()
        .trim_start_matches('{')
        .trim_end_matches('}')
        .trim();
    inner.split(',').find_map(|part| {
        let (key, value) = part.split_once(':')?;
        (key.trim().trim_matches(['\'', '"']) == "noun")
            .then(|| value.trim().trim_matches(['\'', '"']).to_owned())
    })
}

fn parse_field(
    value: &Value,
    source: &RecordSchemaSource<'_>,
    key: &str,
) -> Result<RecordField, RecordDiagnostic> {
    let field_map = value.as_mapping().ok_or_else(|| {
        source.diagnostic(
            RecordDiagnosticCode::InvalidField,
            format!(
                "field definition must be a mapping; got {}",
                yaml_kind(value)
            ),
        )
    })?;
    reject_unknown_keys(field_map, FIELD_KEYS, source, "field")?;

    let required = source_optional_bool(field_map, "required", false, source)?;
    let list = source_optional_bool(field_map, "list", false, source)?;
    let suggest = source_optional_bool(field_map, "suggest", false, source)?;
    let label = source_optional_string(field_map, "label", source)?
        .unwrap_or_else(|| title_case_field(key));
    if label.trim().is_empty() {
        return Err(source.at("label").diagnostic(
            RecordDiagnosticCode::InvalidFieldOption,
            "label must be a non-empty string",
        ));
    }

    let kind = parse_field_kind(field_map, source)?;
    let min = source_optional_number(field_map, "min", source)?;
    let max = source_optional_number(field_map, "max", source)?;
    if (min.is_some() || max.is_some()) && kind != FieldKind::Number {
        return Err(source
            .at(if min.is_some() { "min" } else { "max" })
            .diagnostic(
                RecordDiagnosticCode::InvalidNumberBound,
                "min and max are only valid for number fields",
            ));
    }
    if min.zip(max).is_some_and(|(min, max)| min > max) {
        return Err(source.at("min").diagnostic(
            RecordDiagnosticCode::InvalidNumberBound,
            "min must be less than or equal to max",
        ));
    }
    let mut field = RecordField {
        key: key.to_owned(),
        label,
        kind,
        required,
        list,
        default: None,
        suggest,
        min,
        max,
    };
    if let Some(default_value) = mapping_get(field_map, "default") {
        if key == "status" {
            return Err(source.at("default").diagnostic(
                RecordDiagnosticCode::InvalidDefault,
                "status fields cannot have a default",
            ));
        }
        field.default =
            Some(parse_default(default_value, &field, &source.at("default"))?);
    }
    Ok(field)
}

fn parse_field_kind(
    field_map: &Mapping,
    source: &RecordSchemaSource<'_>,
) -> Result<FieldKind, RecordDiagnostic> {
    let source = source.at("values");
    let Some(value) = mapping_get(field_map, "values") else {
        return Ok(FieldKind::Text);
    };
    if let Some(kind) = value.as_str() {
        return match kind {
            "date" => Ok(FieldKind::Date),
            "link" => Ok(FieldKind::Link),
            "url" => Ok(FieldKind::Url),
            "number" => Ok(FieldKind::Number),
            _ => Err(source.diagnostic(
                RecordDiagnosticCode::InvalidFieldKind,
                format!("values must be date, link, url, number, or a non-empty choice mapping; got {kind:?}"),
            )),
        };
    }
    let Some(choices) = value.as_mapping() else {
        return Err(source.diagnostic(
            RecordDiagnosticCode::InvalidFieldKind,
            format!(
                "values must be a supported kind or choice mapping; got {}",
                yaml_kind(value)
            ),
        ));
    };
    if choices.is_empty() {
        return Err(source.diagnostic(
            RecordDiagnosticCode::InvalidChoice,
            "choice mappings must not be empty",
        ));
    }
    let mut parsed = Vec::with_capacity(choices.len());
    for (choice_value, label_value) in choices {
        let Some(choice) = choice_value.as_str() else {
            return Err(source.diagnostic(
                RecordDiagnosticCode::InvalidChoice,
                format!(
                    "choice keys must be strings; got {}",
                    yaml_kind(choice_value)
                ),
            ));
        };
        let Some(label) = label_value.as_str() else {
            return Err(source.diagnostic(
                RecordDiagnosticCode::InvalidChoice,
                format!(
                    "choice label for {choice:?} must be a string; got {}",
                    yaml_kind(label_value)
                ),
            ));
        };
        if choice.is_empty() || label.trim().is_empty() {
            return Err(source.diagnostic(
                RecordDiagnosticCode::InvalidChoice,
                "choice keys and display labels must be non-empty",
            ));
        }
        parsed.push(RecordChoice {
            value: choice.to_owned(),
            label: label.to_owned(),
        });
    }
    Ok(FieldKind::Choices(parsed))
}

fn parse_default(
    value: &Value,
    field: &RecordField,
    source: &RecordSchemaSource<'_>,
) -> Result<RecordValue, RecordDiagnostic> {
    if field.list {
        let Some(items) = value.as_sequence() else {
            return Err(default_error(
                source,
                format!(
                    "list default must be a sequence; got {}",
                    yaml_kind(value)
                ),
            ));
        };
        let values = items
            .iter()
            .map(|item| default_scalar(item, field, source))
            .collect::<Result<Vec<_>, _>>()?;
        return Ok(RecordValue::List(values));
    }
    if value.is_sequence() {
        return Err(default_error(
            source,
            "scalar default must not be a sequence",
        ));
    }
    default_scalar(value, field, source).map(RecordValue::Scalar)
}

fn default_scalar(
    value: &Value,
    field: &RecordField,
    source: &RecordSchemaSource<'_>,
) -> Result<RecordScalar, RecordDiagnostic> {
    let input = match (&field.kind, value) {
        (FieldKind::Number, Value::Number(number)) => number.to_string(),
        (FieldKind::Number, _) => {
            return Err(default_error(
                source,
                format!(
                    "number default must be an integer or decimal; got {}",
                    yaml_kind(value)
                ),
            ));
        }
        (_, Value::String(text)) => text.clone(),
        (_, _) => {
            return Err(default_error(
                source,
                format!(
                    "default must be a string{}; got {}",
                    if field.list { " member" } else { "" },
                    yaml_kind(value)
                ),
            ));
        }
    };
    if field.kind == FieldKind::Date {
        match input.as_str() {
            "today" => return Ok(RecordScalar::Date(RecordDate::Today)),
            "yesterday" => {
                return Ok(RecordScalar::Date(RecordDate::Yesterday))
            }
            _ => {}
        }
    }
    let date =
        NaiveDate::from_ymd_opt(2000, 1, 1).expect("valid validation date");
    validate_scalar(field, &input, date).map_err(|error| {
        source.diagnostic(
            RecordDiagnosticCode::InvalidDefault,
            format!("default is invalid: {}", error.message),
        )
    })
}

fn default_error(
    source: &RecordSchemaSource<'_>,
    message: impl Into<String>,
) -> RecordDiagnostic {
    source.diagnostic(RecordDiagnosticCode::InvalidDefault, message)
}

fn validate_scalar(
    field: &RecordField,
    raw: &str,
    today: NaiveDate,
) -> Result<RecordScalar, RecordDiagnostic> {
    if raw.is_empty() {
        return Err(RecordDiagnostic::value(
            field,
            RecordDiagnosticCode::InvalidValue,
            "empty values must use the explicit empty-field state",
        ));
    }
    match &field.kind {
        FieldKind::Text => Ok(RecordScalar::Text(raw.to_owned())),
        FieldKind::Choices(choices) => {
            if choices.iter().any(|choice| choice.value == raw) {
                Ok(RecordScalar::Choice(raw.to_owned()))
            } else {
                Err(RecordDiagnostic::value(
                    field,
                    RecordDiagnosticCode::InvalidChoice,
                    format!(
                        "expected one of {}",
                        choices
                            .iter()
                            .map(|choice| choice.value.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                ))
            }
        }
        FieldKind::Date => parse_date(raw, today)
            .map(RecordScalar::Date)
            .map_err(|message| {
                RecordDiagnostic::value(
                    field,
                    RecordDiagnosticCode::InvalidDate,
                    message,
                )
            }),
        FieldKind::Link => {
            normalize_link(raw)
                .map(RecordScalar::Link)
                .map_err(|message| {
                    RecordDiagnostic::value(
                        field,
                        RecordDiagnosticCode::InvalidLink,
                        message,
                    )
                })
        }
        FieldKind::Url => {
            parse_url(raw).map(RecordScalar::Url).map_err(|message| {
                RecordDiagnostic::value(
                    field,
                    RecordDiagnosticCode::InvalidUrl,
                    message,
                )
            })
        }
        FieldKind::Number => parse_number(raw, field.min, field.max)
            .map(RecordScalar::Number)
            .map_err(|mut error| {
                error.field = Some(field.key.clone());
                error
            }),
    }
}

fn parse_date(raw: &str, today: NaiveDate) -> Result<RecordDate, String> {
    match raw {
        "today" => return Ok(RecordDate::Resolved(today)),
        "yesterday" => {
            return today.pred_opt().map(RecordDate::Resolved).ok_or_else(
                || "yesterday is outside the supported date range".to_owned(),
            );
        }
        _ => {}
    }
    let parsed = NaiveDate::parse_from_str(raw, "%Y-%m-%d").map_err(|_| {
        "expected a real ISO date (YYYY-MM-DD), today, or yesterday".to_owned()
    })?;
    if parsed.format("%Y-%m-%d").to_string() != raw {
        return Err("expected a real ISO date in YYYY-MM-DD form".to_owned());
    }
    Ok(RecordDate::Resolved(parsed))
}

fn normalize_link(raw: &str) -> Result<String, String> {
    if raw.is_empty() || raw.trim().is_empty() {
        return Err("link value must not be empty".to_owned());
    }
    if raw.contains('\n') || raw.contains('\r') {
        return Err("link value must be one line".to_owned());
    }
    if raw.starts_with("[[") || raw.ends_with("]]") {
        if !raw.starts_with("[[") || !raw.ends_with("]]") || raw.len() < 5 {
            return Err("expected a complete [[wikilink]]".to_owned());
        }
        let inner = &raw[2..raw.len() - 2];
        if inner.contains("[[")
            || inner.contains("]]")
            || inner.contains('[')
            || inner.contains(']')
        {
            return Err(
                "wikilink contains unmatched or nested brackets".to_owned()
            );
        }
        let (target, alias) = inner.split_once('|').unwrap_or((inner, ""));
        if target.trim().is_empty()
            || (inner.contains('|') && alias.trim().is_empty())
        {
            return Err(
                "wikilink target and alias must be non-empty".to_owned()
            );
        }
        return Ok(raw.to_owned());
    }
    if raw.contains('[') || raw.contains(']') {
        return Err(
            "expected a complete [[wikilink]] or a bare note name".to_owned()
        );
    }
    Ok(format!("[[{raw}]]"))
}

fn parse_url(raw: &str) -> Result<Url, String> {
    let parsed = Url::parse(raw)
        .map_err(|_| "expected an HTTP(S) URL with a host".to_owned())?;
    if !matches!(parsed.scheme(), "http" | "https")
        || parsed.host_str().is_none()
    {
        return Err("expected an HTTP(S) URL with a host".to_owned());
    }
    Ok(parsed)
}

fn parse_number(
    raw: &str,
    min: Option<f64>,
    max: Option<f64>,
) -> Result<f64, RecordDiagnostic> {
    let field = None;
    if !is_decimal_syntax(raw) {
        return Err(RecordDiagnostic {
            code: RecordDiagnosticCode::InvalidNumber,
            config_path: None,
            record_type: None,
            field,
            message: "expected a finite integer or decimal number".to_owned(),
            suggestions: Vec::new(),
            allowed_keys: Vec::new(),
            source_ranges: None,
            file_path: None,
            line_number: None,
            key_path: None,
        });
    }
    let number = raw
        .parse::<f64>()
        .ok()
        .filter(|value| value.is_finite())
        .ok_or_else(|| RecordDiagnostic {
            code: RecordDiagnosticCode::InvalidNumber,
            config_path: None,
            record_type: None,
            field: None,
            message: "expected a finite integer or decimal number".to_owned(),
            suggestions: Vec::new(),
            allowed_keys: Vec::new(),
            source_ranges: None,
            file_path: None,
            line_number: None,
            key_path: None,
        })?;
    if min.is_some_and(|min| number < min)
        || max.is_some_and(|max| number > max)
    {
        return Err(RecordDiagnostic {
            code: RecordDiagnosticCode::NumberOutOfRange,
            config_path: None,
            record_type: None,
            field: None,
            message: format!(
                "number must be within the inclusive bounds {}..={}",
                min.map_or_else(|| "−∞".to_owned(), |value| value.to_string()),
                max.map_or_else(|| "∞".to_owned(), |value| value.to_string())
            ),
            suggestions: Vec::new(),
            allowed_keys: Vec::new(),
            source_ranges: None,
            file_path: None,
            line_number: None,
            key_path: None,
        });
    }
    Ok(number)
}

fn is_decimal_syntax(raw: &str) -> bool {
    if raw.is_empty() || raw.trim() != raw {
        return false;
    }
    let unsigned = raw
        .strip_prefix('+')
        .or_else(|| raw.strip_prefix('-'))
        .unwrap_or(raw);
    if unsigned.is_empty() {
        return false;
    }
    let mut parts = unsigned.split('.');
    let integer = parts.next().unwrap_or_default();
    let fraction = parts.next();
    if parts.next().is_some() {
        return false;
    }
    let integer_digits = integer.bytes().all(|byte| byte.is_ascii_digit());
    let fraction_digits = fraction.is_some_and(|part| {
        !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit())
    });
    match fraction {
        None => !integer.is_empty() && integer_digits,
        Some(_) => {
            (integer.is_empty() || integer_digits)
                && fraction_digits
                && (!integer.is_empty() || fraction.is_some())
        }
    }
}

pub(crate) fn split_list_members(raw: &str) -> Result<Vec<String>, String> {
    if raw.is_empty() {
        return Err(
            "a list value cannot be empty; use an explicit empty field instead"
                .to_owned(),
        );
    }
    let bytes = raw.as_bytes();
    let mut start = 0;
    let mut cursor = 0;
    let mut in_wikilink = false;
    let mut members = Vec::new();
    while cursor < bytes.len() {
        if bytes[cursor..].starts_with(b"[[") {
            if in_wikilink {
                return Err("nested [[wikilink]] in list value".to_owned());
            }
            in_wikilink = true;
            cursor += 2;
        } else if bytes[cursor..].starts_with(b"]]") {
            if !in_wikilink {
                return Err("unmatched ]] in list value".to_owned());
            }
            in_wikilink = false;
            cursor += 2;
        } else if bytes[cursor] == b',' && !in_wikilink {
            push_list_member(raw, start, cursor, &mut members)?;
            cursor += 1;
            start = cursor;
        } else {
            cursor += raw[cursor..]
                .chars()
                .next()
                .expect("cursor remains on a character boundary")
                .len_utf8();
        }
    }
    if in_wikilink {
        return Err("unclosed [[wikilink]] in list value".to_owned());
    }
    push_list_member(raw, start, bytes.len(), &mut members)?;
    Ok(members)
}

fn push_list_member(
    raw: &str,
    start: usize,
    end: usize,
    members: &mut Vec<String>,
) -> Result<(), String> {
    let member = raw[start..end].trim();
    if member.is_empty() {
        return Err("list values must not contain empty members".to_owned());
    }
    members.push(member.to_owned());
    Ok(())
}

fn parse_legacy(
    type_map: &Mapping,
    source: &RecordSchemaSource<'_>,
) -> Result<Vec<String>, RecordDiagnostic> {
    let Some(value) = mapping_get(type_map, "legacy") else {
        return Ok(Vec::new());
    };
    let Some(patterns) = value.as_sequence() else {
        return Err(source.diagnostic(
            RecordDiagnosticCode::InvalidRecordType,
            format!(
                "legacy must be a sequence of non-empty strings; got {}",
                yaml_kind(value)
            ),
        ));
    };
    patterns
        .iter()
        .map(|pattern| {
            pattern
                .as_str()
                .filter(|pattern| !pattern.is_empty())
                .map(str::to_owned)
                .ok_or_else(|| {
                    source.diagnostic(
                        RecordDiagnosticCode::InvalidRecordType,
                        "legacy entries must be non-empty strings",
                    )
                })
        })
        .collect()
}

fn expect_mapping<'a>(
    value: &'a Value,
    path: &Path,
    record_type: &str,
    field: Option<&str>,
    code: RecordDiagnosticCode,
    description: &str,
) -> Result<&'a Mapping, RecordDiagnostic> {
    value.as_mapping().ok_or_else(|| {
        RecordDiagnostic::config(
            path,
            Some(record_type),
            field,
            code,
            format!(
                "{description} must be a mapping; got {}",
                yaml_kind(value)
            ),
        )
    })
}

fn required_string(
    mapping: &Mapping,
    name: &str,
    path: &Path,
    record_type: &str,
    field: Option<&str>,
) -> Result<String, RecordDiagnostic> {
    let value = mapping_get(mapping, name).ok_or_else(|| {
        RecordDiagnostic::config(
            path,
            Some(record_type),
            field,
            RecordDiagnosticCode::MissingRecordProperty,
            format!("{name} is required"),
        )
    })?;
    let Some(text) = value.as_str() else {
        return Err(RecordDiagnostic::config(
            path,
            Some(record_type),
            field,
            RecordDiagnosticCode::InvalidFieldOption,
            format!(
                "{name} must be a non-empty string; got {}",
                yaml_kind(value)
            ),
        ));
    };
    if text.trim().is_empty() {
        return Err(RecordDiagnostic::config(
            path,
            Some(record_type),
            field,
            RecordDiagnosticCode::InvalidFieldOption,
            format!("{name} must be a non-empty string"),
        ));
    }
    Ok(text.to_owned())
}

fn optional_string(
    mapping: &Mapping,
    name: &str,
    path: &Path,
    record_type: &str,
    field: Option<&str>,
) -> Result<Option<String>, RecordDiagnostic> {
    let Some(value) = mapping_get(mapping, name) else {
        return Ok(None);
    };
    let Some(text) = value.as_str() else {
        return Err(RecordDiagnostic::config(
            path,
            Some(record_type),
            field,
            RecordDiagnosticCode::InvalidFieldOption,
            format!(
                "{name} must be a non-empty string; got {}",
                yaml_kind(value)
            ),
        ));
    };
    if text.trim().is_empty() {
        return Err(RecordDiagnostic::config(
            path,
            Some(record_type),
            field,
            RecordDiagnosticCode::InvalidFieldOption,
            format!("{name} must be a non-empty string"),
        ));
    }
    Ok(Some(text.to_owned()))
}

fn optional_bool(
    mapping: &Mapping,
    name: &str,
    default: bool,
    path: &Path,
    record_type: &str,
    field: Option<&str>,
) -> Result<bool, RecordDiagnostic> {
    let Some(value) = mapping_get(mapping, name) else {
        return Ok(default);
    };
    value.as_bool().ok_or_else(|| {
        RecordDiagnostic::config(
            path,
            Some(record_type),
            field,
            RecordDiagnosticCode::InvalidFieldOption,
            format!("{name} must be a boolean; got {}", yaml_kind(value)),
        )
    })
}

fn reject_unknown_keys(
    mapping: &Mapping,
    allowed: &[&str],
    source: &RecordSchemaSource<'_>,
    description: &str,
) -> Result<(), RecordDiagnostic> {
    for key in mapping.keys() {
        let Some(key) = key.as_str() else {
            return Err(source.diagnostic(
                if source.field.is_some() {
                    RecordDiagnosticCode::InvalidFieldOption
                } else {
                    RecordDiagnosticCode::InvalidRecordType
                },
                format!(
                    "{description} keys must be strings; got {}",
                    yaml_kind(key)
                ),
            ));
        };
        if !allowed.contains(&key) {
            let mut diagnostic = source.at(key).diagnostic(
                if source.field.is_some() {
                    RecordDiagnosticCode::InvalidFieldOption
                } else {
                    RecordDiagnosticCode::InvalidRecordType
                },
                format!("unknown {description} key {key:?}"),
            );
            if matches!(source.kind, SchemaSourceKind::Album { .. }) {
                diagnostic.suggestions = spelling_suggestions(key, allowed);
                diagnostic.allowed_keys = allowed
                    .iter()
                    .map(|allowed| (*allowed).to_owned())
                    .collect();
            }
            return Err(diagnostic);
        }
    }
    Ok(())
}

fn source_optional_bool(
    mapping: &Mapping,
    name: &str,
    default: bool,
    source: &RecordSchemaSource<'_>,
) -> Result<bool, RecordDiagnostic> {
    let Some(value) = mapping_get(mapping, name) else {
        return Ok(default);
    };
    value.as_bool().ok_or_else(|| {
        source.at(name).diagnostic(
            RecordDiagnosticCode::InvalidFieldOption,
            format!("{name} must be a boolean; got {}", yaml_kind(value)),
        )
    })
}

fn source_optional_string(
    mapping: &Mapping,
    name: &str,
    source: &RecordSchemaSource<'_>,
) -> Result<Option<String>, RecordDiagnostic> {
    let Some(value) = mapping_get(mapping, name) else {
        return Ok(None);
    };
    let Some(text) = value.as_str() else {
        return Err(source.at(name).diagnostic(
            RecordDiagnosticCode::InvalidFieldOption,
            format!(
                "{name} must be a non-empty string; got {}",
                yaml_kind(value)
            ),
        ));
    };
    if text.trim().is_empty() {
        return Err(source.at(name).diagnostic(
            RecordDiagnosticCode::InvalidFieldOption,
            format!("{name} must be a non-empty string"),
        ));
    }
    Ok(Some(text.to_owned()))
}

fn source_optional_number(
    mapping: &Mapping,
    name: &str,
    source: &RecordSchemaSource<'_>,
) -> Result<Option<f64>, RecordDiagnostic> {
    let Some(value) = mapping_get(mapping, name) else {
        return Ok(None);
    };
    let parsed = value
        .as_f64()
        .filter(|number| number.is_finite())
        .ok_or_else(|| {
            source.at(name).diagnostic(
                RecordDiagnosticCode::InvalidNumberBound,
                format!(
                    "{name} must be a finite number; got {}",
                    yaml_kind(value)
                ),
            )
        })?;
    Ok(Some(parsed))
}

fn spelling_suggestions(key: &str, allowed: &[&str]) -> Vec<String> {
    let mut candidates = allowed
        .iter()
        .filter_map(|candidate| {
            let distance = edit_distance(key, candidate);
            (distance <= 2).then_some((distance, *candidate))
        })
        .collect::<Vec<_>>();
    candidates.sort_by(|left, right| left.cmp(right));
    candidates
        .into_iter()
        .take(3)
        .map(|(_, candidate)| candidate.to_owned())
        .collect()
}

fn edit_distance(left: &str, right: &str) -> usize {
    let mut previous = (0..=right.len()).collect::<Vec<_>>();
    let mut current = vec![0; right.len() + 1];
    for (left_index, left_byte) in left.bytes().enumerate() {
        current[0] = left_index + 1;
        for (right_index, right_byte) in right.bytes().enumerate() {
            current[right_index + 1] = (previous[right_index + 1] + 1)
                .min(current[right_index] + 1)
                .min(
                    previous[right_index]
                        + usize::from(left_byte != right_byte),
                );
        }
        std::mem::swap(&mut previous, &mut current);
    }
    previous[right.len()]
}

fn mapping_get<'a>(mapping: &'a Mapping, key: &str) -> Option<&'a Value> {
    mapping.get(Value::String(key.to_owned()))
}

/// Reserved nouns an album `record.noun` may never claim: today's record
/// ids plus the `album`/`record` storage spellings.
pub(crate) fn album_noun_reserved(noun: &str) -> bool {
    ALBUM_RESERVED_RECORD_IDS.contains(&noun)
}

/// The `[a-z][a-z0-9_]*` identifier alphabet shared with album creation.
pub(crate) fn valid_identifier(value: &str) -> bool {
    let bytes = value.as_bytes();
    !bytes.is_empty()
        && bytes[0].is_ascii_lowercase()
        && bytes[1..].iter().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || *byte == b'_'
        })
}

fn valid_directory(value: &str) -> bool {
    if value.is_empty()
        || value.starts_with('.')
        || value.contains('/')
        || value.contains('\\')
        || value.chars().any(char::is_control)
        || matches!(value.to_lowercase().as_str(), "done" | "ref")
    {
        return false;
    }
    if value.eq_ignore_ascii_case(".bob") {
        return false;
    }
    let mut components = Path::new(value).components();
    matches!(components.next(), Some(std::path::Component::Normal(_)))
        && components.next().is_none()
}

fn title_case_field(key: &str) -> String {
    key.split('_')
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            chars
                .next()
                .map(|first| {
                    first.to_uppercase().chain(chars).collect::<String>()
                })
                .unwrap_or_default()
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn yaml_kind(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Sequence(_) => "sequence",
        Value::Mapping(_) => "mapping",
        Value::Tagged(_) => "tagged value",
    }
}

impl PropertyStorageKind {
    fn display(self) -> &'static str {
        match self {
            Self::String => "string",
            Self::Date => "date",
            Self::Number => "number",
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{collections::BTreeSet, path::Path};

    use chrono::NaiveDate;

    use super::{
        parse_records_config, validate_field_input, validate_field_values,
        FieldInputState, FieldKind, RecordDate, RecordDiagnostic,
        RecordDiagnosticCode, RecordField, RecordScalar, RecordValue,
        RecordsConfig, ValidatedFieldInput,
    };

    const CONFIG: &str = "/config.yml";
    const TODAY: NaiveDate = match NaiveDate::from_ymd_opt(2026, 11, 8) {
        Some(date) => date,
        None => panic!("test date is valid"),
    };

    fn parse(text: &str) -> Result<RecordsConfig, super::RecordDiagnostic> {
        parse_records_config(text, Path::new(CONFIG))
    }

    fn one_field(field_yaml: &str) -> String {
        format!(
            "records:\n  sample:\n    dir: sample\n    label: Sample\n    fields:\n      value: {field_yaml}\n"
        )
    }

    fn field(
        key: &str,
        kind: FieldKind,
        list: bool,
        min: Option<f64>,
        max: Option<f64>,
    ) -> RecordField {
        RecordField {
            key: key.to_owned(),
            label: key.to_owned(),
            kind,
            required: false,
            list,
            default: None,
            suggest: false,
            min,
            max,
        }
    }

    #[test]
    fn diagnostic_display_keeps_config_value_and_album_context() {
        let config = RecordDiagnostic::config(
            Path::new(CONFIG),
            Some("restaurant"),
            Some("status"),
            RecordDiagnosticCode::InvalidChoice,
            "unknown choice",
        );
        assert_eq!(
            config.to_string(),
            "records config in /config.yml, type restaurant, field status: unknown choice"
        );

        let value = RecordDiagnostic::value(
            &field("status", FieldKind::Text, false, None, None),
            RecordDiagnosticCode::InvalidChoice,
            "unknown choice",
        );
        assert_eq!(
            value.to_string(),
            "record value, field status: unknown choice"
        );

        let note = RecordDiagnostic::note(
            Some("title"),
            RecordDiagnosticCode::MissingTitle,
            "record is missing a title",
        );
        assert_eq!(
            note.to_string(),
            "record value, field title: record is missing a title"
        );

        let album = RecordDiagnostic::album(
            Path::new("eat.md"),
            8,
            "record.fields.location.requred",
            Some("restaurant"),
            RecordDiagnosticCode::InvalidFieldOption,
            "unknown option requred",
        );
        assert_eq!(
            album.to_string(),
            "eat.md:8 record.fields.location.requred: unknown option requred"
        );
    }

    #[test]
    fn absent_and_empty_records_blocks_are_valid() {
        assert!(parse("plan: {}\n").unwrap().types.is_empty());
        assert!(parse("records: {}\n").unwrap().types.is_empty());
    }

    #[test]
    fn invalid_config_rules_are_reported_with_context() {
        let cases = [
            ("null records block", "records: null\n", RecordDiagnosticCode::InvalidRecordsBlock),
            ("scalar records block", "records: invalid\n", RecordDiagnosticCode::InvalidRecordsBlock),
            ("sequence records block", "records: []\n", RecordDiagnosticCode::InvalidRecordsBlock),
            ("invalid type id", "records:\n  Bad-Id:\n    dir: bad\n    label: Bad\n    fields: {}\n", RecordDiagnosticCode::InvalidRecordId),
            ("reserved type id", "records:\n  project:\n    dir: projects\n    label: Project\n    fields: {}\n", RecordDiagnosticCode::ReservedRecordId),
            ("type definition is not mapping", "records:\n  sample: []\n", RecordDiagnosticCode::InvalidRecordType),
            ("unknown type option", "records:\n  sample:\n    dir: sample\n    label: Sample\n    ignored: true\n    fields: {}\n", RecordDiagnosticCode::InvalidRecordType),
            ("missing directory", "records:\n  sample:\n    label: Sample\n    fields: {}\n", RecordDiagnosticCode::MissingRecordProperty),
            ("dot folder", "records:\n  sample:\n    dir: .hidden\n    label: Sample\n    fields: {}\n", RecordDiagnosticCode::InvalidDirectory),
            ("bob folder", "records:\n  sample:\n    dir: .bob\n    label: Sample\n    fields: {}\n", RecordDiagnosticCode::InvalidDirectory),
            ("done folder", "records:\n  sample:\n    dir: done\n    label: Sample\n    fields: {}\n", RecordDiagnosticCode::InvalidDirectory),
            ("ref folder", "records:\n  sample:\n    dir: ref\n    label: Sample\n    fields: {}\n", RecordDiagnosticCode::InvalidDirectory),
            ("path traversal", "records:\n  sample:\n    dir: ../outside\n    label: Sample\n    fields: {}\n", RecordDiagnosticCode::InvalidDirectory),
            ("slash in directory", "records:\n  sample:\n    dir: eat/food\n    label: Sample\n    fields: {}\n", RecordDiagnosticCode::InvalidDirectory),
            ("backslash in directory", "records:\n  sample:\n    dir: eat\\\\food\n    label: Sample\n    fields: {}\n", RecordDiagnosticCode::InvalidDirectory),
            ("missing label", "records:\n  sample:\n    dir: sample\n    fields: {}\n", RecordDiagnosticCode::MissingRecordProperty),
            ("missing fields", "records:\n  sample:\n    dir: sample\n    label: Sample\n", RecordDiagnosticCode::MissingRecordProperty),
            ("fields is not mapping", "records:\n  sample:\n    dir: sample\n    label: Sample\n    fields: []\n", RecordDiagnosticCode::InvalidField),
            ("invalid field name", "records:\n  sample:\n    dir: sample\n    label: Sample\n    fields:\n      Bad-Field: {}\n", RecordDiagnosticCode::InvalidFieldName),
            ("reserved field name", "records:\n  sample:\n    dir: sample\n    label: Sample\n    fields:\n      title: {}\n", RecordDiagnosticCode::ReservedFieldName),
            ("field definition is not mapping", "records:\n  sample:\n    dir: sample\n    label: Sample\n    fields:\n      value: text\n", RecordDiagnosticCode::InvalidField),
            ("unknown field option", "records:\n  sample:\n    dir: sample\n    label: Sample\n    fields:\n      value: {wat: true}\n", RecordDiagnosticCode::InvalidFieldOption),
            ("empty choices", "records:\n  sample:\n    dir: sample\n    label: Sample\n    fields:\n      value: {values: {}}\n", RecordDiagnosticCode::InvalidChoice),
            ("unsupported kind", "records:\n  sample:\n    dir: sample\n    label: Sample\n    fields:\n      value: {values: boolean}\n", RecordDiagnosticCode::InvalidFieldKind),
            ("nonmapping values", "records:\n  sample:\n    dir: sample\n    label: Sample\n    fields:\n      value: {values: []}\n", RecordDiagnosticCode::InvalidFieldKind),
            ("empty choice key", "records:\n  sample:\n    dir: sample\n    label: Sample\n    fields:\n      value: {values: {'': Empty}}\n", RecordDiagnosticCode::InvalidChoice),
            ("empty choice label", "records:\n  sample:\n    dir: sample\n    label: Sample\n    fields:\n      value: {values: {one: '  '}}\n", RecordDiagnosticCode::InvalidChoice),
            ("nonstring choice label", "records:\n  sample:\n    dir: sample\n    label: Sample\n    fields:\n      value: {values: {one: false}}\n", RecordDiagnosticCode::InvalidChoice),
            ("nonboolean required", "records:\n  sample:\n    dir: sample\n    label: Sample\n    fields:\n      value: {required: yes}\n", RecordDiagnosticCode::InvalidFieldOption),
            ("nonboolean list", "records:\n  sample:\n    dir: sample\n    label: Sample\n    fields:\n      value: {list: 1}\n", RecordDiagnosticCode::InvalidFieldOption),
            ("blank label override", "records:\n  sample:\n    dir: sample\n    label: Sample\n    fields:\n      value: {label: ''}\n", RecordDiagnosticCode::InvalidFieldOption),
            ("invalid default kind", "records:\n  sample:\n    dir: sample\n    label: Sample\n    fields:\n      value: {default: false}\n", RecordDiagnosticCode::InvalidDefault),
            ("null default", "records:\n  sample:\n    dir: sample\n    label: Sample\n    fields:\n      value: {default: null}\n", RecordDiagnosticCode::InvalidDefault),
            ("scalar list default", "records:\n  sample:\n    dir: sample\n    label: Sample\n    fields:\n      value: {list: true, default: item}\n", RecordDiagnosticCode::InvalidDefault),
            ("list default on scalar", "records:\n  sample:\n    dir: sample\n    label: Sample\n    fields:\n      value: {default: []}\n", RecordDiagnosticCode::InvalidDefault),
            ("status default", "records:\n  sample:\n    dir: sample\n    label: Sample\n    fields:\n      status: {values: {ready: Ready}, default: ready}\n", RecordDiagnosticCode::InvalidDefault),
            ("bad choice default", "records:\n  sample:\n    dir: sample\n    label: Sample\n    fields:\n      value: {values: {one: One}, default: missing}\n", RecordDiagnosticCode::InvalidDefault),
            ("bad date default", "records:\n  sample:\n    dir: sample\n    label: Sample\n    fields:\n      value: {values: date, default: 2026-02-30}\n", RecordDiagnosticCode::InvalidDefault),
            ("string number default", "records:\n  sample:\n    dir: sample\n    label: Sample\n    fields:\n      value: {values: number, default: '2'}\n", RecordDiagnosticCode::InvalidDefault),
            ("nonnumber bound", "records:\n  sample:\n    dir: sample\n    label: Sample\n    fields:\n      value: {min: 1}\n", RecordDiagnosticCode::InvalidNumberBound),
            ("nonnumeric bound", "records:\n  sample:\n    dir: sample\n    label: Sample\n    fields:\n      value: {values: number, min: nope}\n", RecordDiagnosticCode::InvalidNumberBound),
            ("inverted bounds", "records:\n  sample:\n    dir: sample\n    label: Sample\n    fields:\n      value: {values: number, min: 3, max: 2}\n", RecordDiagnosticCode::InvalidNumberBound),
            ("invalid legacy shape", "records:\n  sample:\n    dir: sample\n    label: Sample\n    fields: {}\n    legacy: source_*\n", RecordDiagnosticCode::InvalidRecordType),
        ];

        for (name, text, expected_code) in cases {
            let error = parse(text).unwrap_err();
            assert_eq!(error.code, expected_code, "case: {name}; got {error}");
            assert!(
                error.to_string().contains(CONFIG),
                "case: {name}; {error}"
            );
            if text.contains("  sample:") {
                assert_eq!(
                    error.record_type.as_deref(),
                    Some("sample"),
                    "case: {name}"
                );
            }
            if text.contains("      value:") || text.contains("      status:") {
                assert!(error.field.is_some(), "case: {name}; {error}");
            }
        }
    }

    #[test]
    fn directories_are_unique_under_case_folding_and_safe_segments_only() {
        let duplicate = "records:\n  first:\n    dir: Eat\n    label: First\n    fields: {}\n  second:\n    dir: eat\n    label: Second\n    fields: {}\n";
        let error = parse(duplicate).unwrap_err();
        assert_eq!(error.code, RecordDiagnosticCode::DuplicateDirectory);
        assert_eq!(error.record_type.as_deref(), Some("second"));
        assert!(error.message.contains("first"));
    }

    #[test]
    fn reserved_identifiers_and_boolean_options_are_checked_exhaustively() {
        for id in [
            "project", "area", "ref", "day", "done", "mail", "article", "type",
        ] {
            let text = format!(
                "records:\n  {id}:\n    dir: sample\n    label: Sample\n    fields: {{}}\n"
            );
            assert_eq!(
                parse(&text).unwrap_err().code,
                RecordDiagnosticCode::ReservedRecordId,
                "{id}"
            );
        }
        for field in ["parent", "type", "title", "created"] {
            let text = format!(
                "records:\n  sample:\n    dir: sample\n    label: Sample\n    fields:\n      {field}: {{}}\n"
            );
            assert_eq!(
                parse(&text).unwrap_err().code,
                RecordDiagnosticCode::ReservedFieldName,
                "{field}"
            );
        }
        for invalid in [
            "records:\n  sample:\n    dir: sample\n    label: Sample\n    dated_notes: yes\n    fields: {}\n",
            "records:\n  sample:\n    dir: sample\n    label: Sample\n    emoji: 3\n    fields: {}\n",
            "records:\n  sample:\n    dir: sample\n    label: Sample\n    parent: []\n    fields: {}\n",
            "records:\n  sample:\n    dir: sample\n    label: Sample\n    fields:\n      value: {required: false, list: false, suggest: maybe}\n",
        ] {
            assert_eq!(parse(invalid).unwrap_err().code, RecordDiagnosticCode::InvalidFieldOption);
        }
    }

    #[test]
    fn shared_property_storage_must_agree_but_choice_sets_may_differ() {
        let compatible = "records:\n  restaurant:\n    dir: eat\n    label: Restaurant\n    fields:\n      status: {values: {liked: Liked, not_liked: Not liked}}\n  podcast:\n    dir: podcasts\n    label: Podcast\n    fields:\n      status: {values: {subscribed: Subscribed, unsubscribed: Unsubscribed}}\n";
        let config = parse(compatible).unwrap();
        let statuses = config
            .types
            .iter()
            .map(|kind| kind.fields[0].key.as_str())
            .collect::<Vec<_>>();
        assert_eq!(statuses, ["status", "status"]);

        for fields in [("{values: date}", "{}"), ("{list: true}", "{}")] {
            let first = fields.0;
            let second = fields.1;
            let text = format!(
                "records:\n  first:\n    dir: first\n    label: First\n    fields:\n      shared: {first}\n  second:\n    dir: second\n    label: Second\n    fields:\n      shared: {second}\n"
            );
            let error = parse(&text).unwrap_err();
            assert_eq!(
                error.code,
                RecordDiagnosticCode::IncompatiblePropertyStorage
            );
        }
    }

    #[test]
    fn shipped_restaurant_podcast_show_and_movie_shapes_keep_schema_order() {
        let text = r#"
records:
  restaurant:
    dir: eat
    label: Restaurant
    emoji: 🍽️
    dated_notes: true
    fields:
      status:
        required: true
        values: {to_try: 🔖 To try, liked: 💚 Liked, not_liked: 🚫 Not liked}
      category: {required: true, suggest: true}
      location: {required: true, suggest: true}
      companions: {values: link, list: true, default: ["[[kelly]]"]}
      visited: {values: date}
      url: {values: url}
    legacy: [lid, source_*]
  podcast:
    dir: podcasts
    label: Podcast
    emoji: 🎙️
    fields:
      status:
        required: true
        values: {to_try: 🔖 To try, subscribed: 🎧 Subscribed, unsubscribed: 🚫 Unsubscribed}
      category: {required: true, suggest: true}
      url: {values: url}
    legacy: [zorg_id, source_*]
  show:
    dir: tv
    label: TV show
    emoji: 📺
    dated_notes: true
    fields:
      status:
        required: true
        values: {to_watch: 🔖 To watch, watching: ▶️ Watching, watched: ✅ Watched, dropped: 🚫 Dropped}
      service: {suggest: true}
      recommended_by: {suggest: true}
      verdict:
        values: {loved: 😍 Loved, liked: 💚 Liked, meh: 😐 Meh, disliked: 👎 Disliked}
      companions: {values: link, list: true, default: ["[[kelly]]"]}
      watched_on: {values: date}
      url: {values: url}
  movie:
    dir: movies
    label: Movie
    emoji: 🎬
    dated_notes: true
    fields:
      status: {required: true, values: {to_watch: 🔖 To watch, watched: ✅ Watched}}
      year: {values: number, min: 1888, max: 2100}
      service: {suggest: true}
      recommended_by: {suggest: true}
      verdict:
        values: {loved: 😍 Loved, liked: 💚 Liked, meh: 😐 Meh, disliked: 👎 Disliked}
      companions: {values: link, list: true, default: ["[[kelly]]"]}
      watched_on: {values: date}
      url: {values: url}
"#;
        let config = parse(text).unwrap();
        assert_eq!(
            config
                .types
                .iter()
                .map(|kind| kind.id.as_str())
                .collect::<Vec<_>>(),
            ["restaurant", "podcast", "show", "movie"]
        );
        assert_eq!(
            config
                .record_type("restaurant")
                .unwrap()
                .fields
                .iter()
                .map(|field| field.key.as_str())
                .collect::<Vec<_>>(),
            [
                "status",
                "category",
                "location",
                "companions",
                "visited",
                "url"
            ]
        );
        let status = &config.record_type("restaurant").unwrap().fields[0];
        let FieldKind::Choices(choices) = &status.kind else {
            panic!("status is choices")
        };
        assert_eq!(
            choices
                .iter()
                .map(|choice| choice.value.as_str())
                .collect::<Vec<_>>(),
            ["to_try", "liked", "not_liked"]
        );
        assert!(config.record_type("show").unwrap().dated_notes);
        assert_eq!(
            config.record_type("restaurant").unwrap().legacy,
            ["lid", "source_*"]
        );
    }

    #[test]
    fn defaults_preserve_zero_empty_lists_and_dynamic_dates() {
        let text = r#"
records:
  sample:
    dir: sample
    label: Sample
    fields:
      count: {values: number, min: 0, max: 5, default: 0}
      links: {values: link, list: true, default: []}
      date: {values: date, default: today}
"#;
        let config = parse(text).unwrap();
        let kind = config.record_type("sample").unwrap();
        assert_eq!(
            kind.field("count").unwrap().default,
            Some(RecordValue::Scalar(RecordScalar::Number(0.0)))
        );
        assert_eq!(
            kind.field("links").unwrap().default,
            Some(RecordValue::List(vec![]))
        );
        assert_eq!(
            kind.field("date").unwrap().default,
            Some(RecordValue::Scalar(RecordScalar::Date(RecordDate::Today)))
        );
    }

    #[test]
    fn every_fixture_title_matches_the_algorithm_except_the_frozen_override_set(
    ) {
        let rows =
            include_str!("../../../tests/fixtures/records/name-paths.tsv");
        let pairs = rows.lines().collect::<Vec<_>>();
        assert_eq!(pairs.len(), 107);
        assert_eq!(
            pairs.iter().filter(|line| line.starts_with("eat/")).count(),
            92
        );
        assert_eq!(
            pairs
                .iter()
                .filter(|line| line.starts_with("podcasts/"))
                .count(),
            15
        );

        let expected = [
            "eat/andgrain.md",
            "podcasts/ai_daily_brief.md",
            "podcasts/corepy.md",
            "podcasts/gtd_podcast.md",
            "podcasts/productivity_cast.md",
            "podcasts/talk_python.md",
            "podcasts/techmeme.md",
            "podcasts/the_stackoverflow_podcast.md",
            "podcasts/twit.md",
        ]
        .into_iter()
        .collect::<BTreeSet<_>>();
        let mut mismatches = BTreeSet::new();
        for line in pairs {
            let (path, title) =
                line.split_once('\t').expect("fixture has path and title");
            let actual =
                path.strip_suffix(".md").expect("fixture path is markdown");
            let generated = super::super::stem::normalize_stem(title)
                .unwrap_or_else(|| panic!("{path} needs an explicit stem"));
            if actual
                != format!("{}/{generated}", path.split('/').next().unwrap())
            {
                mismatches.insert(path);
            }
        }
        assert_eq!(mismatches, expected);
    }

    #[test]
    fn value_validation_covers_text_choices_dates_links_urls_and_numbers() {
        let text = field("value", FieldKind::Text, false, None, None);
        for literal in ["false", "0", "%", "#", "@", "[[target#^block]]"] {
            assert_eq!(
                validate_field_values(&text, &[literal.to_owned()], TODAY)
                    .unwrap(),
                RecordValue::Scalar(RecordScalar::Text(literal.to_owned()))
            );
        }

        let choices = field(
            "status",
            FieldKind::Choices(vec![super::RecordChoice {
                value: "liked".into(),
                label: "Liked".into(),
            }]),
            false,
            None,
            None,
        );
        assert_eq!(
            validate_field_values(&choices, &["liked".into()], TODAY).unwrap(),
            RecordValue::Scalar(RecordScalar::Choice("liked".into()))
        );
        assert_eq!(
            validate_field_values(&choices, &["Liked".into()], TODAY)
                .unwrap_err()
                .code,
            RecordDiagnosticCode::InvalidChoice
        );

        let date = field("date", FieldKind::Date, false, None, None);
        assert_eq!(
            validate_field_values(&date, &["today".into()], TODAY).unwrap(),
            RecordValue::Scalar(RecordScalar::Date(RecordDate::Resolved(
                TODAY
            )))
        );
        assert_eq!(
            validate_field_values(&date, &["yesterday".into()], TODAY).unwrap(),
            RecordValue::Scalar(RecordScalar::Date(RecordDate::Resolved(
                TODAY.pred_opt().unwrap()
            )))
        );
        assert_eq!(
            validate_field_values(&date, &["2024-02-29".into()], TODAY)
                .unwrap(),
            RecordValue::Scalar(RecordScalar::Date(RecordDate::Resolved(
                NaiveDate::from_ymd_opt(2024, 2, 29).unwrap()
            )))
        );
        for invalid in ["2026-02-29", "2026-2-03", "tomorrow"] {
            assert_eq!(
                validate_field_values(&date, &[invalid.into()], TODAY)
                    .unwrap_err()
                    .code,
                RecordDiagnosticCode::InvalidDate
            );
        }

        let link = field("link", FieldKind::Link, false, None, None);
        assert_eq!(
            validate_field_values(&link, &["Kelly Smith".into()], TODAY)
                .unwrap(),
            RecordValue::Scalar(RecordScalar::Link("[[Kelly Smith]]".into()))
        );
        assert_eq!(
            validate_field_values(
                &link,
                &["[[nj_towns#^z-241117-0b|cranford]]".into()],
                TODAY
            )
            .unwrap(),
            RecordValue::Scalar(RecordScalar::Link(
                "[[nj_towns#^z-241117-0b|cranford]]".into()
            ))
        );
        for invalid in ["[[incomplete", "[[|alias]]", "[[target|]]", "[target]"]
        {
            assert_eq!(
                validate_field_values(&link, &[invalid.into()], TODAY)
                    .unwrap_err()
                    .code,
                RecordDiagnosticCode::InvalidLink
            );
        }

        let url = field("url", FieldKind::Url, false, None, None);
        assert!(validate_field_values(
            &url,
            &["https://example.com/a".into()],
            TODAY
        )
        .is_ok());
        for invalid in ["ftp://example.com", "https://", "/relative"] {
            assert_eq!(
                validate_field_values(&url, &[invalid.into()], TODAY)
                    .unwrap_err()
                    .code,
                RecordDiagnosticCode::InvalidUrl
            );
        }

        let number =
            field("year", FieldKind::Number, false, Some(1888.0), Some(2100.0));
        assert_eq!(
            validate_field_values(&number, &["1888".into()], TODAY).unwrap(),
            RecordValue::Scalar(RecordScalar::Number(1888.0))
        );
        assert_eq!(
            validate_field_values(&number, &["2100.5".into()], TODAY)
                .unwrap_err()
                .code,
            RecordDiagnosticCode::NumberOutOfRange
        );
        for invalid in ["NaN", "inf", "1e3", "1.", "--2"] {
            assert_eq!(
                validate_field_values(&number, &[invalid.into()], TODAY)
                    .unwrap_err()
                    .code,
                RecordDiagnosticCode::InvalidNumber
            );
        }
    }

    #[test]
    fn list_fields_split_commas_preserve_order_and_keep_complete_wikilinks_intact(
    ) {
        let text = field("category", FieldKind::Text, true, None, None);
        assert_eq!(
            validate_field_values(
                &text,
                &["Coffee, Breakfast".into(), "Brunch".into()],
                TODAY
            )
            .unwrap(),
            RecordValue::List(vec![
                RecordScalar::Text("Coffee".into()),
                RecordScalar::Text("Breakfast".into()),
                RecordScalar::Text("Brunch".into()),
            ])
        );
        let links = field("companions", FieldKind::Link, true, None, None);
        assert_eq!(
            validate_field_values(
                &links,
                &[
                    "[[nj_towns, nj#^id|Cranford, NJ]], Kelly".into(),
                    "[[Sam]]".into()
                ],
                TODAY
            )
            .unwrap(),
            RecordValue::List(vec![
                RecordScalar::Link("[[nj_towns, nj#^id|Cranford, NJ]]".into()),
                RecordScalar::Link("[[Kelly]]".into()),
                RecordScalar::Link("[[Sam]]".into()),
            ])
        );
        for invalid in ["", "one,,two", "one,", "[[unclosed"] {
            assert_eq!(
                validate_field_values(&text, &[invalid.into()], TODAY)
                    .unwrap_err()
                    .code,
                RecordDiagnosticCode::InvalidList
            );
        }
        assert_eq!(
            validate_field_values(&text, &[], TODAY).unwrap_err().code,
            RecordDiagnosticCode::InvalidValue
        );
    }

    #[test]
    fn input_states_and_scalar_repetition_remain_explicit() {
        let field = field("name", FieldKind::Text, false, None, None);
        assert_eq!(
            validate_field_input(&field, &FieldInputState::Absent, TODAY)
                .unwrap(),
            ValidatedFieldInput::Absent
        );
        assert_eq!(
            validate_field_input(
                &field,
                &FieldInputState::SuppliedEmpty,
                TODAY
            )
            .unwrap(),
            ValidatedFieldInput::Empty
        );
        assert_eq!(
            validate_field_values(&field, &["one".into(), "two".into()], TODAY)
                .unwrap_err()
                .code,
            RecordDiagnosticCode::InvalidValue
        );
        let error =
            validate_field_values(&field, &[String::new()], TODAY).unwrap_err();
        assert_eq!(error.code, RecordDiagnosticCode::InvalidValue);
        assert_eq!(error.field.as_deref(), Some("name"));
    }
}
