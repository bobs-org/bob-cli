//! Validation of configured Record Note folders.

use std::collections::{BTreeMap, BTreeSet};

use chrono::NaiveDate;
use serde::Serialize;

use super::{
    catalog::{FolderIndex, IndexedFieldValue, IndexedRecord},
    schema::{
        validate_field_input, FieldInputState, RecordDiagnostic, RecordField,
        RecordType, ValidatedFieldInput,
    },
};

#[derive(Debug, Clone, Serialize)]
pub(crate) struct Finding {
    pub(crate) severity: &'static str,
    pub(crate) code: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) file: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) field: Option<String>,
    pub(crate) message: String,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct TypeReport {
    pub(crate) id: String,
    pub(crate) label: String,
    pub(crate) dir: String,
    pub(crate) count: usize,
    pub(crate) errors: usize,
    pub(crate) warnings: usize,
    pub(crate) findings: Vec<Finding>,
}

impl TypeReport {
    pub(crate) fn new(record_type: &RecordType, folder: &FolderIndex) -> Self {
        let mut report = Self {
            id: record_type.id.clone(),
            label: record_type.label.clone(),
            dir: record_type.dir.clone(),
            count: folder.notes().len(),
            errors: 0,
            warnings: 0,
            findings: Vec::new(),
        };
        for diagnostic in folder.diagnostics() {
            report.push_diagnostic(None, diagnostic, "error");
        }
        report
    }

    pub(crate) fn check_note(
        &mut self,
        record_type: &RecordType,
        note: &IndexedRecord,
        today: NaiveDate,
    ) {
        let file = note.relative_path.display().to_string();
        for diagnostic in &note.diagnostics {
            self.push_diagnostic(Some(&file), diagnostic, "error");
        }

        let expected_type = format!("[[{}]]", record_type.id);
        match note.field("type") {
            Some(IndexedFieldValue::Scalar(value))
                if value == &expected_type => {}
            Some(_) => self.push(
                "error",
                "record_wrong_type",
                Some(&file),
                Some("type"),
                format!("expected type [[{}]]", record_type.id),
            ),
            None => self.push(
                "error",
                "record_missing_type",
                Some(&file),
                Some("type"),
                format!("missing type [[{}]]", record_type.id),
            ),
        }

        for field in &record_type.fields {
            match field_state(note, field) {
                FieldInputState::Absent if field.required => self.push(
                    "error",
                    "record_missing_field",
                    Some(&file),
                    Some(&field.key),
                    format!("missing required field {}", field.key),
                ),
                state => match validate_field_input(field, &state, today) {
                    Ok(ValidatedFieldInput::Absent) => {}
                    Ok(ValidatedFieldInput::Empty) if field.required => self
                        .push(
                            "error",
                            "record_missing_field",
                            Some(&file),
                            Some(&field.key),
                            format!("required field {} is empty", field.key),
                        ),
                    Ok(
                        ValidatedFieldInput::Empty
                        | ValidatedFieldInput::Value(_),
                    ) => {}
                    Err(diagnostic) => {
                        self.push_diagnostic(Some(&file), &diagnostic, "error")
                    }
                },
            }
        }

        for field in &note.fields {
            if matches!(
                field.key.as_str(),
                "parent" | "type" | "title" | "created"
            ) || record_type.field(&field.key).is_some()
                || legacy_field(&field.key, &record_type.legacy)
            {
                continue;
            }
            self.push(
                "error",
                "record_unknown_field",
                Some(&file),
                Some(&field.key),
                format!("unknown field {}", field.key),
            );
        }
    }

    pub(crate) fn check_suggested_values(
        &mut self,
        record_type: &RecordType,
        folder: &FolderIndex,
    ) {
        for field in record_type.fields.iter().filter(|field| field.suggest) {
            let mut values: BTreeMap<String, (BTreeSet<String>, String)> =
                BTreeMap::new();
            for note in folder.notes() {
                let Some(value) = note.field(&field.key) else {
                    continue;
                };
                let members: Vec<&str> = match value {
                    IndexedFieldValue::Scalar(value) => vec![value],
                    IndexedFieldValue::List(values) => {
                        values.iter().map(String::as_str).collect()
                    }
                    IndexedFieldValue::Null
                    | IndexedFieldValue::Unsupported => Vec::new(),
                };
                for member in members {
                    let display = suggestion_display(member);
                    let fold = display.to_lowercase();
                    let entry = values.entry(fold).or_insert_with(|| {
                        (
                            BTreeSet::new(),
                            note.relative_path.display().to_string(),
                        )
                    });
                    entry.0.insert(member.to_owned());
                }
            }
            for (_, (spellings, file)) in values {
                if spellings.len() < 2 {
                    continue;
                }
                self.push(
                    "warning",
                    "record_near_duplicate_value",
                    Some(&file),
                    Some(&field.key),
                    format!(
                        "{} has equivalent spellings: {}",
                        field.label,
                        spellings.into_iter().collect::<Vec<_>>().join(" · ")
                    ),
                );
            }
        }
    }

    pub(crate) fn push_diagnostic(
        &mut self,
        file: Option<&str>,
        diagnostic: &RecordDiagnostic,
        severity: &'static str,
    ) {
        self.push(
            severity,
            diagnostic.diagnostic_code(),
            file,
            diagnostic.field.as_deref(),
            diagnostic.message.clone(),
        );
    }

    pub(crate) fn push(
        &mut self,
        severity: &'static str,
        code: &'static str,
        file: Option<&str>,
        field: Option<&str>,
        message: impl Into<String>,
    ) {
        match severity {
            "warning" => self.warnings += 1,
            _ => self.errors += 1,
        }
        self.findings.push(Finding {
            severity,
            code,
            file: file.map(str::to_owned),
            field: field.map(str::to_owned),
            message: message.into(),
        });
    }
}

fn suggestion_display(original: &str) -> String {
    let trimmed = original.trim();
    if let Some(inner) = trimmed
        .strip_prefix("[[")
        .and_then(|rest| rest.strip_suffix("]]"))
        && !inner.contains("[[")
    {
        crate::native::capture_block_ids::wikilink_phrase(inner)
    } else {
        original.to_owned()
    }
}

fn field_state(note: &IndexedRecord, field: &RecordField) -> FieldInputState {
    match note.field(&field.key) {
        None => FieldInputState::Absent,
        Some(IndexedFieldValue::Null) => FieldInputState::SuppliedEmpty,
        Some(IndexedFieldValue::Scalar(value)) => {
            FieldInputState::SuppliedValues(vec![value.clone()])
        }
        Some(IndexedFieldValue::List(values)) => {
            FieldInputState::SuppliedValues(values.clone())
        }
        Some(IndexedFieldValue::Unsupported) => {
            FieldInputState::SuppliedValues(vec![
                "<unsupported YAML value>".to_owned()
            ])
        }
    }
}

fn legacy_field(key: &str, legacy: &[String]) -> bool {
    legacy.iter().any(|pattern| {
        if let Some(prefix) = pattern.strip_suffix('*') {
            key.starts_with(prefix)
        } else if let Some(suffix) = pattern.strip_prefix('*') {
            key.ends_with(suffix)
        } else {
            key == pattern
        }
    })
}
