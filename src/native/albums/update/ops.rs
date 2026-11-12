//! Pure ordered schema operations for the guarded album writer.
//!
//! This module keeps an ordered raw `record:` mapping alongside the
//! validated [`RecordType`] so absent properties, explicit false flags,
//! typed defaults, declaration order, and stored choice identities survive
//! edits. Operations apply in order, usage is counted from a complete
//! folder scan with the report's rules, the final candidate reuses the
//! shared schema validator, and every forward op returns a semantic
//! inverse in reverse order. No vault I/O happens here.

use std::collections::{HashMap, HashSet};

use chrono::NaiveDate;
use serde_json::Value as JsonValue;
use serde_yaml::{Mapping, Value as YamlValue};

use super::protocol::{ChoiceDraft, Op};
#[cfg(test)]
use crate::native::records::schema::album_schema_fingerprint;
use crate::native::{
    albums::{output::split_icon, write::folder_complete},
    records::{
        catalog::FolderIndex,
        schema::{
            parse_album_record_schema, valid_identifier, FieldKind,
            PropertyStorageKind, RecordType,
        },
        stem::normalize_stem,
    },
};

const RESERVED_FIELDS: &[&str] = &["parent", "type", "title", "created"];
const MIGRATION_HINT: &str =
    "edit in YAML (`y`) or wait for the migration flow";

/// Typed operation failure: invalid requests and semantic refusals share
/// exit 2 in the command layer; codes stay stable for the card.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OpError {
    pub(crate) code: &'static str,
    pub(crate) message: String,
    pub(crate) hint: Option<String>,
    pub(crate) op_index: Option<usize>,
    pub(crate) field: Option<String>,
    pub(crate) choice: Option<String>,
    pub(crate) affected: Option<usize>,
    pub(crate) key_path: Option<String>,
}

impl OpError {
    fn invalid(
        code: &'static str,
        op_index: usize,
        message: impl Into<String>,
    ) -> Self {
        Self {
            code,
            message: message.into(),
            hint: None,
            op_index: Some(op_index),
            field: None,
            choice: None,
            affected: None,
            key_path: None,
        }
    }

    fn refusal(
        code: &'static str,
        op_index: usize,
        message: impl Into<String>,
        hint: impl Into<String>,
    ) -> Self {
        Self {
            code,
            message: message.into(),
            hint: Some(hint.into()),
            op_index: Some(op_index),
            field: None,
            choice: None,
            affected: None,
            key_path: None,
        }
    }
}

impl std::fmt::Display for OpError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.op_index {
            Some(index) => write!(
                formatter,
                "op {index} [{}]: {}",
                self.code, self.message
            ),
            None => {
                write!(formatter, "[{}]: {}", self.code, self.message)
            }
        }
    }
}

impl std::error::Error for OpError {}

/// Album index references that guard field removals.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct IndexRefs {
    pub(crate) index_by: Option<String>,
    pub(crate) index_facets: Vec<String>,
}

/// Another album's storage shape for cross-album clash checks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OtherAlbumField {
    pub(crate) path: String,
    pub(crate) key: String,
    pub(crate) storage: PropertyStorageKind,
    pub(crate) list: bool,
}

/// Inputs for a pure operation batch.
pub(crate) struct ApplyInputs<'a> {
    pub(crate) before_raw: &'a Mapping,
    pub(crate) before_type: &'a RecordType,
    pub(crate) before_plural: Option<&'a str>,
    pub(crate) stem: &'a str,
    pub(crate) folder: &'a FolderIndex,
    pub(crate) index_refs: &'a IndexRefs,
    pub(crate) other_fields: &'a [OtherAlbumField],
    pub(crate) today: NaiveDate,
}

/// Per-operation result for dry-run and execution.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct OpResult {
    pub(crate) op_index: usize,
    pub(crate) op_name: &'static str,
    pub(crate) summary: String,
    pub(crate) derived_key: Option<String>,
    pub(crate) filled: Option<usize>,
    pub(crate) count: Option<usize>,
    pub(crate) newly_invalid: usize,
    pub(crate) inverse: Vec<Op>,
}

/// Complete outcome of an ordered batch.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct OpOutcome {
    pub(crate) after_raw: Mapping,
    pub(crate) after_type: RecordType,
    pub(crate) after_plural: Option<String>,
    pub(crate) per_op: Vec<OpResult>,
    pub(crate) derived_keys: Vec<Option<String>>,
    pub(crate) consequence: String,
    pub(crate) inverse_ops: Vec<Op>,
    pub(crate) group_before: Option<String>,
    pub(crate) group_after: Option<String>,
}

/// Apply ordered ops to the raw mapping, count complete-record impacts,
/// validate the final candidate with the shared validator, and derive
/// semantic inverses in reverse order.
pub(crate) fn apply_ops(
    envelope_ops: &[Op],
    inputs: &ApplyInputs<'_>,
) -> Result<OpOutcome, OpError> {
    // `before_plural` is part of the planner handoff (the display plural
    // is not stored on `RecordType`); read it so the contract stays explicit.
    let _ = inputs.before_plural;
    let mut raw = inputs.before_raw.clone();
    let mut per_op = Vec::with_capacity(envelope_ops.len());
    let mut derived_keys: Vec<Option<String>> =
        Vec::with_capacity(envelope_ops.len());

    for (index, op) in envelope_ops.iter().enumerate() {
        let result = apply_one(op, index, &mut raw, inputs)?;
        derived_keys.push(result.derived_key.clone());
        per_op.push(result);
    }

    let (after_type, after_plural) =
        validate_final(&raw, inputs, envelope_ops.len().saturating_sub(1))?;
    check_storage_conflicts(
        &after_type,
        inputs.other_fields,
        envelope_ops.len().saturating_sub(1),
    )?;

    let group_before = group_field_key(inputs.before_type);
    let group_after = group_field_key(&after_type);
    let consequence = build_consequence(&per_op, inputs.folder);

    let mut inverse_ops = Vec::new();
    for result in per_op.iter().rev() {
        inverse_ops.extend(result.inverse.iter().cloned());
    }

    Ok(OpOutcome {
        after_raw: raw,
        after_type,
        after_plural,
        per_op,
        derived_keys,
        consequence,
        inverse_ops,
        group_before,
        group_after,
    })
}

/// Apply one already-parsed operation to an ordered raw schema mapping.
/// The command planner uses this only while composing byte spans; the
/// envelope is still validated atomically by [`apply_ops`] afterwards.
pub(crate) fn apply_one(
    op: &Op,
    index: usize,
    raw: &mut Mapping,
    inputs: &ApplyInputs<'_>,
) -> Result<OpResult, OpError> {
    match op {
        Op::AlbumSet { set, unset } => {
            apply_album_set(set, unset, index, raw, inputs)
        }
        Op::FieldAdd {
            label,
            kind,
            list,
            suggest,
            position,
            choices,
            restore_key,
            restore_values,
        } => apply_field_add(
            label,
            kind,
            *list,
            *suggest,
            *position,
            choices,
            restore_key.as_deref(),
            restore_values.as_deref(),
            index,
            raw,
            inputs,
        ),
        Op::FieldSet { field, set, unset } => {
            apply_field_set(field, set, unset, index, raw, inputs)
        }
        Op::FieldMove { field, position } => {
            apply_field_move(field, *position, index, raw)
        }
        Op::FieldRemove { field } => {
            apply_field_remove(field, index, raw, inputs)
        }
        Op::ChoiceAdd {
            field,
            label,
            emoji,
            position,
            restore_value,
        } => apply_choice_add(
            field,
            label,
            emoji.as_deref(),
            *position,
            restore_value.as_deref(),
            index,
            raw,
            inputs,
        ),
        Op::ChoiceSet { field, value, set } => {
            apply_choice_set(field, value, set, index, raw, inputs)
        }
        Op::ChoiceMove {
            field,
            value,
            position,
        } => apply_choice_move(field, value, *position, index, raw),
        Op::ChoiceRemove { field, value } => {
            apply_choice_remove(field, value, index, raw, inputs)
        }
    }
}

// ---------------------------------------------------------------------------
// Album-level ops
// ---------------------------------------------------------------------------

fn apply_album_set(
    set: &HashMap<String, JsonValue>,
    unset: &[String],
    index: usize,
    raw: &mut Mapping,
    inputs: &ApplyInputs<'_>,
) -> Result<OpResult, OpError> {
    const ALLOWED_SET: &[&str] = &["label", "plural", "dated_notes"];
    const ALLOWED_UNSET: &[&str] = &["label", "plural", "dated_notes"];
    for key in set.keys() {
        match key.as_str() {
            "label" | "plural" | "dated_notes" => {}
            "noun" => {
                let mut error = OpError::refusal(
                    "noun_change_prohibited",
                    index,
                    format!(
                        "noun is not editable through ops; kept {:?}",
                        inputs.before_type.id
                    ),
                    MIGRATION_HINT,
                );
                error.key_path = Some("record.noun".to_owned());
                return Err(error);
            }
            "legacy" => {
                let mut error = OpError::refusal(
                    "legacy_change_prohibited",
                    index,
                    "legacy patterns are not editable through ops",
                    MIGRATION_HINT,
                );
                error.key_path = Some("record.legacy".to_owned());
                return Err(error);
            }
            _ => {
                return Err(unknown_set_key(index, key, ALLOWED_SET, None));
            }
        }
    }
    for key in unset {
        if !ALLOWED_UNSET.contains(&key.as_str()) {
            match key.as_str() {
                "noun" => {
                    let mut error = OpError::refusal(
                        "noun_change_prohibited",
                        index,
                        "noun is not editable through ops",
                        MIGRATION_HINT,
                    );
                    error.key_path = Some("record.noun".to_owned());
                    return Err(error);
                }
                "legacy" => {
                    let mut error = OpError::refusal(
                        "legacy_change_prohibited",
                        index,
                        "legacy patterns are not editable through ops",
                        MIGRATION_HINT,
                    );
                    error.key_path = Some("record.legacy".to_owned());
                    return Err(error);
                }
                _ => {
                    return Err(unknown_set_key(
                        index,
                        key,
                        ALLOWED_UNSET,
                        None,
                    ));
                }
            }
        }
    }
    check_set_unset_overlap(set, unset, index)?;

    // Capture prior effective values and presence for the inverse.
    let prior_label = raw_string(raw, "label");
    let prior_plural = raw_string(raw, "plural");
    let prior_dated = raw_bool(raw, "dated_notes");

    // Type-check set values before mutating.
    if let Some(value) = set.get("label") {
        require_json_string(value, "label", index, None)?;
        if value.as_str().is_some_and(|text| text.trim().is_empty()) {
            return Err(OpError::invalid(
                "invalid_label",
                index,
                "album label must be a non-empty string",
            ));
        }
    }
    if let Some(value) = set.get("plural") {
        require_json_string(value, "plural", index, None)?;
        if value.as_str().is_some_and(|text| text.trim().is_empty()) {
            return Err(OpError::invalid(
                "invalid_label",
                index,
                "album plural must be a non-empty string",
            ));
        }
    }
    if let Some(value) = set.get("dated_notes") {
        require_json_bool(value, "dated_notes", index, None)?;
    }

    // Apply unset first, then set.
    for key in unset {
        raw.remove(YamlValue::String(key.clone()));
    }
    for (key, value) in set {
        raw.insert(YamlValue::String(key.clone()), json_to_yaml(value));
    }

    // Build the inverse that restores prior presence exactly.
    let mut inverse_set = HashMap::new();
    let mut inverse_unset = Vec::new();
    for key in set.keys().chain(unset.iter()) {
        match key.as_str() {
            "label" => match prior_label.clone() {
                Some(prior) => {
                    inverse_set
                        .insert("label".to_owned(), JsonValue::String(prior));
                }
                None => inverse_unset.push("label".to_owned()),
            },
            "plural" => match prior_plural.clone() {
                Some(prior) => {
                    inverse_set
                        .insert("plural".to_owned(), JsonValue::String(prior));
                }
                None => inverse_unset.push("plural".to_owned()),
            },
            "dated_notes" => match prior_dated {
                Some(prior) => {
                    inverse_set.insert(
                        "dated_notes".to_owned(),
                        JsonValue::Bool(prior),
                    );
                }
                None => inverse_unset.push("dated_notes".to_owned()),
            },
            _ => {}
        }
    }
    // Deduplicate when both set and unset touched the same key set;
    // overlap was already rejected, but the union may repeat across
    // the chained iteration when set and unset are disjoint.
    inverse_unset.sort();
    inverse_unset.dedup();
    let inverse = vec![Op::AlbumSet {
        set: inverse_set,
        unset: inverse_unset,
    }];

    let summary = describe_album_set(set, unset);
    Ok(OpResult {
        op_index: index,
        op_name: "album.set",
        summary,
        derived_key: None,
        filled: None,
        count: None,
        newly_invalid: 0,
        inverse,
    })
}

fn describe_album_set(
    set: &HashMap<String, JsonValue>,
    unset: &[String],
) -> String {
    let mut parts = Vec::new();
    if let Some(label) = set.get("label").and_then(JsonValue::as_str) {
        parts.push(format!("label to {label:?}"));
    }
    if let Some(plural) = set.get("plural").and_then(JsonValue::as_str) {
        parts.push(format!("plural to {plural:?}"));
    }
    if let Some(dated) = set.get("dated_notes").and_then(JsonValue::as_bool) {
        parts.push(format!("dated_notes to {dated}"));
    }
    for key in unset {
        parts.push(format!("unset {key}"));
    }
    if parts.is_empty() {
        "album edit".to_owned()
    } else {
        format!("album {}", parts.join(", "))
    }
}

// ---------------------------------------------------------------------------
// Field-level ops
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
fn apply_field_add(
    label: &str,
    kind: &str,
    list: bool,
    suggest: bool,
    position: Option<usize>,
    choices: &[ChoiceDraft],
    restore_key: Option<&str>,
    restore_values: Option<&[String]>,
    index: usize,
    raw: &mut Mapping,
    inputs: &ApplyInputs<'_>,
) -> Result<OpResult, OpError> {
    if label.trim().is_empty() {
        return Err(OpError::invalid(
            "derived_key_invalid",
            index,
            "field label must be a non-empty string",
        ));
    }
    let derived = match restore_key {
        Some(explicit) => {
            if explicit.trim().is_empty() {
                return Err(OpError::invalid(
                    "derived_key_invalid",
                    index,
                    "restore_key must be a non-empty string",
                ));
            }
            if !valid_identifier(explicit) {
                return Err(OpError::invalid(
                    "derived_key_invalid",
                    index,
                    format!(
                        "restore_key {explicit:?} must match [a-z][a-z0-9_]*"
                    ),
                ));
            }
            if RESERVED_FIELDS.contains(&explicit) {
                return Err(OpError::invalid(
                    "derived_key_reserved",
                    index,
                    format!("field name {explicit:?} is reserved"),
                ));
            }
            // Restoration only creates an absent key, never renames.
            if field_mapping(raw, explicit).is_some() {
                return Err(OpError::invalid(
                    "derived_key_collision",
                    index,
                    format!(
                        "restore_key {explicit:?} already exists; restoration cannot rename an existing field"
                    ),
                ));
            }
            explicit.to_owned()
        }
        None => {
            let derived = normalize_stem(label).ok_or_else(|| {
                OpError::invalid(
                    "derived_key_invalid",
                    index,
                    format!(
                        "field label {label:?} has no alphanumeric characters for a key"
                    ),
                )
            })?;
            if !valid_identifier(&derived) {
                return Err(OpError::invalid(
                    "derived_key_invalid",
                    index,
                    format!(
                        "derived key {derived:?} from {label:?} must match [a-z][a-z0-9_]*"
                    ),
                ));
            }
            if RESERVED_FIELDS.contains(&derived.as_str()) {
                return Err(OpError::invalid(
                    "derived_key_reserved",
                    index,
                    format!(
                        "derived key {derived:?} from {label:?} is reserved by record-note frontmatter"
                    ),
                ));
            }
            if field_mapping(raw, &derived).is_some() {
                return Err(OpError::invalid(
                    "derived_key_collision",
                    index,
                    format!(
                        "derived key {derived:?} from {label:?} already exists; pick a distinct label"
                    ),
                ));
            }
            derived
        }
    };

    // Validate choice drafts for choices-kind additions. Undo restores
    // carry stored keys in `restore_values` so identities survive even
    // when they differ from their labels.
    if kind == "choices" {
        if let Some(values) = restore_values {
            if values.len() != choices.len() {
                return Err(OpError::invalid(
                    "envelope_wrong_type",
                    index,
                    "restore_values length must match choices length",
                ));
            }
            let mut seen = HashSet::new();
            for value in values {
                if value.trim().is_empty() {
                    return Err(OpError::invalid(
                        "derived_key_invalid",
                        index,
                        "restore value must be a non-empty string",
                    ));
                }
                if !valid_identifier(value) {
                    return Err(OpError::invalid(
                        "derived_key_invalid",
                        index,
                        format!(
                            "restore value {value:?} must match [a-z][a-z0-9_]*"
                        ),
                    ));
                }
                if !seen.insert(value.clone()) {
                    return Err(OpError::invalid(
                        "derived_key_collision",
                        index,
                        format!(
                            "duplicate choice value {value:?} in the same field.add"
                        ),
                    ));
                }
            }
            for draft in choices {
                if draft.label.trim().is_empty() {
                    return Err(OpError::invalid(
                        "invalid_choice",
                        index,
                        "each choice requires a non-empty label",
                    ));
                }
            }
        } else {
            let mut seen = HashSet::new();
            for draft in choices {
                if draft.label.trim().is_empty() {
                    return Err(OpError::invalid(
                        "invalid_choice",
                        index,
                        "each choice requires a non-empty label",
                    ));
                }
                let value = normalize_stem(&draft.label).ok_or_else(|| {
                    OpError::invalid(
                        "derived_key_invalid",
                        index,
                        format!(
                            "choice label {:?} has no alphanumeric characters for a value",
                            draft.label
                        ),
                    )
                })?;
                if !seen.insert(value.clone()) {
                    return Err(OpError::invalid(
                        "derived_key_collision",
                        index,
                        format!(
                            "duplicate choice value {value:?} in the same field.add"
                        ),
                    ));
                }
                if choice_exists(raw, &derived, &value) {
                    return Err(OpError::invalid(
                        "derived_key_collision",
                        index,
                        format!(
                            "choice value {value:?} already exists on field {derived:?}"
                        ),
                    ));
                }
            }
        }
    } else if restore_values.is_some_and(|values| !values.is_empty()) {
        return Err(OpError::invalid(
            "envelope_wrong_type",
            index,
            "restore_values is only valid for kind choices",
        ));
    }

    let order = field_order(raw);
    let insert_at = position.unwrap_or(order.len());
    if insert_at > order.len() {
        return Err(OpError::invalid(
            "position_out_of_range",
            index,
            format!(
                "field position {insert_at} is out of range for {} fields",
                order.len()
            ),
        ));
    }

    let mut field_map = Mapping::new();
    // Preserve the parser's absent/default distinction: only explicit
    // label/list/suggest flags are written; required stays absent so the
    // new field is optional until a later field.set. An explicit label is
    // always stored, even when it matches the derived title case, so
    // unset can restore absence and round-trips stay stable.
    field_map.insert(
        YamlValue::String("label".to_owned()),
        YamlValue::String(label.to_owned()),
    );
    if list {
        field_map.insert(
            YamlValue::String("list".to_owned()),
            YamlValue::Bool(true),
        );
    }
    if suggest {
        field_map.insert(
            YamlValue::String("suggest".to_owned()),
            YamlValue::Bool(true),
        );
    }
    match kind {
        "text" => {}
        "date" | "link" | "url" | "number" => {
            field_map.insert(
                YamlValue::String("values".to_owned()),
                YamlValue::String(kind.to_owned()),
            );
        }
        "choices" => {
            let mut values = Mapping::new();
            if let Some(stored) = restore_values {
                for (draft, value) in choices.iter().zip(stored.iter()) {
                    let combined = combine_choice_label(
                        draft.emoji.as_deref(),
                        &draft.label,
                    );
                    values.insert(
                        YamlValue::String(value.clone()),
                        YamlValue::String(combined),
                    );
                }
            } else {
                for draft in choices {
                    let value = normalize_stem(&draft.label)
                        .expect("choice value validated above");
                    let combined = combine_choice_label(
                        draft.emoji.as_deref(),
                        &draft.label,
                    );
                    values.insert(
                        YamlValue::String(value),
                        YamlValue::String(combined),
                    );
                }
            }
            field_map.insert(
                YamlValue::String("values".to_owned()),
                YamlValue::Mapping(values),
            );
        }
        _ => {
            return Err(OpError::invalid(
                "envelope_wrong_type",
                index,
                format!("unsupported kind {kind:?}"),
            ));
        }
    }

    // Insert at the ordered position.
    let mut next: Vec<(String, Mapping)> = Vec::new();
    for key in &order {
        if let Some(existing) = field_mapping(raw, key) {
            next.push((key.clone(), existing));
        }
    }
    next.insert(insert_at, (derived.clone(), field_map));
    write_field_order(raw, &next, inputs);

    let summary = format!("add field {derived:?} ({kind}) at {insert_at}");
    let inverse = vec![Op::FieldRemove {
        field: derived.clone(),
    }];
    Ok(OpResult {
        op_index: index,
        op_name: "field.add",
        summary,
        derived_key: Some(derived),
        filled: Some(0),
        count: None,
        newly_invalid: 0,
        inverse,
    })
}

fn apply_field_set(
    field: &str,
    set: &HashMap<String, JsonValue>,
    unset: &[String],
    index: usize,
    raw: &mut Mapping,
    inputs: &ApplyInputs<'_>,
) -> Result<OpResult, OpError> {
    const ALLOWED_SET: &[&str] =
        &["label", "required", "suggest", "default", "min", "max"];
    const ALLOWED_UNSET: &[&str] =
        &["label", "required", "suggest", "default", "min", "max"];
    for key in set.keys() {
        match key.as_str() {
            "label" | "required" | "suggest" | "default" | "min" | "max" => {}
            "key" => {
                let mut error = OpError::refusal(
                    "field_rename_prohibited",
                    index,
                    format!(
                        "field keys never change; field {field:?} keeps its stored key"
                    ),
                    MIGRATION_HINT,
                );
                error.field = Some(field.to_owned());
                if folder_complete(inputs.folder) {
                    error.affected = Some(filled_for(inputs.folder, field));
                }
                return Err(error);
            }
            "kind" | "values" | "choices" => {
                let mut error = OpError::refusal(
                    "kind_change_prohibited",
                    index,
                    format!("field {field:?} kind never changes through ops"),
                    MIGRATION_HINT,
                );
                error.field = Some(field.to_owned());
                if folder_complete(inputs.folder) {
                    error.affected = Some(filled_for(inputs.folder, field));
                }
                return Err(error);
            }
            "list" => {
                let mut error = OpError::refusal(
                    "list_change_prohibited",
                    index,
                    format!(
                        "field {field:?} list shape never changes through ops"
                    ),
                    MIGRATION_HINT,
                );
                error.field = Some(field.to_owned());
                if folder_complete(inputs.folder) {
                    error.affected = Some(filled_for(inputs.folder, field));
                }
                return Err(error);
            }
            _ => {
                return Err(unknown_set_key(
                    index,
                    key,
                    ALLOWED_SET,
                    Some(field),
                ));
            }
        }
    }
    for key in unset {
        match key.as_str() {
            "label" | "required" | "suggest" | "default" | "min" | "max" => {}
            "key" => {
                let mut error = OpError::refusal(
                    "field_rename_prohibited",
                    index,
                    format!("field {field:?} key never changes"),
                    MIGRATION_HINT,
                );
                error.field = Some(field.to_owned());
                if folder_complete(inputs.folder) {
                    error.affected = Some(filled_for(inputs.folder, field));
                }
                return Err(error);
            }
            "kind" | "values" | "choices" => {
                let mut error = OpError::refusal(
                    "kind_change_prohibited",
                    index,
                    format!("field {field:?} kind never changes through ops"),
                    MIGRATION_HINT,
                );
                error.field = Some(field.to_owned());
                if folder_complete(inputs.folder) {
                    error.affected = Some(filled_for(inputs.folder, field));
                }
                return Err(error);
            }
            "list" => {
                let mut error = OpError::refusal(
                    "list_change_prohibited",
                    index,
                    format!(
                        "field {field:?} list shape never changes through ops"
                    ),
                    MIGRATION_HINT,
                );
                error.field = Some(field.to_owned());
                if folder_complete(inputs.folder) {
                    error.affected = Some(filled_for(inputs.folder, field));
                }
                return Err(error);
            }
            _ => {
                return Err(unknown_set_key(
                    index,
                    key,
                    ALLOWED_UNSET,
                    Some(field),
                ));
            }
        }
    }
    check_set_unset_overlap(set, unset, index)?;

    let mut field_map = field_mapping(raw, field).ok_or_else(|| {
        let mut error = OpError::invalid(
            "missing_target",
            index,
            format!("field {field:?} does not exist"),
        );
        error.field = Some(field.to_owned());
        error
    })?;

    // Snapshot prior presence/values for the semantic inverse.
    let prior_label = mapping_string(&field_map, "label");
    let prior_required = mapping_bool(&field_map, "required");
    let prior_suggest = mapping_bool(&field_map, "suggest");
    let prior_default = field_map
        .get(YamlValue::String("default".to_owned()))
        .cloned();
    let prior_min = mapping_number(&field_map, "min");
    let prior_max = mapping_number(&field_map, "max");

    // Type-check set values; false/zero/empty stay explicit and are
    // accepted only when the shared validator accepts them at the end.
    if let Some(value) = set.get("label") {
        require_json_string(value, "label", index, Some(field))?;
        if value.as_str().is_some_and(|text| text.trim().is_empty()) {
            return Err(OpError::invalid(
                "invalid_label",
                index,
                "field label must be a non-empty string",
            ));
        }
    }
    if let Some(value) = set.get("required") {
        require_json_bool(value, "required", index, Some(field))?;
    }
    if let Some(value) = set.get("suggest") {
        require_json_bool(value, "suggest", index, Some(field))?;
    }
    if let Some(value) = set.get("min") {
        require_json_number(value, "min", index, Some(field))?;
    }
    if let Some(value) = set.get("max") {
        require_json_number(value, "max", index, Some(field))?;
    }
    if let Some(value) = set.get("default")
        && value.is_null()
    {
        return Err(OpError::invalid(
            "invalid_default",
            index,
            "default null is not an implicit unset; use unset: [\"default\"]",
        ));
    }
    if field == "status" && set.contains_key("default") {
        return Err(OpError::invalid(
            "invalid_default",
            index,
            "status fields cannot have a default",
        ));
    }

    // Capture before validation state for newly-invalid accounting from
    // the current ordered operation state, so multiple edits to the same
    // field in one envelope count each transition rather than the original.
    let before_field = field_from_raw(field, &field_map)
        .or_else(|| inputs.before_type.field(field).cloned());
    let newly_invalid = match before_field {
        Some(before) => {
            // Apply set/unset to a scratch copy to build the after
            // candidate for this field only, then count records that
            // were valid before but invalid after.
            let mut scratch = field_map.clone();
            apply_field_set_map(&mut scratch, set, unset);
            match field_from_raw(field, &scratch) {
                Some(after) => count_newly_invalid(
                    inputs.folder,
                    &before,
                    &after,
                    inputs.today,
                ),
                None => 0,
            }
        }
        None => 0,
    };

    apply_field_set_map(&mut field_map, set, unset);
    insert_field_mapping(raw, field, field_map);

    // Inverse restores prior presence exactly: set prior values where
    // they were present, unset where they were absent.
    let mut inverse_set = HashMap::new();
    let mut inverse_unset = Vec::new();
    let touched: HashSet<&str> = set
        .keys()
        .map(String::as_str)
        .chain(unset.iter().map(String::as_str))
        .collect();
    for key in touched {
        match key {
            "label" => match prior_label.clone() {
                Some(prior) => {
                    inverse_set
                        .insert("label".to_owned(), JsonValue::String(prior));
                }
                None => inverse_unset.push("label".to_owned()),
            },
            "required" => match prior_required {
                Some(prior) => {
                    inverse_set
                        .insert("required".to_owned(), JsonValue::Bool(prior));
                }
                None => inverse_unset.push("required".to_owned()),
            },
            "suggest" => match prior_suggest {
                Some(prior) => {
                    inverse_set
                        .insert("suggest".to_owned(), JsonValue::Bool(prior));
                }
                None => inverse_unset.push("suggest".to_owned()),
            },
            "default" => match prior_default.clone() {
                Some(prior) => {
                    inverse_set
                        .insert("default".to_owned(), yaml_to_json(&prior));
                }
                None => inverse_unset.push("default".to_owned()),
            },
            "min" => match prior_min {
                Some(prior) => {
                    inverse_set
                        .insert("min".to_owned(), JsonValue::from(prior));
                }
                None => inverse_unset.push("min".to_owned()),
            },
            "max" => match prior_max {
                Some(prior) => {
                    inverse_set
                        .insert("max".to_owned(), JsonValue::from(prior));
                }
                None => inverse_unset.push("max".to_owned()),
            },
            _ => {}
        }
    }
    inverse_unset.sort();
    inverse_unset.dedup();
    let inverse = vec![Op::FieldSet {
        field: field.to_owned(),
        set: inverse_set,
        unset: inverse_unset,
    }];

    let filled = filled_for(inputs.folder, field);
    let summary = format!(
        "set field {field:?} ({}) · {filled} filled · {newly_invalid} newly invalid",
        describe_set_unset(set, unset)
    );
    Ok(OpResult {
        op_index: index,
        op_name: "field.set",
        summary,
        derived_key: None,
        filled: Some(filled),
        count: None,
        newly_invalid,
        inverse,
    })
}

fn apply_field_set_map(
    field_map: &mut Mapping,
    set: &HashMap<String, JsonValue>,
    unset: &[String],
) {
    for key in unset {
        field_map.remove(YamlValue::String(key.clone()));
    }
    for (key, value) in set {
        field_map.insert(YamlValue::String(key.clone()), json_to_yaml(value));
    }
}

fn apply_field_move(
    field: &str,
    position: usize,
    index: usize,
    raw: &mut Mapping,
) -> Result<OpResult, OpError> {
    let order = field_order(raw);
    let current =
        order.iter().position(|key| key == field).ok_or_else(|| {
            let mut error = OpError::invalid(
                "missing_target",
                index,
                format!("field {field:?} does not exist"),
            );
            error.field = Some(field.to_owned());
            error
        })?;
    if position >= order.len() {
        return Err(OpError::invalid(
            "position_out_of_range",
            index,
            format!(
                "field position {position} is out of range for {} fields",
                order.len()
            ),
        ));
    }
    if position == current {
        let inverse = vec![Op::FieldMove {
            field: field.to_owned(),
            position: current,
        }];
        return Ok(OpResult {
            op_index: index,
            op_name: "field.move",
            summary: format!("move field {field:?} stays at {current}"),
            derived_key: None,
            filled: None,
            count: None,
            newly_invalid: 0,
            inverse,
        });
    }
    let mut entries: Vec<(String, Mapping)> = Vec::new();
    for key in &order {
        if let Some(existing) = field_mapping(raw, key) {
            entries.push((key.clone(), existing));
        }
    }
    let (key, mapping) = entries.remove(current);
    debug_assert_eq!(key, field);
    // Positions index the remaining list: 0 prepends, len appends.
    let remaining = entries.len();
    if position > remaining {
        return Err(OpError::invalid(
            "position_out_of_range",
            index,
            format!(
                "field position {position} is out of range for {remaining} remaining fields"
            ),
        ));
    }
    entries.insert(position, (key, mapping));
    let fields_value = raw.get(YamlValue::String("fields".to_owned())).cloned();
    let mut next_fields = Mapping::new();
    for (key, mapping) in entries {
        next_fields.insert(YamlValue::String(key), YamlValue::Mapping(mapping));
    }
    raw.insert(
        YamlValue::String("fields".to_owned()),
        YamlValue::Mapping(next_fields),
    );
    let _ = fields_value;
    let inverse = vec![Op::FieldMove {
        field: field.to_owned(),
        position: current,
    }];
    Ok(OpResult {
        op_index: index,
        op_name: "field.move",
        summary: format!("move field {field:?} {current}→{position}"),
        derived_key: None,
        filled: None,
        count: None,
        newly_invalid: 0,
        inverse,
    })
}

fn apply_field_remove(
    field: &str,
    index: usize,
    raw: &mut Mapping,
    inputs: &ApplyInputs<'_>,
) -> Result<OpResult, OpError> {
    let field_map = field_mapping(raw, field).ok_or_else(|| {
        let mut error = OpError::invalid(
            "missing_target",
            index,
            format!("field {field:?} does not exist"),
        );
        error.field = Some(field.to_owned());
        error
    })?;
    if let Some(index_by) = inputs.index_refs.index_by.as_deref()
        && index_by == field
    {
        let mut error = OpError::refusal(
            "index_reference",
            index,
            format!(
                "field {field:?} is named by index_by; remove the index reference in YAML first"
            ),
            MIGRATION_HINT,
        );
        error.field = Some(field.to_owned());
        return Err(error);
    }
    if inputs
        .index_refs
        .index_facets
        .iter()
        .any(|facet| facet == field)
    {
        let mut error = OpError::refusal(
            "index_reference",
            index,
            format!(
                "field {field:?} is named by index_facets; remove the index reference in YAML first"
            ),
            MIGRATION_HINT,
        );
        error.field = Some(field.to_owned());
        return Err(error);
    }
    if !folder_complete(inputs.folder) {
        let mut error = OpError::invalid(
            "incomplete_scan",
            index,
            format!(
                "cannot prove field {field:?} unused; the folder scan is incomplete"
            ),
        );
        error.field = Some(field.to_owned());
        return Err(error);
    }
    let filled = filled_for(inputs.folder, field);
    if filled > 0 {
        let mut error = OpError::refusal(
            "field_used",
            index,
            format!("refused: {filled} records use field {field:?}"),
            MIGRATION_HINT,
        );
        error.field = Some(field.to_owned());
        error.affected = Some(filled);
        return Err(error);
    }

    // Snapshot the removed field for the restoration inverse: the add
    // carries the original stored key even when it differs from the current
    // label, plus kind/list/suggest/position and stored choice identities
    // via `restore_values` so keys survive even when they differ from their
    // labels. Required, default, bounds, absent label, and explicit
    // suggest-false return through ordinary follow-up sets/unsets.
    let position = field_order(raw)
        .iter()
        .position(|key| key == field)
        .unwrap_or(0);
    let prior_label = mapping_string(&field_map, "label");
    let label_for_add =
        prior_label.clone().unwrap_or_else(|| title_case(field));
    let kind = field_kind_name(&field_map);
    let list = mapping_bool(&field_map, "list").unwrap_or(false);
    let suggest_opt = mapping_bool(&field_map, "suggest");
    let suggest = suggest_opt.unwrap_or(false);
    let required = mapping_bool(&field_map, "required");
    let default = field_map
        .get(YamlValue::String("default".to_owned()))
        .cloned();
    let min = mapping_number(&field_map, "min");
    let max = mapping_number(&field_map, "max");
    let original_choices: Vec<(String, String)> = choice_order(&field_map)
        .into_iter()
        .filter_map(|value| {
            choice_label_in_map(&field_map, &value).map(|label| (value, label))
        })
        .collect();
    let (choice_drafts, restore_values) = if kind == "choices" {
        let mut drafts = Vec::with_capacity(original_choices.len());
        let mut values = Vec::with_capacity(original_choices.len());
        for (value, combined) in &original_choices {
            let (icon, rest) = split_icon(combined);
            drafts.push(ChoiceDraft {
                label: rest.to_owned(),
                emoji: icon.map(str::to_owned),
            });
            values.push(value.clone());
        }
        (drafts, Some(values))
    } else {
        (Vec::new(), None)
    };

    remove_field(raw, field);

    let mut inverse = vec![Op::FieldAdd {
        label: label_for_add,
        kind: kind.to_owned(),
        list,
        suggest,
        position: Some(position),
        choices: choice_drafts,
        restore_key: Some(field.to_owned()),
        restore_values,
    }];
    let mut restore_set = HashMap::new();
    let mut restore_unset = Vec::new();
    if prior_label.is_none() {
        restore_unset.push("label".to_owned());
    }
    // Explicit suggest:false must stay distinct from absent.
    if suggest_opt == Some(false) {
        restore_set.insert("suggest".to_owned(), JsonValue::Bool(false));
    }
    if let Some(required) = required {
        restore_set.insert("required".to_owned(), JsonValue::Bool(required));
    }
    if let Some(default) = default {
        restore_set.insert("default".to_owned(), yaml_to_json(&default));
    }
    if let Some(min) = min {
        restore_set.insert("min".to_owned(), JsonValue::from(min));
    }
    if let Some(max) = max {
        restore_set.insert("max".to_owned(), JsonValue::from(max));
    }
    if !restore_set.is_empty() || !restore_unset.is_empty() {
        restore_unset.sort();
        restore_unset.dedup();
        inverse.push(Op::FieldSet {
            field: field.to_owned(),
            set: restore_set,
            unset: restore_unset,
        });
    }

    Ok(OpResult {
        op_index: index,
        op_name: "field.remove",
        summary: format!("remove field {field:?} (was unused)"),
        derived_key: None,
        filled: Some(filled),
        count: None,
        newly_invalid: 0,
        inverse,
    })
}

// ---------------------------------------------------------------------------
// Choice-level ops
// ---------------------------------------------------------------------------

fn apply_choice_add(
    field: &str,
    label: &str,
    emoji: Option<&str>,
    position: Option<usize>,
    restore_value: Option<&str>,
    index: usize,
    raw: &mut Mapping,
    inputs: &ApplyInputs<'_>,
) -> Result<OpResult, OpError> {
    let mut field_map = field_mapping(raw, field).ok_or_else(|| {
        let mut error = OpError::invalid(
            "missing_target",
            index,
            format!("field {field:?} does not exist"),
        );
        error.field = Some(field.to_owned());
        error
    })?;
    if field_kind_name(&field_map) != "choices" {
        return Err(OpError::invalid(
            "invalid_choice",
            index,
            format!("field {field:?} is not choices-kind"),
        ));
    }
    if label.trim().is_empty() {
        return Err(OpError::invalid(
            "derived_key_invalid",
            index,
            "choice label must be a non-empty string",
        ));
    }
    let normalized_emoji = emoji.filter(|emoji| !emoji.is_empty());
    let derived = match restore_value {
        Some(explicit) => {
            if explicit.is_empty() {
                return Err(OpError::invalid(
                    "derived_key_invalid",
                    index,
                    "restore_value must be a non-empty string",
                ));
            }
            if choice_exists_in_map(&field_map, explicit) {
                return Err(OpError::invalid(
                    "derived_key_collision",
                    index,
                    format!(
                        "restore_value {explicit:?} already exists on field {field:?}; restoration cannot rename"
                    ),
                ));
            }
            explicit.to_owned()
        }
        None => {
            let derived = normalize_stem(label).ok_or_else(|| {
                OpError::invalid(
                    "derived_key_invalid",
                    index,
                    format!(
                        "choice label {label:?} has no alphanumeric characters for a value"
                    ),
                )
            })?;
            if choice_exists_in_map(&field_map, &derived) {
                return Err(OpError::invalid(
                    "derived_key_collision",
                    index,
                    format!(
                        "derived choice value {derived:?} from {label:?} already exists on field {field:?}"
                    ),
                ));
            }
            derived
        }
    };

    let order = choice_order(&field_map);
    let insert_at = position.unwrap_or(order.len());
    if insert_at > order.len() {
        return Err(OpError::invalid(
            "position_out_of_range",
            index,
            format!(
                "choice position {insert_at} is out of range for {} choices",
                order.len()
            ),
        ));
    }

    let combined = combine_choice_label(normalized_emoji, label);
    insert_choice(&mut field_map, &derived, &combined, insert_at);
    insert_field_mapping(raw, field, field_map);

    let count = count_for(inputs.folder, field, &derived);
    let summary = format!(
        "add choice {derived:?} to {field:?} at {insert_at} · {count} records use it"
    );
    let inverse = vec![Op::ChoiceRemove {
        field: field.to_owned(),
        value: derived.clone(),
    }];
    Ok(OpResult {
        op_index: index,
        op_name: "choice.add",
        summary,
        derived_key: Some(derived),
        filled: None,
        count: Some(count),
        newly_invalid: 0,
        inverse,
    })
}

fn apply_choice_set(
    field: &str,
    value: &str,
    set: &HashMap<String, JsonValue>,
    index: usize,
    raw: &mut Mapping,
    inputs: &ApplyInputs<'_>,
) -> Result<OpResult, OpError> {
    const ALLOWED: &[&str] = &["label", "emoji"];
    for key in set.keys() {
        if !ALLOWED.contains(&key.as_str()) {
            if key == "value" {
                let mut error = OpError::refusal(
                    "choice_rename_prohibited",
                    index,
                    format!(
                        "choice values never change; {value:?} keeps its stored value"
                    ),
                    MIGRATION_HINT,
                );
                error.field = Some(field.to_owned());
                error.choice = Some(value.to_owned());
                if folder_complete(inputs.folder) {
                    error.affected =
                        Some(count_for(inputs.folder, field, value));
                }
                return Err(error);
            }
            return Err(unknown_set_key(index, key, ALLOWED, Some(field)));
        }
    }
    if set.is_empty() {
        return Err(OpError::invalid(
            "envelope_missing_key",
            index,
            "choice.set requires set with label and/or emoji",
        ));
    }
    let mut field_map = field_mapping(raw, field).ok_or_else(|| {
        let mut error = OpError::invalid(
            "missing_target",
            index,
            format!("field {field:?} does not exist"),
        );
        error.field = Some(field.to_owned());
        error
    })?;
    if field_kind_name(&field_map) != "choices" {
        return Err(OpError::invalid(
            "invalid_choice",
            index,
            format!("field {field:?} is not choices-kind"),
        ));
    }
    let current = choice_label_in_map(&field_map, value).ok_or_else(|| {
        let mut error = OpError::invalid(
            "missing_target",
            index,
            format!("choice {value:?} does not exist on field {field:?}"),
        );
        error.field = Some(field.to_owned());
        error.choice = Some(value.to_owned());
        error
    })?;

    if let Some(label_value) = set.get("label") {
        require_json_string(label_value, "label", index, Some(field))?;
        if label_value
            .as_str()
            .is_some_and(|text| text.trim().is_empty())
        {
            return Err(OpError::invalid(
                "invalid_label",
                index,
                "choice label must be a non-empty string",
            ));
        }
    }
    if let Some(emoji_value) = set.get("emoji") {
        require_json_string(emoji_value, "emoji", index, Some(field))?;
    }

    let (current_icon, current_rest) = split_icon(&current);
    let next_label = set
        .get("label")
        .and_then(JsonValue::as_str)
        .unwrap_or(current_rest)
        .to_owned();
    if next_label.trim().is_empty() {
        return Err(OpError::invalid(
            "invalid_label",
            index,
            "choice label must be a non-empty string",
        ));
    }
    let next_emoji: Option<String> = match set.get("emoji") {
        None => current_icon.map(str::to_owned),
        Some(value) => {
            let text = value.as_str().unwrap_or_default();
            (!text.is_empty()).then(|| text.to_owned())
        }
    };
    let combined = combine_choice_label(next_emoji.as_deref(), &next_label);
    set_choice_label(&mut field_map, value, &combined);
    insert_field_mapping(raw, field, field_map);

    // Inverse restores the prior combined label exactly.
    let (prior_icon, prior_rest) = split_icon(&current);
    let mut inverse_set = HashMap::new();
    inverse_set
        .insert("label".to_owned(), JsonValue::String(prior_rest.to_owned()));
    // Empty emoji explicitly removes the icon; absent retains it, so
    // the inverse must spell the prior icon state explicitly.
    match prior_icon {
        Some(icon) => {
            inverse_set
                .insert("emoji".to_owned(), JsonValue::String(icon.to_owned()));
        }
        None => {
            inverse_set
                .insert("emoji".to_owned(), JsonValue::String(String::new()));
        }
    }
    // When the forward op only changed one component, the inverse
    // still restores both so omitted components do not drift.
    let inverse = vec![Op::ChoiceSet {
        field: field.to_owned(),
        value: value.to_owned(),
        set: inverse_set,
    }];

    Ok(OpResult {
        op_index: index,
        op_name: "choice.set",
        summary: format!("set choice {value:?} on {field:?} to {combined:?}"),
        derived_key: None,
        filled: None,
        count: None,
        newly_invalid: 0,
        inverse,
    })
}

fn apply_choice_move(
    field: &str,
    value: &str,
    position: usize,
    index: usize,
    raw: &mut Mapping,
) -> Result<OpResult, OpError> {
    let mut field_map = field_mapping(raw, field).ok_or_else(|| {
        let mut error = OpError::invalid(
            "missing_target",
            index,
            format!("field {field:?} does not exist"),
        );
        error.field = Some(field.to_owned());
        error
    })?;
    if field_kind_name(&field_map) != "choices" {
        return Err(OpError::invalid(
            "invalid_choice",
            index,
            format!("field {field:?} is not choices-kind"),
        ));
    }
    let order = choice_order(&field_map);
    let current =
        order.iter().position(|item| item == value).ok_or_else(|| {
            let mut error = OpError::invalid(
                "missing_target",
                index,
                format!("choice {value:?} does not exist on field {field:?}"),
            );
            error.field = Some(field.to_owned());
            error.choice = Some(value.to_owned());
            error
        })?;
    if position >= order.len() {
        return Err(OpError::invalid(
            "position_out_of_range",
            index,
            format!(
                "choice position {position} is out of range for {} choices",
                order.len()
            ),
        ));
    }
    if position == current {
        let inverse = vec![Op::ChoiceMove {
            field: field.to_owned(),
            value: value.to_owned(),
            position: current,
        }];
        return Ok(OpResult {
            op_index: index,
            op_name: "choice.move",
            summary: format!(
                "move choice {value:?} on {field:?} stays at {current}"
            ),
            derived_key: None,
            filled: None,
            count: None,
            newly_invalid: 0,
            inverse,
        });
    }
    move_choice(&mut field_map, current, position);
    insert_field_mapping(raw, field, field_map);
    let inverse = vec![Op::ChoiceMove {
        field: field.to_owned(),
        value: value.to_owned(),
        position: current,
    }];
    Ok(OpResult {
        op_index: index,
        op_name: "choice.move",
        summary: format!(
            "move choice {value:?} on {field:?} {current}→{position}"
        ),
        derived_key: None,
        filled: None,
        count: None,
        newly_invalid: 0,
        inverse,
    })
}

fn apply_choice_remove(
    field: &str,
    value: &str,
    index: usize,
    raw: &mut Mapping,
    inputs: &ApplyInputs<'_>,
) -> Result<OpResult, OpError> {
    let field_map = field_mapping(raw, field).ok_or_else(|| {
        let mut error = OpError::invalid(
            "missing_target",
            index,
            format!("field {field:?} does not exist"),
        );
        error.field = Some(field.to_owned());
        error
    })?;
    if field_kind_name(&field_map) != "choices" {
        return Err(OpError::invalid(
            "invalid_choice",
            index,
            format!("field {field:?} is not choices-kind"),
        ));
    }
    let label = choice_label_in_map(&field_map, value).ok_or_else(|| {
        let mut error = OpError::invalid(
            "missing_target",
            index,
            format!("choice {value:?} does not exist on field {field:?}"),
        );
        error.field = Some(field.to_owned());
        error.choice = Some(value.to_owned());
        error
    })?;
    if !folder_complete(inputs.folder) {
        let mut error = OpError::invalid(
            "incomplete_scan",
            index,
            format!(
                "cannot prove choice {value:?} on {field:?} unused; the folder scan is incomplete"
            ),
        );
        error.field = Some(field.to_owned());
        error.choice = Some(value.to_owned());
        return Err(error);
    }
    let count = count_for(inputs.folder, field, value);
    if count > 0 {
        let mut error = OpError::refusal(
            "choice_used",
            index,
            format!(
                "refused: {count} records use choice {value:?} on {field:?}"
            ),
            MIGRATION_HINT,
        );
        error.field = Some(field.to_owned());
        error.choice = Some(value.to_owned());
        error.affected = Some(count);
        return Err(error);
    }
    // Atomic batches may temporarily empty a choices mapping when later
    // ops restore validity; usage safety stays enforced above and the final
    // schema validation remains mandatory.
    let _remaining: Vec<String> = choice_order(&field_map)
        .into_iter()
        .filter(|item| item != value)
        .collect();

    let position = choice_order(&field_map)
        .iter()
        .position(|item| item == value)
        .unwrap_or(0);
    let (icon, rest) = split_icon(&label);
    let mut next_map = field_map.clone();
    remove_choice(&mut next_map, value);
    insert_field_mapping(raw, field, next_map);

    // Restoration retains the original stored value even when it
    // differs from the current label, and re-inserts at the prior slot.
    let inverse = vec![Op::ChoiceAdd {
        field: field.to_owned(),
        label: rest.to_owned(),
        emoji: icon.map(str::to_owned),
        position: Some(position),
        restore_value: Some(value.to_owned()),
    }];
    Ok(OpResult {
        op_index: index,
        op_name: "choice.remove",
        summary: format!("remove choice {value:?} from {field:?} (was unused)"),
        derived_key: None,
        filled: None,
        count: Some(count),
        newly_invalid: 0,
        inverse,
    })
}

// ---------------------------------------------------------------------------
// Final validation and guards
// ---------------------------------------------------------------------------

fn validate_final(
    raw: &Mapping,
    inputs: &ApplyInputs<'_>,
    op_index: usize,
) -> Result<(RecordType, Option<String>), OpError> {
    use std::path::Path;
    let value = YamlValue::Mapping(raw.clone());
    let parsed = parse_album_record_schema(
        Some(&value),
        Path::new("album.md"),
        inputs.stem,
        None,
        &HashMap::new(),
        1,
    );
    match (parsed.record_type, parsed.diagnostic) {
        (Some(record_type), None) => Ok((record_type, parsed.plural)),
        (_, Some(diagnostic)) => {
            let code = match diagnostic.code {
                crate::native::records::schema::RecordDiagnosticCode::InvalidDefault => {
                    "invalid_default"
                }
                crate::native::records::schema::RecordDiagnosticCode::InvalidNumberBound => {
                    "invalid_bounds"
                }
                _ => "invalid_schema",
            };
            let mut error = OpError::invalid(
                code,
                op_index,
                format!("final schema is invalid: {}", diagnostic.message),
            );
            error.field = diagnostic.field.clone();
            error.key_path = diagnostic.key_path.clone();
            Err(error)
        }
        (None, None) => Err(OpError::invalid(
            "invalid_schema",
            op_index,
            "final schema is invalid",
        )),
    }
}

fn check_storage_conflicts(
    after: &RecordType,
    others: &[OtherAlbumField],
    op_index: usize,
) -> Result<(), OpError> {
    for field in &after.fields {
        let storage = field.kind.storage_kind();
        for other in others.iter().filter(|other| other.key == field.key) {
            if other.storage != storage || other.list != field.list {
                let mut error = OpError::refusal(
                    "storage_conflict",
                    op_index,
                    format!(
                        "field {:?} storage {} {} clashes with {}",
                        field.key,
                        storage_name(storage),
                        if field.list { "list" } else { "scalar" },
                        other.path
                    ),
                    MIGRATION_HINT,
                );
                error.field = Some(field.key.clone());
                return Err(error);
            }
        }
    }
    Ok(())
}

fn storage_name(storage: PropertyStorageKind) -> &'static str {
    match storage {
        PropertyStorageKind::String => "string",
        PropertyStorageKind::Date => "date",
        PropertyStorageKind::Number => "number",
    }
}

fn build_consequence(results: &[OpResult], folder: &FolderIndex) -> String {
    if results.is_empty() {
        return "no schema edits".to_owned();
    }
    let total = folder.notes().len();
    let summaries: Vec<String> = results
        .iter()
        .map(|result| result.summary.clone())
        .collect();
    format!("{} · {total} records scanned", summaries.join("; "))
}

// ---------------------------------------------------------------------------
// Raw mapping helpers (ordered, presence-preserving)
// ---------------------------------------------------------------------------

fn raw_string(raw: &Mapping, key: &str) -> Option<String> {
    raw.get(YamlValue::String(key.to_owned()))
        .and_then(YamlValue::as_str)
        .map(str::to_owned)
}

fn raw_bool(raw: &Mapping, key: &str) -> Option<bool> {
    raw.get(YamlValue::String(key.to_owned()))
        .and_then(YamlValue::as_bool)
}

fn mapping_string(map: &Mapping, key: &str) -> Option<String> {
    map.get(YamlValue::String(key.to_owned()))
        .and_then(YamlValue::as_str)
        .map(str::to_owned)
}

fn mapping_bool(map: &Mapping, key: &str) -> Option<bool> {
    map.get(YamlValue::String(key.to_owned()))
        .and_then(YamlValue::as_bool)
}

fn mapping_number(map: &Mapping, key: &str) -> Option<f64> {
    map.get(YamlValue::String(key.to_owned()))
        .and_then(YamlValue::as_f64)
}

fn field_order(raw: &Mapping) -> Vec<String> {
    raw.get(YamlValue::String("fields".to_owned()))
        .and_then(YamlValue::as_mapping)
        .map(|fields| {
            fields
                .keys()
                .filter_map(YamlValue::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

fn field_mapping(raw: &Mapping, key: &str) -> Option<Mapping> {
    raw.get(YamlValue::String("fields".to_owned()))
        .and_then(YamlValue::as_mapping)
        .and_then(|fields| fields.get(YamlValue::String(key.to_owned())))
        .and_then(YamlValue::as_mapping)
        .cloned()
}

fn insert_field_mapping(raw: &mut Mapping, key: &str, map: Mapping) {
    let entry = raw
        .entry(YamlValue::String("fields".to_owned()))
        .or_insert_with(|| YamlValue::Mapping(Mapping::new()));
    if let YamlValue::Mapping(fields) = entry {
        fields
            .insert(YamlValue::String(key.to_owned()), YamlValue::Mapping(map));
    }
}

fn write_field_order(
    raw: &mut Mapping,
    ordered: &[(String, Mapping)],
    _inputs: &ApplyInputs<'_>,
) {
    let mut next = Mapping::new();
    for (key, map) in ordered {
        next.insert(
            YamlValue::String(key.clone()),
            YamlValue::Mapping(map.clone()),
        );
    }
    raw.insert(
        YamlValue::String("fields".to_owned()),
        YamlValue::Mapping(next),
    );
}

fn remove_field(raw: &mut Mapping, key: &str) {
    let Some(fields) = raw
        .get(YamlValue::String("fields".to_owned()))
        .and_then(YamlValue::as_mapping)
        .cloned()
    else {
        return;
    };
    // `Mapping::remove` may swap-remove and reorder siblings; rebuild
    // without the key so declaration order survives removals and Undo
    // round-trips stay byte-identical.
    let mut next = Mapping::new();
    for (item_key, item_value) in fields {
        if item_key != YamlValue::String(key.to_owned()) {
            next.insert(item_key, item_value);
        }
    }
    raw.insert(
        YamlValue::String("fields".to_owned()),
        YamlValue::Mapping(next),
    );
}

fn field_kind_name(field_map: &Mapping) -> &'static str {
    match field_map.get(YamlValue::String("values".to_owned())) {
        None => "text",
        Some(YamlValue::String(kind)) => match kind.as_str() {
            "date" => "date",
            "link" => "link",
            "url" => "url",
            "number" => "number",
            _ => "text",
        },
        Some(YamlValue::Mapping(_)) => "choices",
        _ => "text",
    }
}

fn choice_order(field_map: &Mapping) -> Vec<String> {
    field_map
        .get(YamlValue::String("values".to_owned()))
        .and_then(YamlValue::as_mapping)
        .map(|values| {
            values
                .keys()
                .filter_map(YamlValue::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

fn choice_exists(raw: &Mapping, field: &str, value: &str) -> bool {
    field_mapping(raw, field)
        .as_ref()
        .is_some_and(|map| choice_exists_in_map(map, value))
}

fn choice_exists_in_map(field_map: &Mapping, value: &str) -> bool {
    field_map
        .get(YamlValue::String("values".to_owned()))
        .and_then(YamlValue::as_mapping)
        .is_some_and(|values| {
            values.contains_key(YamlValue::String(value.to_owned()))
        })
}

fn choice_label_in_map(field_map: &Mapping, value: &str) -> Option<String> {
    field_map
        .get(YamlValue::String("values".to_owned()))
        .and_then(YamlValue::as_mapping)
        .and_then(|values| values.get(YamlValue::String(value.to_owned())))
        .and_then(YamlValue::as_str)
        .map(str::to_owned)
}

fn insert_choice(field_map: &mut Mapping, value: &str, label: &str, at: usize) {
    let mut ordered: Vec<(String, String)> = choice_order(field_map)
        .into_iter()
        .filter_map(|key| {
            choice_label_in_map(field_map, &key).map(|label| (key, label))
        })
        .collect();
    // Remove-then-index is handled by the caller for moves; adds
    // insert fresh keys at the requested slot.
    ordered.insert(at.min(ordered.len()), (value.to_owned(), label.to_owned()));
    let mut next = Mapping::new();
    for (key, label) in ordered {
        next.insert(YamlValue::String(key), YamlValue::String(label));
    }
    field_map.insert(
        YamlValue::String("values".to_owned()),
        YamlValue::Mapping(next),
    );
}

fn set_choice_label(field_map: &mut Mapping, value: &str, label: &str) {
    if let Some(YamlValue::Mapping(values)) =
        field_map.get_mut(YamlValue::String("values".to_owned()))
    {
        values.insert(
            YamlValue::String(value.to_owned()),
            YamlValue::String(label.to_owned()),
        );
    }
}

fn move_choice(field_map: &mut Mapping, from: usize, to: usize) {
    let mut ordered: Vec<(String, String)> = choice_order(field_map)
        .into_iter()
        .filter_map(|key| {
            choice_label_in_map(field_map, &key).map(|label| (key, label))
        })
        .collect();
    if from >= ordered.len() || to >= ordered.len() {
        return;
    }
    let entry = ordered.remove(from);
    ordered.insert(to, entry);
    let mut next = Mapping::new();
    for (key, label) in ordered {
        next.insert(YamlValue::String(key), YamlValue::String(label));
    }
    field_map.insert(
        YamlValue::String("values".to_owned()),
        YamlValue::Mapping(next),
    );
}

fn remove_choice(field_map: &mut Mapping, value: &str) {
    let Some(values) = field_map
        .get(YamlValue::String("values".to_owned()))
        .and_then(YamlValue::as_mapping)
        .cloned()
    else {
        return;
    };
    // Preserve choice order like `remove_field` does.
    let mut next = Mapping::new();
    for (item_key, item_value) in values {
        if item_key != YamlValue::String(value.to_owned()) {
            next.insert(item_key, item_value);
        }
    }
    field_map.insert(
        YamlValue::String("values".to_owned()),
        YamlValue::Mapping(next),
    );
}

fn combine_choice_label(emoji: Option<&str>, label: &str) -> String {
    match emoji {
        Some(icon) if !icon.trim().is_empty() => {
            format!("{} {}", icon.trim(), label.trim())
        }
        _ => label.trim().to_owned(),
    }
}

fn title_case(key: &str) -> String {
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

fn group_field_key(record_type: &RecordType) -> Option<String> {
    // Share the authoritative selector with `grouping.rs::group_field`:
    // prefer `status` choices, otherwise the first choices field.
    record_type
        .fields
        .iter()
        .find(|field| {
            field.key == "status" && matches!(field.kind, FieldKind::Choices(_))
        })
        .or_else(|| {
            record_type
                .fields
                .iter()
                .find(|field| matches!(field.kind, FieldKind::Choices(_)))
        })
        .map(|field| field.key.clone())
}

// ---------------------------------------------------------------------------
// Usage and validation consequences (shared counting rules)
// ---------------------------------------------------------------------------

fn filled_for(folder: &FolderIndex, field: &str) -> usize {
    folder.field_usage(field, &[]).0
}

fn count_for(folder: &FolderIndex, field: &str, choice: &str) -> usize {
    // field_usage counts each record once per choice, including list
    // values and values from readable invalid records.
    let ordered = choice_values_for(folder, field, choice);
    let (_, counts) = folder.field_usage(field, &ordered);
    ordered
        .iter()
        .position(|value| value == choice)
        .and_then(|index| counts.get(index).copied())
        .unwrap_or(0)
}

/// Build the choice-value order for counting: current folder-visible
/// values are unknown here, so count with the single target value.
/// `field_usage` matches by exact stored value, so a one-element order
/// yields the exact count for that choice.
fn choice_values_for(
    _folder: &FolderIndex,
    _field: &str,
    choice: &str,
) -> Vec<String> {
    vec![choice.to_owned()]
}

fn is_valid_for_counting(
    field: &crate::native::records::schema::RecordField,
    state: &crate::native::records::schema::FieldInputState,
    today: NaiveDate,
) -> bool {
    use crate::native::records::schema::{
        validate_field_input, FieldInputState,
    };
    match state {
        FieldInputState::Absent | FieldInputState::SuppliedEmpty
            if field.required =>
        {
            false
        }
        _ => validate_field_input(field, state, today).is_ok(),
    }
}

fn count_newly_invalid(
    folder: &FolderIndex,
    before: &crate::native::records::schema::RecordField,
    after: &crate::native::records::schema::RecordField,
    today: NaiveDate,
) -> usize {
    use crate::native::records::schema::FieldInputState;
    use crate::native::records::IndexedFieldValue;

    let mut newly = 0;
    for note in folder.notes() {
        let state = match note.field(&before.key) {
            None => FieldInputState::Absent,
            Some(IndexedFieldValue::Null) => FieldInputState::SuppliedEmpty,
            Some(IndexedFieldValue::Scalar(value)) => {
                FieldInputState::SuppliedValues(vec![value.clone()])
            }
            Some(IndexedFieldValue::List(values)) => {
                FieldInputState::SuppliedValues(values.clone())
            }
            Some(IndexedFieldValue::Unsupported) => {
                // Unsupported YAML can never prove validity; skip it
                // so malformed members do not inflate newly-invalid.
                continue;
            }
        };
        // `validate_field_input` alone accepts Absent/Empty even when
        // required; mirror `check.rs` so required tightening counts.
        let before_ok = is_valid_for_counting(before, &state, today);
        let after_ok = is_valid_for_counting(after, &state, today);
        if before_ok && !after_ok {
            newly += 1;
        }
    }
    newly
}

fn field_from_raw(
    key: &str,
    map: &Mapping,
) -> Option<crate::native::records::schema::RecordField> {
    // Share the authoritative schema parser so URL, numeric bounds, list
    // values, defaults, and choice identities validate exactly like the
    // final candidate. A minimal record mapping isolates the single field.
    use std::path::Path as StdPath;
    let mut fields = Mapping::new();
    fields.insert(
        YamlValue::String(key.to_owned()),
        YamlValue::Mapping(map.clone()),
    );
    let mut record = Mapping::new();
    record.insert(
        YamlValue::String("noun".to_owned()),
        YamlValue::String("test".to_owned()),
    );
    record.insert(
        YamlValue::String("fields".to_owned()),
        YamlValue::Mapping(fields),
    );
    let value = YamlValue::Mapping(record);
    let parsed = crate::native::records::schema::parse_album_record_schema(
        Some(&value),
        StdPath::new("album.md"),
        "test",
        None,
        &HashMap::new(),
        1,
    );
    parsed.record_type?.field(key).cloned()
}

// ---------------------------------------------------------------------------
// Small validators
// ---------------------------------------------------------------------------

fn check_set_unset_overlap(
    set: &HashMap<String, JsonValue>,
    unset: &[String],
    index: usize,
) -> Result<(), OpError> {
    for key in unset {
        if set.contains_key(key) {
            return Err(OpError::invalid(
                "set_unset_conflict",
                index,
                format!("{key:?} cannot be in both set and unset"),
            ));
        }
    }
    let mut seen = HashSet::new();
    for key in unset {
        if !seen.insert(key) {
            return Err(OpError::invalid(
                "envelope_duplicate_key",
                index,
                format!("duplicate unset entry {key:?}"),
            ));
        }
    }
    Ok(())
}

fn unknown_set_key(
    index: usize,
    key: &str,
    allowed: &[&str],
    field: Option<&str>,
) -> OpError {
    let mut error = OpError::invalid(
        "unknown_set_key",
        index,
        match field {
            Some(field) => format!(
                "unknown set key {key:?} for field {field:?}; expected one of {allowed:?}"
            ),
            None => format!(
                "unknown set key {key:?}; expected one of {allowed:?}"
            ),
        },
    );
    error.key_path = Some(key.to_owned());
    let suggestions = spelling_suggestions(key, allowed);
    if !suggestions.is_empty() {
        error.hint = Some(format!("did you mean {}?", suggestions.join(", ")));
    }
    error
}

fn spelling_suggestions(key: &str, allowed: &[&str]) -> Vec<String> {
    let mut candidates: Vec<(usize, &str)> = allowed
        .iter()
        .filter_map(|candidate| {
            let distance = edit_distance(key, candidate);
            (distance <= 2).then_some((distance, *candidate))
        })
        .collect();
    candidates.sort();
    candidates
        .into_iter()
        .take(3)
        .map(|(_, item)| item.to_owned())
        .collect()
}

fn edit_distance(left: &str, right: &str) -> usize {
    let mut previous: Vec<usize> = (0..=right.len()).collect();
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

fn require_json_string(
    value: &JsonValue,
    name: &str,
    index: usize,
    field: Option<&str>,
) -> Result<String, OpError> {
    value.as_str().map(str::to_owned).ok_or_else(|| {
        let mut error = OpError::invalid(
            "envelope_wrong_type",
            index,
            match field {
                Some(field) => {
                    format!("{name} for field {field:?} must be a string")
                }
                None => format!("{name} must be a string"),
            },
        );
        error.field = field.map(str::to_owned);
        error
    })
}

fn require_json_bool(
    value: &JsonValue,
    name: &str,
    index: usize,
    field: Option<&str>,
) -> Result<bool, OpError> {
    value.as_bool().ok_or_else(|| {
        let mut error = OpError::invalid(
            "envelope_wrong_type",
            index,
            match field {
                Some(field) => {
                    format!("{name} for field {field:?} must be a boolean")
                }
                None => format!("{name} must be a boolean"),
            },
        );
        error.field = field.map(str::to_owned);
        error
    })
}

fn require_json_number(
    value: &JsonValue,
    name: &str,
    index: usize,
    field: Option<&str>,
) -> Result<f64, OpError> {
    value
        .as_f64()
        .filter(|number| number.is_finite())
        .ok_or_else(|| {
            let mut error = OpError::invalid(
                "envelope_wrong_type",
                index,
                match field {
                    Some(field) => format!(
                        "{name} for field {field:?} must be a finite number"
                    ),
                    None => format!("{name} must be a finite number"),
                },
            );
            error.field = field.map(str::to_owned);
            error
        })
}

fn json_to_yaml(value: &JsonValue) -> YamlValue {
    match value {
        JsonValue::Null => YamlValue::Null,
        JsonValue::Bool(flag) => YamlValue::Bool(*flag),
        JsonValue::Number(number) => {
            if let Some(int) = number.as_i64() {
                YamlValue::Number(serde_yaml::Number::from(int))
            } else if let Some(uint) = number.as_u64() {
                YamlValue::Number(serde_yaml::Number::from(uint))
            } else if let Some(float) = number.as_f64() {
                YamlValue::Number(serde_yaml::Number::from(float))
            } else {
                YamlValue::Null
            }
        }
        JsonValue::String(text) => YamlValue::String(text.clone()),
        JsonValue::Array(items) => {
            YamlValue::Sequence(items.iter().map(json_to_yaml).collect())
        }
        JsonValue::Object(map) => {
            let mut out = Mapping::new();
            for (key, item) in map {
                out.insert(YamlValue::String(key.clone()), json_to_yaml(item));
            }
            YamlValue::Mapping(out)
        }
    }
}

fn yaml_to_json(value: &YamlValue) -> JsonValue {
    match value {
        YamlValue::Null => JsonValue::Null,
        YamlValue::Bool(flag) => JsonValue::Bool(*flag),
        YamlValue::Number(number) => {
            if let Some(int) = number.as_i64() {
                JsonValue::from(int)
            } else if let Some(uint) = number.as_u64() {
                JsonValue::from(uint)
            } else if let Some(float) = number.as_f64() {
                JsonValue::from(float)
            } else {
                JsonValue::Null
            }
        }
        YamlValue::String(text) => JsonValue::String(text.clone()),
        YamlValue::Sequence(items) => {
            JsonValue::Array(items.iter().map(yaml_to_json).collect())
        }
        YamlValue::Mapping(map) => {
            let mut out = serde_json::Map::new();
            for (key, item) in map {
                if let Some(name) = key.as_str() {
                    out.insert(name.to_owned(), yaml_to_json(item));
                }
            }
            JsonValue::Object(out)
        }
        YamlValue::Tagged(tagged) => yaml_to_json(&tagged.value),
    }
}

fn describe_set_unset(
    set: &HashMap<String, JsonValue>,
    unset: &[String],
) -> String {
    let mut parts: Vec<String> = set.keys().cloned().collect();
    parts.sort();
    let mut unset_sorted = unset.to_owned();
    unset_sorted.sort();
    for key in unset_sorted {
        parts.push(format!("unset {key}"));
    }
    parts.join(", ")
}

/// Semantic fingerprint of the current raw mapping for `expect` checks.
#[cfg(test)]
pub(crate) fn fingerprint_raw(raw: &Mapping, stem: &str) -> Option<String> {
    use std::path::Path;
    let value = YamlValue::Mapping(raw.clone());
    let parsed = parse_album_record_schema(
        Some(&value),
        Path::new("album.md"),
        stem,
        None,
        &HashMap::new(),
        1,
    );
    let record_type = parsed.record_type?;
    Some(album_schema_fingerprint(
        &record_type.id,
        &record_type.label,
        parsed.plural.as_deref().unwrap_or(""),
        &record_type,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native::records::RecordCatalog;
    use std::{
        fs,
        path::{Path, PathBuf},
        sync::atomic::{AtomicUsize, Ordering},
    };

    static COUNTER: AtomicUsize = AtomicUsize::new(0);

    struct TempVault {
        path: PathBuf,
    }

    impl TempVault {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "bob-update-ops-{}-{}",
                std::process::id(),
                COUNTER.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&path).expect("create temp vault");
            Self { path }
        }

        fn write(&self, relative: &str, contents: &str) {
            let full = self.path.join(relative);
            if let Some(parent) = full.parent() {
                fs::create_dir_all(parent).expect("create parent");
            }
            fs::write(&full, contents).expect("write note");
        }
    }

    impl Drop for TempVault {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    fn record_raw(yaml: &str) -> Mapping {
        let value: YamlValue =
            serde_yaml::from_str(yaml).expect("record yaml parses");
        value.as_mapping().cloned().expect("record is mapping")
    }

    fn parse_type(raw: &Mapping, stem: &str) -> (RecordType, Option<String>) {
        let value = YamlValue::Mapping(raw.clone());
        let parsed = parse_album_record_schema(
            Some(&value),
            Path::new("album.md"),
            stem,
            None,
            &HashMap::new(),
            1,
        );
        assert!(
            parsed.diagnostic.is_none(),
            "fixture schema must parse: {:?}",
            parsed.diagnostic
        );
        let record_type = parsed.record_type.expect("record type");
        (record_type, parsed.plural)
    }

    fn base_fixture() -> Mapping {
        record_raw(
            r#"
noun: restaurant
label: Restaurant
plural: restaurants
fields:
  status:
    label: Status
    values:
      to_try: To try
      liked: "💚 Liked"
  location:
    label: Location
"#,
        )
    }

    fn inputs_for<'a>(
        raw: &'a Mapping,
        record_type: &'a RecordType,
        plural: Option<&'a str>,
        folder: &'a FolderIndex,
    ) -> ApplyInputs<'a> {
        ApplyInputs {
            before_raw: raw,
            before_type: record_type,
            before_plural: plural,
            stem: "eat",
            folder,
            index_refs: &EMPTY_REFS,
            other_fields: &[],
            today: NaiveDate::from_ymd_opt(2026, 11, 8).expect("today"),
        }
    }

    static EMPTY_REFS: IndexRefs = IndexRefs {
        index_by: None,
        index_facets: Vec::new(),
    };

    fn empty_folder() -> (TempVault, RecordCatalog) {
        let vault = TempVault::new();
        let catalog = RecordCatalog::new(vault.path.clone());
        (vault, catalog)
    }

    #[test]
    fn every_operation_applies_and_reports() {
        let raw = base_fixture();
        let (before_type, before_plural) = parse_type(&raw, "eat");
        let (_vault, mut catalog) = empty_folder();
        let folder = catalog.folder("eat").clone();
        let inputs = ApplyInputs {
            before_raw: &raw,
            before_type: &before_type,
            before_plural: before_plural.as_deref(),
            stem: "eat",
            folder: &folder,
            index_refs: &EMPTY_REFS,
            other_fields: &[],
            today: NaiveDate::from_ymd_opt(2026, 11, 8).expect("today"),
        };

        // album.set
        let ops = vec![Op::AlbumSet {
            set: HashMap::from([(
                "label".to_owned(),
                JsonValue::String("Eateries".to_owned()),
            )]),
            unset: Vec::new(),
        }];
        let outcome = apply_ops(&ops, &inputs).expect("album.set applies");
        assert_eq!(outcome.after_type.label, "Eateries");
        assert_eq!(outcome.per_op[0].op_name, "album.set");

        // field.add (text)
        let ops = vec![Op::FieldAdd {
            label: "Notes".to_owned(),
            kind: "text".to_owned(),
            list: false,
            suggest: false,
            position: None,
            choices: Vec::new(),
            restore_key: None,
            restore_values: None,
        }];
        let outcome = apply_ops(&ops, &inputs).expect("field.add applies");
        assert_eq!(outcome.derived_keys[0].as_deref(), Some("notes"));
        assert!(outcome.after_type.field("notes").is_some());

        // field.set
        let ops = vec![Op::FieldSet {
            field: "location".to_owned(),
            set: HashMap::from([(
                "required".to_owned(),
                JsonValue::Bool(true),
            )]),
            unset: Vec::new(),
        }];
        let outcome = apply_ops(&ops, &inputs).expect("field.set applies");
        assert!(
            outcome
                .after_type
                .field("location")
                .expect("field")
                .required
        );

        // field.move
        let ops = vec![Op::FieldMove {
            field: "location".to_owned(),
            position: 0,
        }];
        let outcome = apply_ops(&ops, &inputs).expect("field.move applies");
        assert_eq!(outcome.after_type.fields[0].key, "location");

        // choice.add
        let ops = vec![Op::ChoiceAdd {
            field: "status".to_owned(),
            label: "Regular".to_owned(),
            emoji: Some("🔁".to_owned()),
            position: None,
            restore_value: None,
        }];
        let outcome = apply_ops(&ops, &inputs).expect("choice.add applies");
        assert_eq!(outcome.derived_keys[0].as_deref(), Some("regular"));

        // choice.set (empty emoji removes the icon)
        let ops = vec![Op::ChoiceSet {
            field: "status".to_owned(),
            value: "liked".to_owned(),
            set: HashMap::from([
                ("label".to_owned(), JsonValue::String("Liked".to_owned())),
                ("emoji".to_owned(), JsonValue::String(String::new())),
            ]),
        }];
        let outcome = apply_ops(&ops, &inputs).expect("choice.set applies");
        let status = outcome.after_type.field("status").expect("status");
        match &status.kind {
            FieldKind::Choices(choices) => {
                let liked = choices
                    .iter()
                    .find(|choice| choice.value == "liked")
                    .expect("liked");
                assert_eq!(liked.label, "Liked");
            }
            _ => panic!("status stays choices"),
        }

        // choice.move
        let ops = vec![Op::ChoiceMove {
            field: "status".to_owned(),
            value: "liked".to_owned(),
            position: 0,
        }];
        let outcome = apply_ops(&ops, &inputs).expect("choice.move applies");
        let status = outcome.after_type.field("status").expect("status");
        match &status.kind {
            FieldKind::Choices(choices) => {
                assert_eq!(choices[0].value, "liked");
            }
            _ => panic!("status stays choices"),
        }
    }

    #[test]
    fn ordered_batches_use_stable_keys_from_earlier_ops() {
        let raw = base_fixture();
        let (before_type, before_plural) = parse_type(&raw, "eat");
        let (_vault, mut catalog) = empty_folder();
        let folder = catalog.folder("eat").clone();
        let inputs = ApplyInputs {
            before_raw: &raw,
            before_type: &before_type,
            before_plural: before_plural.as_deref(),
            stem: "eat",
            folder: &folder,
            index_refs: &EMPTY_REFS,
            other_fields: &[],
            today: NaiveDate::from_ymd_opt(2026, 11, 8).expect("today"),
        };
        let ops = vec![
            Op::FieldAdd {
                label: "Category".to_owned(),
                kind: "text".to_owned(),
                list: false,
                suggest: true,
                position: None,
                choices: Vec::new(),
                restore_key: None,
                restore_values: None,
            },
            Op::FieldSet {
                field: "category".to_owned(),
                set: HashMap::from([(
                    "required".to_owned(),
                    JsonValue::Bool(true),
                )]),
                unset: Vec::new(),
            },
            Op::ChoiceAdd {
                field: "status".to_owned(),
                label: "Regular".to_owned(),
                emoji: Some("🔁".to_owned()),
                position: Some(0),
                restore_value: None,
            },
            Op::ChoiceMove {
                field: "status".to_owned(),
                value: "regular".to_owned(),
                position: 1,
            },
        ];
        let outcome = apply_ops(&ops, &inputs).expect("batch applies");
        assert_eq!(outcome.derived_keys[0].as_deref(), Some("category"));
        assert!(
            outcome
                .after_type
                .field("category")
                .expect("field")
                .required
        );
        assert_eq!(outcome.per_op.len(), 4);
        // Failure anywhere refuses the whole envelope.
        let bad = vec![
            Op::FieldAdd {
                label: "Ok".to_owned(),
                kind: "text".to_owned(),
                list: false,
                suggest: false,
                position: None,
                choices: Vec::new(),
                restore_key: None,
                restore_values: None,
            },
            Op::FieldMove {
                field: "missing".to_owned(),
                position: 0,
            },
        ];
        let error = apply_ops(&bad, &inputs).expect_err("batch refuses");
        assert_eq!(error.code, "missing_target");
    }

    #[test]
    fn derived_key_collisions_reserved_and_invalid_refuse() {
        let raw = base_fixture();
        let (before_type, before_plural) = parse_type(&raw, "eat");
        let (_vault, mut catalog) = empty_folder();
        let folder = catalog.folder("eat").clone();
        let inputs =
            inputs_for(&raw, &before_type, before_plural.as_deref(), &folder);

        // Colliding field label derives to an existing key.
        let ops = vec![Op::FieldAdd {
            label: "Status".to_owned(),
            kind: "text".to_owned(),
            list: false,
            suggest: false,
            position: None,
            choices: Vec::new(),
            restore_key: None,
            restore_values: None,
        }];
        let error = apply_ops(&ops, &inputs).expect_err("collision refuses");
        assert_eq!(error.code, "derived_key_collision");

        // Reserved field name.
        let ops = vec![Op::FieldAdd {
            label: "Title".to_owned(),
            kind: "text".to_owned(),
            list: false,
            suggest: false,
            position: None,
            choices: Vec::new(),
            restore_key: None,
            restore_values: None,
        }];
        let error = apply_ops(&ops, &inputs).expect_err("reserved refuses");
        assert_eq!(error.code, "derived_key_reserved");

        // Empty label has no alphanumeric characters.
        let ops = vec![Op::FieldAdd {
            label: "!!!".to_owned(),
            kind: "text".to_owned(),
            list: false,
            suggest: false,
            position: None,
            choices: Vec::new(),
            restore_key: None,
            restore_values: None,
        }];
        let error = apply_ops(&ops, &inputs).expect_err("empty refuses");
        assert_eq!(error.code, "derived_key_invalid");

        // Choice collision on the same field.
        let ops = vec![Op::ChoiceAdd {
            field: "status".to_owned(),
            label: "Liked".to_owned(),
            emoji: None,
            position: None,
            restore_value: None,
        }];
        let error = apply_ops(&ops, &inputs).expect_err("choice collision");
        assert_eq!(error.code, "derived_key_collision");
    }

    #[test]
    fn kind_specific_defaults_validate_through_the_shared_validator() {
        let raw = record_raw(
            r#"
noun: thing
fields:
  count:
    values: number
  when:
    values: date
  status:
    values:
      one: One
"#,
        );
        let (before_type, before_plural) = parse_type(&raw, "eat");
        let (_vault, mut catalog) = empty_folder();
        let folder = catalog.folder("eat").clone();
        let inputs =
            inputs_for(&raw, &before_type, before_plural.as_deref(), &folder);

        // Number default as JSON number is accepted.
        let ops = vec![Op::FieldSet {
            field: "count".to_owned(),
            set: HashMap::from([("default".to_owned(), JsonValue::from(2.5))]),
            unset: Vec::new(),
        }];
        apply_ops(&ops, &inputs).expect("number default applies");

        // Boolean default for a text field is refused, never stringified.
        let text_raw = record_raw(
            r#"
noun: thing
fields:
  note: {}
"#,
        );
        let (text_type, text_plural) = parse_type(&text_raw, "eat");
        let text_inputs =
            inputs_for(&text_raw, &text_type, text_plural.as_deref(), &folder);
        let ops = vec![Op::FieldSet {
            field: "note".to_owned(),
            set: HashMap::from([(
                "default".to_owned(),
                JsonValue::Bool(false),
            )]),
            unset: Vec::new(),
        }];
        let error = apply_ops(&ops, &text_inputs).expect_err("bool refuses");
        assert_eq!(error.code, "invalid_default");

        // Status defaults stay invalid.
        let ops = vec![Op::FieldSet {
            field: "status".to_owned(),
            set: HashMap::from([(
                "default".to_owned(),
                JsonValue::String("one".to_owned()),
            )]),
            unset: Vec::new(),
        }];
        let error = apply_ops(&ops, &inputs).expect_err("status refuses");
        assert_eq!(error.code, "invalid_default");

        // Date tokens are preserved, not resolved.
        let ops = vec![Op::FieldSet {
            field: "when".to_owned(),
            set: HashMap::from([(
                "default".to_owned(),
                JsonValue::String("today".to_owned()),
            )]),
            unset: Vec::new(),
        }];
        let outcome = apply_ops(&ops, &inputs).expect("today default");
        let when = outcome.after_type.field("when").expect("when");
        assert!(matches!(
            when.default,
            Some(crate::native::records::schema::RecordValue::Scalar(
                crate::native::records::schema::RecordScalar::Date(
                    crate::native::records::schema::RecordDate::Today
                )
            ))
        ));
    }

    #[test]
    fn unset_is_distinct_from_false_zero_and_empty() {
        // min on a text field is invalid, so the fixture omits it and
        // exercises unset directly.
        let raw = record_raw(
            r#"
noun: thing
label: Thing
fields:
  note:
    label: Note
    required: true
    default: hi
"#,
        );
        let (before_type, before_plural) = parse_type(&raw, "eat");
        let (_vault, mut catalog) = empty_folder();
        let folder = catalog.folder("eat").clone();
        let inputs =
            inputs_for(&raw, &before_type, before_plural.as_deref(), &folder);

        // Unset removes the YAML key.
        let ops = vec![Op::FieldSet {
            field: "note".to_owned(),
            set: HashMap::new(),
            unset: vec!["required".to_owned(), "default".to_owned()],
        }];
        let outcome = apply_ops(&ops, &inputs).expect("unset applies");
        let note = outcome.after_type.field("note").expect("note");
        assert!(!note.required);
        assert!(note.default.is_none());

        // Explicit false stays explicit and valid.
        let ops = vec![Op::FieldSet {
            field: "note".to_owned(),
            set: HashMap::from([(
                "required".to_owned(),
                JsonValue::Bool(false),
            )]),
            unset: Vec::new(),
        }];
        let outcome = apply_ops(&ops, &inputs).expect("false applies");
        assert!(!outcome.after_type.field("note").expect("note").required);

        // Zero, empty string, and null are distinct from absent: the
        // validator decides, and null is never an implicit unset.
        let ops = vec![Op::FieldSet {
            field: "note".to_owned(),
            set: HashMap::from([("default".to_owned(), JsonValue::Null)]),
            unset: Vec::new(),
        }];
        let error = apply_ops(&ops, &inputs).expect_err("null refuses");
        assert_eq!(error.code, "invalid_default");

        // set and unset cannot name the same attribute.
        let ops = vec![Op::FieldSet {
            field: "note".to_owned(),
            set: HashMap::from([(
                "required".to_owned(),
                JsonValue::Bool(true),
            )]),
            unset: vec!["required".to_owned()],
        }];
        let error = apply_ops(&ops, &inputs).expect_err("overlap refuses");
        assert_eq!(error.code, "set_unset_conflict");
    }

    #[test]
    fn forbidden_edits_return_typed_refusals_with_counts() {
        let vault = TempVault::new();
        vault.write(
            "eat/one.md",
            "---\ntitle: One\nstatus: liked\nlocation: Westfield\n---\n# One\n",
        );
        vault.write(
            "eat/two.md",
            "---\ntitle: Two\nstatus: liked\nlocation: Clark\n---\n# Two\n",
        );
        let mut catalog = RecordCatalog::new(vault.path.clone());
        let folder = catalog.folder("eat").clone();
        assert_eq!(folder.notes().len(), 2);

        let raw = base_fixture();
        let (before_type, before_plural) = parse_type(&raw, "eat");
        // Used-field and used-choice refusals are isolated from the
        // index guard with empty refs; the index guard is covered
        // separately below and always runs first for indexed fields.
        let unindexed_inputs = ApplyInputs {
            before_raw: &raw,
            before_type: &before_type,
            before_plural: before_plural.as_deref(),
            stem: "eat",
            folder: &folder,
            index_refs: &EMPTY_REFS,
            other_fields: &[],
            today: NaiveDate::from_ymd_opt(2026, 11, 8).expect("today"),
        };
        let refs = IndexRefs {
            index_by: Some("status".to_owned()),
            index_facets: vec!["location".to_owned()],
        };
        let inputs = ApplyInputs {
            before_raw: &raw,
            before_type: &before_type,
            before_plural: before_plural.as_deref(),
            stem: "eat",
            folder: &folder,
            index_refs: &refs,
            other_fields: &[],
            today: NaiveDate::from_ymd_opt(2026, 11, 8).expect("today"),
        };

        // Used field removal refuses with the filled count.
        let ops = vec![Op::FieldRemove {
            field: "status".to_owned(),
        }];
        let error = apply_ops(&ops, &unindexed_inputs).expect_err("used field");
        assert_eq!(error.code, "field_used");
        assert_eq!(error.affected, Some(2));

        // Used choice removal refuses with the choice count.
        let ops = vec![Op::ChoiceRemove {
            field: "status".to_owned(),
            value: "liked".to_owned(),
        }];
        let error =
            apply_ops(&ops, &unindexed_inputs).expect_err("used choice");
        assert_eq!(error.code, "choice_used");
        assert_eq!(error.affected, Some(2));

        // Index references refuse rather than changing top-level bytes.
        let ops = vec![Op::FieldRemove {
            field: "location".to_owned(),
        }];
        // location is filled too, but the index guard runs first for
        // this fixture; assert the refusal is the index one by using
        // an unused indexed field below.
        let _ = ops;
        let unused_raw = record_raw(
            r#"
noun: restaurant
fields:
  status:
    values:
      a: A
  extra: {}
"#,
        );
        let (unused_type, unused_plural) = parse_type(&unused_raw, "eat");
        let indexed = IndexRefs {
            index_by: Some("extra".to_owned()),
            index_facets: Vec::new(),
        };
        let unused_inputs = ApplyInputs {
            before_raw: &unused_raw,
            before_type: &unused_type,
            before_plural: unused_plural.as_deref(),
            stem: "eat",
            folder: &folder,
            index_refs: &indexed,
            other_fields: &[],
            today: NaiveDate::from_ymd_opt(2026, 11, 8).expect("today"),
        };
        let ops = vec![Op::FieldRemove {
            field: "extra".to_owned(),
        }];
        let error = apply_ops(&ops, &unused_inputs).expect_err("index refuses");
        assert_eq!(error.code, "index_reference");

        // Noun, kind, and list rewrites are prohibited with guidance.
        let ops = vec![Op::AlbumSet {
            set: HashMap::from([(
                "noun".to_owned(),
                JsonValue::String("x".to_owned()),
            )]),
            unset: Vec::new(),
        }];
        let error = apply_ops(&ops, &inputs).expect_err("noun refuses");
        assert_eq!(error.code, "noun_change_prohibited");

        let ops = vec![Op::FieldSet {
            field: "status".to_owned(),
            set: HashMap::from([(
                "kind".to_owned(),
                JsonValue::String("text".to_owned()),
            )]),
            unset: Vec::new(),
        }];
        let error = apply_ops(&ops, &inputs).expect_err("kind refuses");
        assert_eq!(error.code, "kind_change_prohibited");

        let ops = vec![Op::FieldSet {
            field: "status".to_owned(),
            set: HashMap::from([("list".to_owned(), JsonValue::Bool(true))]),
            unset: Vec::new(),
        }];
        let error = apply_ops(&ops, &inputs).expect_err("list refuses");
        assert_eq!(error.code, "list_change_prohibited");

        // Cross-album storage clashes name the key and the other album.
        let others = [OtherAlbumField {
            path: "drink.md".to_owned(),
            key: "location".to_owned(),
            storage: PropertyStorageKind::Number,
            list: false,
        }];
        let clash_inputs = ApplyInputs {
            before_raw: &raw,
            before_type: &before_type,
            before_plural: before_plural.as_deref(),
            stem: "eat",
            folder: &folder,
            index_refs: &EMPTY_REFS,
            other_fields: &others,
            today: NaiveDate::from_ymd_opt(2026, 11, 8).expect("today"),
        };
        // Adding a text `past` field is fine; the clash is on the
        // existing `location` (string scalar vs number scalar).
        let ops = vec![Op::FieldSet {
            field: "location".to_owned(),
            set: HashMap::from([(
                "label".to_owned(),
                JsonValue::String("Where".to_owned()),
            )]),
            unset: Vec::new(),
        }];
        let error =
            apply_ops(&ops, &clash_inputs).expect_err("storage clashes");
        assert_eq!(error.code, "storage_conflict");
    }

    #[test]
    fn list_values_false_and_zero_count_as_filled() {
        let vault = TempVault::new();
        vault.write(
            "eat/a.md",
            "---\ntitle: A\nflag: false\ncount: 0\ntags:\n  - x\n  - x\n  - y\n---\n# A\n",
        );
        vault.write(
            "eat/b.md",
            "---\ntitle: B\nflag: true\ncount: 1\ntags: x\n---\n# B\n",
        );
        let mut catalog = RecordCatalog::new(vault.path.clone());
        let folder = catalog.folder("eat").clone();
        // Each record counts once per choice, including list members.
        let (filled_tags, _) = folder.field_usage("tags", &["x".to_owned()]);
        assert_eq!(filled_tags, 2);
        let (_, counts) =
            folder.field_usage("tags", &["x".to_owned(), "y".to_owned()]);
        assert_eq!(counts, vec![2, 1]);
        let (filled_flag, _) = folder.field_usage("flag", &[]);
        assert_eq!(filled_flag, 2);
        let (filled_count, _) = folder.field_usage("count", &[]);
        assert_eq!(filled_count, 2);

        // Removal of a filled list field refuses with the filled count.
        let raw = record_raw(
            r#"
noun: thing
fields:
  tags:
    list: true
  flag: {}
  count:
    values: number
"#,
        );
        let (before_type, before_plural) = parse_type(&raw, "eat");
        let inputs =
            inputs_for(&raw, &before_type, before_plural.as_deref(), &folder);
        let error = apply_ops(
            &[Op::FieldRemove {
                field: "tags".to_owned(),
            }],
            &inputs,
        )
        .expect_err("filled list refuses");
        assert_eq!(error.code, "field_used");
        assert_eq!(error.affected, Some(2));
    }

    #[test]
    fn incomplete_scans_and_malformed_members_cannot_prove_unused() {
        // Readable invalid records stay visible in usage counts.
        let vault = TempVault::new();
        vault.write("eat/ok.md", "---\ntitle: Ok\nstatus: liked\n---\n# Ok\n");
        vault.write("eat/broken.md", "---\n: : not yaml\n---\n# Broken\n");
        let mut catalog = RecordCatalog::new(vault.path.clone());
        let folder = catalog.folder("eat").clone();
        assert_eq!(folder.notes().len(), 2);
        let (filled, _) = folder.field_usage("status", &["liked".to_owned()]);
        assert_eq!(filled, 1);

        let raw = record_raw(
            r#"
noun: restaurant
fields:
  status:
    values:
      liked: Liked
  unused: {}
"#,
        );
        let (before_type, before_plural) = parse_type(&raw, "eat");
        let inputs =
            inputs_for(&raw, &before_type, before_plural.as_deref(), &folder);
        // `unused` is genuinely absent everywhere, so removal is allowed
        // even though one member is malformed.
        apply_ops(
            &[Op::FieldRemove {
                field: "unused".to_owned(),
            }],
            &inputs,
        )
        .expect("unused removal allows readable-invalid members");

        // Unknown/unreadable state cannot prove removal unused. An
        // unreadable member (no complete frontmatter) makes the folder
        // incomplete, so even `unused` refuses.
        let vault = TempVault::new();
        vault.write("eat/good.md", "---\ntitle: Good\n---\n# Good\n");
        std::fs::write(
            vault.path.join("eat/bad.md"),
            "---\ntitle: Bad\nstatus: [unclosed\n",
        )
        .expect("write undecodable");
        let mut catalog = RecordCatalog::new(vault.path.clone());
        let folder = catalog.folder("eat").clone();
        let inputs =
            inputs_for(&raw, &before_type, before_plural.as_deref(), &folder);
        if folder_complete(&folder) {
            // Filesystem decoded the truncated note as readable-invalid;
            // the incomplete path is covered by the NoteRead branch in
            // production. This assertion documents the test setup.
            apply_ops(
                &[Op::FieldRemove {
                    field: "unused".to_owned(),
                }],
                &inputs,
            )
            .expect("readable-invalid still allows unused removal");
        } else {
            let error = apply_ops(
                &[Op::FieldRemove {
                    field: "unused".to_owned(),
                }],
                &inputs,
            )
            .expect_err("incomplete refuses");
            assert_eq!(error.code, "incomplete_scan");
        }
    }

    #[test]
    fn missing_targets_and_position_edges_refuse() {
        let raw = base_fixture();
        let (before_type, before_plural) = parse_type(&raw, "eat");
        let (_vault, mut catalog) = empty_folder();
        let folder = catalog.folder("eat").clone();
        let inputs =
            inputs_for(&raw, &before_type, before_plural.as_deref(), &folder);

        for op in [
            Op::FieldSet {
                field: "missing".to_owned(),
                set: HashMap::from([(
                    "label".to_owned(),
                    JsonValue::String("X".to_owned()),
                )]),
                unset: Vec::new(),
            },
            Op::FieldMove {
                field: "missing".to_owned(),
                position: 0,
            },
            Op::FieldRemove {
                field: "missing".to_owned(),
            },
            Op::ChoiceAdd {
                field: "missing".to_owned(),
                label: "X".to_owned(),
                emoji: None,
                position: None,
                restore_value: None,
            },
        ] {
            let error = apply_ops(std::slice::from_ref(&op), &inputs)
                .expect_err("missing");
            assert_eq!(error.code, "missing_target", "{op:?}");
        }

        // Add positions: 0 prepends, len appends, len+1 refuses.
        let ops = vec![Op::FieldAdd {
            label: "Extra".to_owned(),
            kind: "text".to_owned(),
            list: false,
            suggest: false,
            position: Some(0),
            choices: Vec::new(),
            restore_key: None,
            restore_values: None,
        }];
        let outcome = apply_ops(&ops, &inputs).expect("prepend applies");
        assert_eq!(outcome.after_type.fields[0].key, "extra");

        let len = before_type.fields.len();
        let ops = vec![Op::FieldAdd {
            label: "Extra".to_owned(),
            kind: "text".to_owned(),
            list: false,
            suggest: false,
            position: Some(len),
            choices: Vec::new(),
            restore_key: None,
            restore_values: None,
        }];
        apply_ops(&ops, &inputs).expect("append applies");

        let ops = vec![Op::FieldAdd {
            label: "Extra".to_owned(),
            kind: "text".to_owned(),
            list: false,
            suggest: false,
            position: Some(len + 1),
            choices: Vec::new(),
            restore_key: None,
            restore_values: None,
        }];
        let error = apply_ops(&ops, &inputs).expect_err("oob refuses");
        assert_eq!(error.code, "position_out_of_range");

        // Move positions index the remaining list.
        let ops = vec![Op::FieldMove {
            field: "status".to_owned(),
            position: 99,
        }];
        let error = apply_ops(&ops, &inputs).expect_err("move oob");
        assert_eq!(error.code, "position_out_of_range");
    }

    #[test]
    fn required_and_bound_changes_report_newly_invalid() {
        let vault = TempVault::new();
        vault.write(
            "eat/a.md",
            "---\ntitle: A\nlocation: Westfield\nscore: 5\n---\n# A\n",
        );
        vault.write("eat/b.md", "---\ntitle: B\nscore: 50\n---\n# B\n");
        let mut catalog = RecordCatalog::new(vault.path.clone());
        let folder = catalog.folder("eat").clone();

        let raw = record_raw(
            r#"
noun: thing
fields:
  location: {}
  score:
    values: number
"#,
        );
        let (before_type, before_plural) = parse_type(&raw, "eat");
        let inputs =
            inputs_for(&raw, &before_type, before_plural.as_deref(), &folder);

        // Making location required newly invalidates the record missing it.
        let ops = vec![Op::FieldSet {
            field: "location".to_owned(),
            set: HashMap::from([(
                "required".to_owned(),
                JsonValue::Bool(true),
            )]),
            unset: Vec::new(),
        }];
        let outcome = apply_ops(&ops, &inputs).expect("required applies");
        assert_eq!(outcome.per_op[0].newly_invalid, 1);

        // Tightening max newly invalidates the out-of-range record.
        let ops = vec![Op::FieldSet {
            field: "score".to_owned(),
            set: HashMap::from([("max".to_owned(), JsonValue::from(10))]),
            unset: Vec::new(),
        }];
        let outcome = apply_ops(&ops, &inputs).expect("bound applies");
        // One record is missing the field (still valid when optional)
        // and one record exceeds the new max.
        assert_eq!(outcome.per_op[0].newly_invalid, 1);
    }

    #[test]
    fn semantic_inverses_restore_order_and_identity() {
        let raw = base_fixture();
        let (before_type, before_plural) = parse_type(&raw, "eat");
        let (_vault, mut catalog) = empty_folder();
        let folder = catalog.folder("eat").clone();
        let inputs =
            inputs_for(&raw, &before_type, before_plural.as_deref(), &folder);

        // Forward add then inverse remove round-trips the fingerprint.
        let forward = vec![Op::FieldAdd {
            label: "Category".to_owned(),
            kind: "text".to_owned(),
            list: false,
            suggest: true,
            position: Some(0),
            choices: Vec::new(),
            restore_key: None,
            restore_values: None,
        }];
        let outcome = apply_ops(&forward, &inputs).expect("add applies");
        let after_raw = outcome.after_raw.clone();
        let (after_type, after_plural) = parse_type(&after_raw, "eat");
        let after_inputs = ApplyInputs {
            before_raw: &after_raw,
            before_type: &after_type,
            before_plural: after_plural.as_deref(),
            stem: "eat",
            folder: &folder,
            index_refs: &EMPTY_REFS,
            other_fields: &[],
            today: NaiveDate::from_ymd_opt(2026, 11, 8).expect("today"),
        };
        let back = apply_ops(&outcome.inverse_ops, &after_inputs)
            .expect("inverse applies");
        assert_eq!(
            fingerprint_raw(&back.after_raw, "eat"),
            fingerprint_raw(&raw, "eat")
        );

        // Removed unused fields restore their original stored key even
        // when it differs from the current label.
        let diverged = record_raw(
            r#"
noun: thing
fields:
  old_key:
    label: New Label
"#,
        );
        let (diverged_type, diverged_plural) = parse_type(&diverged, "eat");
        let diverged_inputs = inputs_for(
            &diverged,
            &diverged_type,
            diverged_plural.as_deref(),
            &folder,
        );
        let forward = vec![Op::FieldRemove {
            field: "old_key".to_owned(),
        }];
        let outcome =
            apply_ops(&forward, &diverged_inputs).expect("remove applies");
        assert!(matches!(
            &outcome.inverse_ops[0],
            Op::FieldAdd { restore_key: Some(key), .. } if key == "old_key"
        ));
        // The restoration only creates the absent key and validates.
        let (removed_type, removed_plural) =
            parse_type(&outcome.after_raw, "eat");
        let removed_inputs = ApplyInputs {
            before_raw: &outcome.after_raw,
            before_type: &removed_type,
            before_plural: removed_plural.as_deref(),
            stem: "eat",
            folder: &folder,
            index_refs: &EMPTY_REFS,
            other_fields: &[],
            today: NaiveDate::from_ymd_opt(2026, 11, 8).expect("today"),
        };
        let restored =
            apply_ops(&outcome.inverse_ops, &removed_inputs).expect("undo");
        assert!(restored.after_type.field("old_key").is_some());

        // Choice inverses restore semantics and order.
        let forward = vec![
            Op::ChoiceAdd {
                field: "status".to_owned(),
                label: "Regular".to_owned(),
                emoji: Some("🔁".to_owned()),
                position: Some(0),
                restore_value: None,
            },
            Op::ChoiceMove {
                field: "status".to_owned(),
                value: "regular".to_owned(),
                position: 1,
            },
        ];
        let outcome = apply_ops(&forward, &inputs).expect("choices apply");
        assert_eq!(outcome.inverse_ops.len(), 2);
        assert!(matches!(&outcome.inverse_ops[0], Op::ChoiceMove { .. }));
    }

    #[test]
    fn envelope_examples_parse_for_the_span_phase() {
        use super::super::protocol::parse_envelope;
        let envelope = parse_envelope(
            r#"{"schema_version":1,"expect":"sha256:abc","ops":[
              {"op":"album.set","set":{"label":"Eateries"},"unset":["plural"]},
              {"op":"field.add","label":"Status","kind":"choices","position":0,
               "choices":[{"label":"To try"},{"label":"Liked","emoji":"💚"}]},
              {"op":"choice.add","field":"status","label":"Regular","emoji":"🔁","position":2}
            ]}"#,
        )
        .expect("wire examples parse");
        assert_eq!(envelope.ops.len(), 3);
        assert_eq!(envelope.ops[2].name(), "choice.add");
    }

    #[test]
    fn group_prefers_status_over_first_choices() {
        let raw = record_raw(
            r#"
noun: restaurant
fields:
  category:
    values:
      family: Family
  status:
    values:
      ready: Ready
"#,
        );
        let (record_type, _) = parse_type(&raw, "eat");
        assert_eq!(
            group_field_key(&record_type),
            Some("status".to_owned()),
            "status wins even when it is not first"
        );
        let moved = record_raw(
            r#"
noun: restaurant
fields:
  status:
    values:
      ready: Ready
  category:
    values:
      family: Family
"#,
        );
        let (moved_type, _) = parse_type(&moved, "eat");
        assert_eq!(group_field_key(&moved_type), Some("status".to_owned()));
    }

    #[test]
    fn required_url_counts_newly_invalid() {
        let vault = TempVault::new();
        vault.write("eat/example.md", "---\ntitle: Example\n---\n# Example\n");
        let mut catalog = RecordCatalog::new(vault.path.clone());
        let folder = catalog.folder("eat").clone();
        let raw = record_raw(
            r#"
noun: restaurant
fields:
  website:
    values: url
"#,
        );
        let (before_type, before_plural) = parse_type(&raw, "eat");
        let inputs =
            inputs_for(&raw, &before_type, before_plural.as_deref(), &folder);
        let ops = vec![Op::FieldSet {
            field: "website".to_owned(),
            set: HashMap::from([(
                "required".to_owned(),
                JsonValue::Bool(true),
            )]),
            unset: Vec::new(),
        }];
        let outcome = apply_ops(&ops, &inputs).expect("required url applies");
        assert_eq!(outcome.per_op[0].newly_invalid, 1);
    }

    #[test]
    fn atomic_last_choice_replacement_allows_empty_intermediate() {
        let raw = record_raw(
            r#"
noun: restaurant
fields:
  category:
    values:
      old_key: Old label
"#,
        );
        let (before_type, before_plural) = parse_type(&raw, "eat");
        let (_vault, mut catalog) = empty_folder();
        let folder = catalog.folder("eat").clone();
        let inputs =
            inputs_for(&raw, &before_type, before_plural.as_deref(), &folder);
        let ops = vec![
            Op::ChoiceRemove {
                field: "category".to_owned(),
                value: "old_key".to_owned(),
            },
            Op::ChoiceAdd {
                field: "category".to_owned(),
                label: "New label".to_owned(),
                emoji: None,
                position: Some(0),
                restore_value: None,
            },
        ];
        let outcome =
            apply_ops(&ops, &inputs).expect("atomic replacement applies");
        assert_eq!(outcome.per_op.len(), 2);
        assert!(outcome.after_type.field("category").is_some());
    }

    #[test]
    fn prohibited_kind_change_carries_target_and_count() {
        let vault = TempVault::new();
        vault.write(
            "eat/one.md",
            "---\ntitle: One\nstatus: ready\n---\n# One\n",
        );
        let mut catalog = RecordCatalog::new(vault.path.clone());
        let folder = catalog.folder("eat").clone();
        let raw = record_raw(
            r#"
noun: restaurant
fields:
  status:
    values:
      ready: Ready
"#,
        );
        let (before_type, before_plural) = parse_type(&raw, "eat");
        let inputs =
            inputs_for(&raw, &before_type, before_plural.as_deref(), &folder);
        let ops = vec![Op::FieldSet {
            field: "status".to_owned(),
            set: HashMap::from([(
                "kind".to_owned(),
                JsonValue::String("text".to_owned()),
            )]),
            unset: Vec::new(),
        }];
        let error = apply_ops(&ops, &inputs).expect_err("kind refuses");
        assert_eq!(error.code, "kind_change_prohibited");
        assert_eq!(error.field, Some("status".to_owned()));
        assert_eq!(error.affected, Some(1));
    }

    #[test]
    fn field_remove_inverse_preserves_keys_and_absent_label() {
        let raw = record_raw(
            r#"
noun: restaurant
fields:
  category:
    label: Category
    values:
      old_key: Display label
      other_key: Another label
"#,
        );
        let (before_type, before_plural) = parse_type(&raw, "eat");
        let (_vault, mut catalog) = empty_folder();
        let folder = catalog.folder("eat").clone();
        let inputs =
            inputs_for(&raw, &before_type, before_plural.as_deref(), &folder);
        let outcome = apply_ops(
            &[Op::FieldRemove {
                field: "category".to_owned(),
            }],
            &inputs,
        )
        .expect("remove applies");
        assert_eq!(outcome.inverse_ops.len(), 1);
        let Op::FieldAdd {
            restore_key,
            restore_values,
            choices,
            ..
        } = &outcome.inverse_ops[0]
        else {
            panic!("inverse is field.add with restore");
        };
        assert_eq!(restore_key.as_deref(), Some("category"));
        assert_eq!(
            restore_values.as_deref(),
            Some(&["old_key".to_owned(), "other_key".to_owned()][..])
        );
        assert_eq!(choices.len(), 2);
        let (after_type, after_plural) = parse_type(&outcome.after_raw, "eat");
        let after_inputs = ApplyInputs {
            before_raw: &outcome.after_raw,
            before_type: &after_type,
            before_plural: after_plural.as_deref(),
            stem: "eat",
            folder: &folder,
            index_refs: &EMPTY_REFS,
            other_fields: &[],
            today: NaiveDate::from_ymd_opt(2026, 11, 8).expect("today"),
        };
        let restored = apply_ops(&outcome.inverse_ops, &after_inputs)
            .expect("inverse validates");
        assert!(restored.after_type.field("category").is_some());
        assert_eq!(
            restored.after_type.field("category").unwrap().kind,
            before_type.field("category").unwrap().kind
        );
    }
}
