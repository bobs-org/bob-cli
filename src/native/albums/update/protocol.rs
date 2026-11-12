//! Typed update envelope protocol for the guarded album schema writer.
//!
//! This module parses the JSON envelope once, rejects duplicate keys,
//! unknown versions/operations/keys, and wrong types with structured
//! diagnostics, and returns strongly typed operations for [`super::ops`].
//! It performs no vault I/O and never interpolates labels into shell
//! commands. An empty operation list is a valid no-op.

use std::collections::{HashMap, HashSet};

use serde_json::Value as JsonValue;

pub(crate) const SCHEMA_VERSION: u32 = 1;

/// Structured protocol failure. Semantic refusals (used removals,
/// index references, storage clashes, prohibited rewrites) are reported
/// by `ops` with the same shape; this type covers malformed envelopes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProtocolError {
    pub(crate) code: &'static str,
    pub(crate) message: String,
    pub(crate) op_index: Option<usize>,
    pub(crate) hint: Option<String>,
}

impl ProtocolError {
    fn envelope(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            op_index: None,
            hint: None,
        }
    }

    fn op(
        code: &'static str,
        index: usize,
        message: impl Into<String>,
    ) -> Self {
        Self {
            code,
            message: message.into(),
            op_index: Some(index),
            hint: None,
        }
    }

    fn op_hint(
        code: &'static str,
        index: usize,
        message: impl Into<String>,
        hint: impl Into<String>,
    ) -> Self {
        Self {
            code,
            message: message.into(),
            op_index: Some(index),
            hint: Some(hint.into()),
        }
    }
}

impl std::fmt::Display for ProtocolError {
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

impl std::error::Error for ProtocolError {}

/// Parsed envelope: the semantic fingerprint expectation plus ordered ops.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Envelope {
    pub(crate) expect: String,
    pub(crate) ops: Vec<Op>,
    /// Optional server-produced byte restorations, aligned with `ops`.
    /// The operation layer ignores these; the span planner resolves each
    /// schema key path against the current note and checks its preimage.
    pub(crate) restorations: Vec<Option<Restoration>>,
}

/// Narrow byte restoration carried by an inverse operation. Paths are
/// schema key paths, never filesystem paths or client-supplied offsets.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct Restoration {
    pub(crate) spans: Vec<RestoreSpan>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RestoreSpan {
    pub(crate) key_path: String,
    /// Exact current entry bytes, or `None` when the key is absent.
    pub(crate) expected: Option<String>,
    /// Exact prior entry bytes, or `None` when the key was absent.
    pub(crate) prior: Option<String>,
}

/// The nine v1 operations. Positions are zero-based; `None` means append
/// for adds. `restore_key` / `restore_value` / `restore_values` are Undo-only
/// restoration metadata: they create absent keys, never rename existing keys.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Op {
    AlbumSet {
        set: HashMap<String, JsonValue>,
        unset: Vec<String>,
    },
    FieldAdd {
        label: String,
        kind: String,
        list: bool,
        suggest: bool,
        position: Option<usize>,
        choices: Vec<ChoiceDraft>,
        restore_key: Option<String>,
        restore_values: Option<Vec<String>>,
    },
    FieldSet {
        field: String,
        set: HashMap<String, JsonValue>,
        unset: Vec<String>,
    },
    FieldMove {
        field: String,
        position: usize,
    },
    FieldRemove {
        field: String,
    },
    ChoiceAdd {
        field: String,
        label: String,
        emoji: Option<String>,
        position: Option<usize>,
        restore_value: Option<String>,
    },
    ChoiceSet {
        field: String,
        value: String,
        set: HashMap<String, JsonValue>,
    },
    ChoiceMove {
        field: String,
        value: String,
        position: usize,
    },
    ChoiceRemove {
        field: String,
        value: String,
    },
}

impl Op {
    pub(crate) fn name(&self) -> &'static str {
        match self {
            Self::AlbumSet { .. } => "album.set",
            Self::FieldAdd { .. } => "field.add",
            Self::FieldSet { .. } => "field.set",
            Self::FieldMove { .. } => "field.move",
            Self::FieldRemove { .. } => "field.remove",
            Self::ChoiceAdd { .. } => "choice.add",
            Self::ChoiceSet { .. } => "choice.set",
            Self::ChoiceMove { .. } => "choice.move",
            Self::ChoiceRemove { .. } => "choice.remove",
        }
    }
}

/// One choice draft for `field.add` when `kind == "choices"`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ChoiceDraft {
    pub(crate) label: String,
    pub(crate) emoji: Option<String>,
}

/// Parse and strictly validate the JSON envelope.
pub(crate) fn parse_envelope(text: &str) -> Result<Envelope, ProtocolError> {
    check_duplicate_keys(text)?;
    let value: JsonValue = serde_json::from_str(text).map_err(|error| {
        ProtocolError::envelope(
            "envelope_malformed",
            format!("malformed JSON envelope: {error}"),
        )
    })?;
    let mapping = value.as_object().ok_or_else(|| {
        ProtocolError::envelope(
            "envelope_wrong_type",
            "envelope must be a JSON object",
        )
    })?;
    const ALLOWED_TOP: &[&str] = &["schema_version", "expect", "ops"];
    for key in mapping.keys() {
        if !ALLOWED_TOP.contains(&key.as_str()) {
            return Err(ProtocolError::envelope(
                "envelope_unknown_key",
                format!("unknown envelope key {key:?}; expected one of {ALLOWED_TOP:?}"),
            ));
        }
    }
    let version_value = mapping.get("schema_version").ok_or_else(|| {
        ProtocolError::envelope(
            "envelope_missing_key",
            "envelope requires schema_version",
        )
    })?;
    let version = version_value.as_u64().ok_or_else(|| {
        ProtocolError::envelope(
            "envelope_wrong_type",
            "schema_version must be an integer",
        )
    })?;
    if version != u64::from(SCHEMA_VERSION) {
        return Err(ProtocolError::envelope(
            "envelope_unknown_version",
            format!(
                "unsupported schema_version {version}; expected {}",
                SCHEMA_VERSION
            ),
        ));
    }
    let expect_value = mapping.get("expect").ok_or_else(|| {
        ProtocolError::envelope(
            "envelope_missing_key",
            "envelope requires expect",
        )
    })?;
    let expect = expect_value.as_str().ok_or_else(|| {
        ProtocolError::envelope(
            "envelope_wrong_type",
            "expect must be a string semantic fingerprint",
        )
    })?;
    if expect.trim().is_empty() {
        return Err(ProtocolError::envelope(
            "envelope_wrong_type",
            "expect must be a non-empty fingerprint string",
        ));
    }
    if !expect.starts_with("sha256:") {
        return Err(ProtocolError::envelope(
            "envelope_wrong_type",
            "expect must look like sha256:…",
        ));
    }
    let ops_value = mapping.get("ops").ok_or_else(|| {
        ProtocolError::envelope("envelope_missing_key", "envelope requires ops")
    })?;
    let ops_array = ops_value.as_array().ok_or_else(|| {
        ProtocolError::envelope("envelope_wrong_type", "ops must be an array")
    })?;
    let mut ops = Vec::with_capacity(ops_array.len());
    let mut restorations = Vec::with_capacity(ops_array.len());
    for (index, raw) in ops_array.iter().enumerate() {
        ops.push(parse_op(raw, index)?);
        restorations.push(parse_restoration(raw, index)?);
    }
    Ok(Envelope {
        expect: expect.to_owned(),
        ops,
        restorations,
    })
}

fn parse_restoration(
    raw: &JsonValue,
    index: usize,
) -> Result<Option<Restoration>, ProtocolError> {
    let mapping = raw.as_object().ok_or_else(|| {
        ProtocolError::op(
            "envelope_wrong_type",
            index,
            "each op must be a JSON object",
        )
    })?;
    let Some(value) = mapping.get("restore") else {
        return Ok(None);
    };
    let object = value.as_object().ok_or_else(|| {
        ProtocolError::op(
            "envelope_wrong_type",
            index,
            "restore must be an object",
        )
    })?;
    for key in object.keys() {
        if key != "spans" {
            return Err(ProtocolError::op(
                "envelope_unknown_key",
                index,
                format!("unknown restore key {key:?}; expected \"spans\""),
            ));
        }
    }
    let entries = object.get("spans").ok_or_else(|| {
        ProtocolError::op(
            "envelope_missing_key",
            index,
            "restore requires spans",
        )
    })?;
    let entries = entries.as_array().ok_or_else(|| {
        ProtocolError::op(
            "envelope_wrong_type",
            index,
            "restore.spans must be an array",
        )
    })?;
    if entries.is_empty() {
        return Err(ProtocolError::op(
            "envelope_wrong_type",
            index,
            "restore.spans must not be empty",
        ));
    }
    let mut spans = Vec::with_capacity(entries.len());
    let mut paths = HashSet::new();
    for entry in entries {
        let fields = entry.as_object().ok_or_else(|| {
            ProtocolError::op(
                "envelope_wrong_type",
                index,
                "each restore span must be an object",
            )
        })?;
        for key in fields.keys() {
            if !["key_path", "expected", "prior"].contains(&key.as_str()) {
                return Err(ProtocolError::op(
                    "envelope_unknown_key",
                    index,
                    format!("unknown restore span key {key:?}"),
                ));
            }
        }
        let key_path = fields
            .get("key_path")
            .and_then(JsonValue::as_str)
            .ok_or_else(|| {
                ProtocolError::op(
                    "envelope_wrong_type",
                    index,
                    "restore span key_path must be a string",
                )
            })?
            .to_owned();
        if !valid_restore_key_path(&key_path) {
            return Err(ProtocolError::op_hint(
                "envelope_wrong_type",
                index,
                format!("unsupported restore key_path {key_path:?}"),
                "restore paths must name an album schema entry under record",
            ));
        }
        if !paths.insert(key_path.clone()) {
            return Err(ProtocolError::op(
                "envelope_duplicate_key",
                index,
                format!("restore contains duplicate key_path {key_path:?}"),
            ));
        }
        let expected = parse_optional_raw_span(fields, "expected", index)?;
        let prior = parse_optional_raw_span(fields, "prior", index)?;
        if !fields.contains_key("expected") || !fields.contains_key("prior") {
            return Err(ProtocolError::op(
                "envelope_missing_key",
                index,
                "restore spans require expected and prior (use null for an absent key)",
            ));
        }
        spans.push(RestoreSpan {
            key_path,
            expected,
            prior,
        });
    }
    Ok(Some(Restoration { spans }))
}

fn parse_optional_raw_span(
    mapping: &serde_json::Map<String, JsonValue>,
    key: &str,
    index: usize,
) -> Result<Option<String>, ProtocolError> {
    match mapping.get(key) {
        Some(JsonValue::String(text)) => Ok(Some(text.clone())),
        Some(JsonValue::Null) => Ok(None),
        Some(_) => Err(ProtocolError::op(
            "envelope_wrong_type",
            index,
            format!("restore span {key} must be a string or null"),
        )),
        None => Ok(None),
    }
}

fn valid_restore_key_path(path: &str) -> bool {
    let parts = path.split('.').collect::<Vec<_>>();
    match parts.as_slice() {
        ["record", key] => {
            ["label", "plural", "dated_notes", "legacy"].contains(key)
        }
        ["record", "fields", field] => valid_schema_key(field),
        ["record", "fields", field, property] => {
            valid_schema_key(field)
                && [
                    "label", "required", "list", "suggest", "default", "min",
                    "max", "values",
                ]
                .contains(property)
        }
        ["record", "fields", field, "values", choice] => {
            valid_schema_key(field) && valid_schema_key(choice)
        }
        _ => false,
    }
}

fn valid_schema_key(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}

fn parse_op(raw: &JsonValue, index: usize) -> Result<Op, ProtocolError> {
    let mapping = raw.as_object().ok_or_else(|| {
        ProtocolError::op(
            "envelope_wrong_type",
            index,
            "each op must be a JSON object",
        )
    })?;
    let op_value = mapping.get("op").ok_or_else(|| {
        ProtocolError::op(
            "envelope_missing_key",
            index,
            "each op requires an op name",
        )
    })?;
    let op_name = op_value.as_str().ok_or_else(|| {
        ProtocolError::op("envelope_wrong_type", index, "op must be a string")
    })?;
    match op_name {
        "album.set" => parse_album_set(mapping, index),
        "field.add" => parse_field_add(mapping, index),
        "field.set" => parse_field_set(mapping, index),
        "field.move" => parse_field_move(mapping, index),
        "field.remove" => parse_field_remove(mapping, index),
        "choice.add" => parse_choice_add(mapping, index),
        "choice.set" => parse_choice_set(mapping, index),
        "choice.move" => parse_choice_move(mapping, index),
        "choice.remove" => parse_choice_remove(mapping, index),
        _ => Err(ProtocolError::op_hint(
            "envelope_unknown_op",
            index,
            format!("unknown op {op_name:?}"),
            "expected one of album.set, field.add, field.set, field.move, field.remove, choice.add, choice.set, choice.move, choice.remove",
        )),
    }
}

fn reject_unknown_op_keys(
    mapping: &serde_json::Map<String, JsonValue>,
    allowed: &[&str],
    index: usize,
) -> Result<(), ProtocolError> {
    for key in mapping.keys() {
        if key != "restore" && !allowed.contains(&key.as_str()) {
            return Err(ProtocolError::op(
                "envelope_unknown_key",
                index,
                format!(
                    "unknown key {key:?} for op {:?}; expected one of {allowed:?}",
                    mapping.get("op"),
                ),
            ));
        }
    }
    Ok(())
}

fn required_string(
    mapping: &serde_json::Map<String, JsonValue>,
    key: &str,
    index: usize,
) -> Result<String, ProtocolError> {
    let value = mapping.get(key).ok_or_else(|| {
        ProtocolError::op(
            "envelope_missing_key",
            index,
            format!("op requires {key:?}"),
        )
    })?;
    value.as_str().map(str::to_owned).ok_or_else(|| {
        ProtocolError::op(
            "envelope_wrong_type",
            index,
            format!("{key} must be a string"),
        )
    })
}

fn optional_string(
    mapping: &serde_json::Map<String, JsonValue>,
    key: &str,
    index: usize,
) -> Result<Option<String>, ProtocolError> {
    match mapping.get(key) {
        None => Ok(None),
        Some(value) => {
            value.as_str().map(str::to_owned).map(Some).ok_or_else(|| {
                ProtocolError::op(
                    "envelope_wrong_type",
                    index,
                    format!("{key} must be a string"),
                )
            })
        }
    }
}

fn optional_bool(
    mapping: &serde_json::Map<String, JsonValue>,
    key: &str,
    index: usize,
    default: bool,
) -> Result<bool, ProtocolError> {
    match mapping.get(key) {
        None => Ok(default),
        Some(value) => value.as_bool().ok_or_else(|| {
            ProtocolError::op(
                "envelope_wrong_type",
                index,
                format!("{key} must be a boolean"),
            )
        }),
    }
}

fn optional_position(
    mapping: &serde_json::Map<String, JsonValue>,
    key: &str,
    index: usize,
) -> Result<Option<usize>, ProtocolError> {
    match mapping.get(key) {
        None => Ok(None),
        Some(value) => {
            let number = value.as_u64().ok_or_else(|| {
                ProtocolError::op(
                    "envelope_wrong_type",
                    index,
                    format!("{key} must be a non-negative integer position"),
                )
            })?;
            usize::try_from(number).map(Some).map_err(|_| {
                ProtocolError::op(
                    "envelope_wrong_type",
                    index,
                    format!("{key} position is too large"),
                )
            })
        }
    }
}

fn required_position(
    mapping: &serde_json::Map<String, JsonValue>,
    key: &str,
    index: usize,
) -> Result<usize, ProtocolError> {
    let value = mapping.get(key).ok_or_else(|| {
        ProtocolError::op(
            "envelope_missing_key",
            index,
            format!("op requires {key:?}"),
        )
    })?;
    let number = value.as_u64().ok_or_else(|| {
        ProtocolError::op(
            "envelope_wrong_type",
            index,
            format!("{key} must be a non-negative integer position"),
        )
    })?;
    usize::try_from(number).map_err(|_| {
        ProtocolError::op(
            "envelope_wrong_type",
            index,
            format!("{key} position is too large"),
        )
    })
}

fn parse_set_map(
    mapping: &serde_json::Map<String, JsonValue>,
    key: &str,
    index: usize,
) -> Result<HashMap<String, JsonValue>, ProtocolError> {
    match mapping.get(key) {
        None => Ok(HashMap::new()),
        Some(value) => {
            let object = value.as_object().ok_or_else(|| {
                ProtocolError::op(
                    "envelope_wrong_type",
                    index,
                    format!("{key} must be an object"),
                )
            })?;
            Ok(object
                .iter()
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect())
        }
    }
}

fn parse_unset_vec(
    mapping: &serde_json::Map<String, JsonValue>,
    key: &str,
    index: usize,
) -> Result<Vec<String>, ProtocolError> {
    match mapping.get(key) {
        None => Ok(Vec::new()),
        Some(value) => {
            let array = value.as_array().ok_or_else(|| {
                ProtocolError::op(
                    "envelope_wrong_type",
                    index,
                    format!("{key} must be an array of property names"),
                )
            })?;
            array
                .iter()
                .map(|item| {
                    item.as_str().map(str::to_owned).ok_or_else(|| {
                        ProtocolError::op(
                            "envelope_wrong_type",
                            index,
                            format!("{key} entries must be strings"),
                        )
                    })
                })
                .collect()
        }
    }
}

fn parse_album_set(
    mapping: &serde_json::Map<String, JsonValue>,
    index: usize,
) -> Result<Op, ProtocolError> {
    reject_unknown_op_keys(mapping, &["op", "set", "unset"], index)?;
    let set = parse_set_map(mapping, "set", index)?;
    let unset = parse_unset_vec(mapping, "unset", index)?;
    if set.is_empty() && unset.is_empty() {
        return Err(ProtocolError::op(
            "envelope_missing_key",
            index,
            "album.set requires set and/or unset",
        ));
    }
    Ok(Op::AlbumSet { set, unset })
}

fn parse_field_add(
    mapping: &serde_json::Map<String, JsonValue>,
    index: usize,
) -> Result<Op, ProtocolError> {
    reject_unknown_op_keys(
        mapping,
        &[
            "op",
            "label",
            "kind",
            "list",
            "suggest",
            "position",
            "choices",
            "restore_key",
            "restore_values",
        ],
        index,
    )?;
    let label = required_string(mapping, "label", index)?;
    let kind = optional_string(mapping, "kind", index)?
        .unwrap_or_else(|| "text".to_owned());
    const KINDS: &[&str] =
        &["text", "choices", "date", "link", "url", "number"];
    if !KINDS.contains(&kind.as_str()) {
        return Err(ProtocolError::op(
            "envelope_wrong_type",
            index,
            format!("kind must be one of {KINDS:?}; got {kind:?}"),
        ));
    }
    let list = optional_bool(mapping, "list", index, false)?;
    let suggest = optional_bool(mapping, "suggest", index, false)?;
    let position = optional_position(mapping, "position", index)?;
    let restore_key = optional_string(mapping, "restore_key", index)?;
    let restore_values = match mapping.get("restore_values") {
        None => None,
        Some(value) => {
            let array = value.as_array().ok_or_else(|| {
                ProtocolError::op(
                    "envelope_wrong_type",
                    index,
                    "restore_values must be an array of stored choice keys",
                )
            })?;
            let mut values = Vec::with_capacity(array.len());
            for entry in array {
                let text = entry.as_str().ok_or_else(|| {
                    ProtocolError::op(
                        "envelope_wrong_type",
                        index,
                        "each restore value must be a string",
                    )
                })?;
                values.push(text.to_owned());
            }
            Some(values)
        }
    };
    let choices = match mapping.get("choices") {
        None => Vec::new(),
        Some(value) => {
            let array = value.as_array().ok_or_else(|| {
                ProtocolError::op(
                    "envelope_wrong_type",
                    index,
                    "choices must be an array of label/emoji objects",
                )
            })?;
            let mut drafts = Vec::with_capacity(array.len());
            for entry in array {
                let entry_map = entry.as_object().ok_or_else(|| {
                    ProtocolError::op(
                        "envelope_wrong_type",
                        index,
                        "each choice must be an object",
                    )
                })?;
                for key in entry_map.keys() {
                    if !["label", "emoji"].contains(&key.as_str()) {
                        return Err(ProtocolError::op(
                            "envelope_unknown_key",
                            index,
                            format!("unknown choice key {key:?}"),
                        ));
                    }
                }
                let label_value = entry_map.get("label").ok_or_else(|| {
                    ProtocolError::op(
                        "envelope_missing_key",
                        index,
                        "each choice requires label",
                    )
                })?;
                let label_text = label_value.as_str().ok_or_else(|| {
                    ProtocolError::op(
                        "envelope_wrong_type",
                        index,
                        "choice label must be a string",
                    )
                })?;
                let emoji = match entry_map.get("emoji") {
                    None => None,
                    Some(value) => Some(
                        value
                            .as_str()
                            .ok_or_else(|| {
                                ProtocolError::op(
                                    "envelope_wrong_type",
                                    index,
                                    "choice emoji must be a string",
                                )
                            })?
                            .to_owned(),
                    ),
                };
                drafts.push(ChoiceDraft {
                    label: label_text.to_owned(),
                    emoji,
                });
            }
            drafts
        }
    };
    if kind == "choices" && choices.is_empty() && restore_key.is_none() {
        return Err(ProtocolError::op(
            "envelope_missing_key",
            index,
            "choices-kind field.add requires a nonempty choices array",
        ));
    }
    if kind != "choices" && !choices.is_empty() {
        return Err(ProtocolError::op(
            "envelope_wrong_type",
            index,
            "choices are only valid for kind choices",
        ));
    }
    if let Some(values) = restore_values.as_ref() {
        if restore_key.is_none() {
            return Err(ProtocolError::op(
                "envelope_wrong_type",
                index,
                "restore_values requires restore_key",
            ));
        }
        if kind != "choices" && !values.is_empty() {
            return Err(ProtocolError::op(
                "envelope_wrong_type",
                index,
                "restore_values is only valid for kind choices",
            ));
        }
        if values.len() != choices.len() {
            return Err(ProtocolError::op(
                "envelope_wrong_type",
                index,
                "restore_values length must match choices length",
            ));
        }
    }
    Ok(Op::FieldAdd {
        label,
        kind,
        list,
        suggest,
        position,
        choices,
        restore_key,
        restore_values,
    })
}

fn parse_field_set(
    mapping: &serde_json::Map<String, JsonValue>,
    index: usize,
) -> Result<Op, ProtocolError> {
    reject_unknown_op_keys(mapping, &["op", "field", "set", "unset"], index)?;
    let field = required_string(mapping, "field", index)?;
    let set = parse_set_map(mapping, "set", index)?;
    let unset = parse_unset_vec(mapping, "unset", index)?;
    if set.is_empty() && unset.is_empty() {
        return Err(ProtocolError::op(
            "envelope_missing_key",
            index,
            "field.set requires set and/or unset",
        ));
    }
    Ok(Op::FieldSet { field, set, unset })
}

fn parse_field_move(
    mapping: &serde_json::Map<String, JsonValue>,
    index: usize,
) -> Result<Op, ProtocolError> {
    reject_unknown_op_keys(mapping, &["op", "field", "position"], index)?;
    let field = required_string(mapping, "field", index)?;
    let position = required_position(mapping, "position", index)?;
    Ok(Op::FieldMove { field, position })
}

fn parse_field_remove(
    mapping: &serde_json::Map<String, JsonValue>,
    index: usize,
) -> Result<Op, ProtocolError> {
    reject_unknown_op_keys(mapping, &["op", "field"], index)?;
    let field = required_string(mapping, "field", index)?;
    Ok(Op::FieldRemove { field })
}

fn parse_choice_add(
    mapping: &serde_json::Map<String, JsonValue>,
    index: usize,
) -> Result<Op, ProtocolError> {
    reject_unknown_op_keys(
        mapping,
        &["op", "field", "label", "emoji", "position", "restore_value"],
        index,
    )?;
    let field = required_string(mapping, "field", index)?;
    let label = required_string(mapping, "label", index)?;
    let emoji = optional_string(mapping, "emoji", index)?;
    let position = optional_position(mapping, "position", index)?;
    let restore_value = optional_string(mapping, "restore_value", index)?;
    Ok(Op::ChoiceAdd {
        field,
        label,
        emoji,
        position,
        restore_value,
    })
}

fn parse_choice_set(
    mapping: &serde_json::Map<String, JsonValue>,
    index: usize,
) -> Result<Op, ProtocolError> {
    reject_unknown_op_keys(mapping, &["op", "field", "value", "set"], index)?;
    let field = required_string(mapping, "field", index)?;
    let value = required_string(mapping, "value", index)?;
    let set = parse_set_map(mapping, "set", index)?;
    if set.is_empty() {
        return Err(ProtocolError::op(
            "envelope_missing_key",
            index,
            "choice.set requires set with label and/or emoji",
        ));
    }
    Ok(Op::ChoiceSet { field, value, set })
}

fn parse_choice_move(
    mapping: &serde_json::Map<String, JsonValue>,
    index: usize,
) -> Result<Op, ProtocolError> {
    reject_unknown_op_keys(
        mapping,
        &["op", "field", "value", "position"],
        index,
    )?;
    let field = required_string(mapping, "field", index)?;
    let value = required_string(mapping, "value", index)?;
    let position = required_position(mapping, "position", index)?;
    Ok(Op::ChoiceMove {
        field,
        value,
        position,
    })
}

fn parse_choice_remove(
    mapping: &serde_json::Map<String, JsonValue>,
    index: usize,
) -> Result<Op, ProtocolError> {
    reject_unknown_op_keys(mapping, &["op", "field", "value"], index)?;
    let field = required_string(mapping, "field", index)?;
    let value = required_string(mapping, "value", index)?;
    Ok(Op::ChoiceRemove { field, value })
}

/// Reject duplicate JSON object keys anywhere in the envelope.
///
/// `serde_json` keeps the last duplicate, so scan the raw text: a string
/// followed by `:` inside an object is a key; two identical keys in one
/// object refuse the whole envelope.
fn check_duplicate_keys(text: &str) -> Result<(), ProtocolError> {
    let bytes = text.as_bytes();
    let mut index = 0;
    let mut stack: Vec<ObjectFrame> = Vec::new();
    while index < bytes.len() {
        let byte = bytes[index];
        if byte.is_ascii_whitespace() || byte == b':' || byte == b',' {
            index += 1;
            continue;
        }
        if byte == b'{' {
            stack.push(ObjectFrame {
                kind: FrameKind::Object,
                keys: HashSet::new(),
            });
            index += 1;
            continue;
        }
        if byte == b'[' {
            stack.push(ObjectFrame {
                kind: FrameKind::Array,
                keys: HashSet::new(),
            });
            index += 1;
            continue;
        }
        if byte == b'}' || byte == b']' {
            stack.pop();
            index += 1;
            continue;
        }
        if byte == b'"' {
            let (parsed, next) =
                parse_json_string(text, index).map_err(|message| {
                    ProtocolError::envelope("envelope_malformed", message)
                })?;
            let mut cursor = next;
            while cursor < bytes.len() && bytes[cursor].is_ascii_whitespace() {
                cursor += 1;
            }
            let is_key = cursor < bytes.len()
                && bytes[cursor] == b':'
                && matches!(
                    stack.last(),
                    Some(frame) if frame.kind == FrameKind::Object
                );
            if is_key {
                let frame = stack.last_mut().expect("object frame");
                if !frame.keys.insert(parsed) {
                    return Err(ProtocolError::envelope(
                        "envelope_duplicate_key",
                        "envelope contains a duplicate JSON key",
                    ));
                }
            }
            index = next;
            continue;
        }
        if text[index..].starts_with("true")
            || text[index..].starts_with("false")
            || text[index..].starts_with("null")
        {
            if text[index..].starts_with("true") {
                index += 4;
            } else if text[index..].starts_with("false") {
                index += 5;
            } else {
                index += 4;
            }
            continue;
        }
        // Numbers and structural bytes advance one byte; malformed
        // content surfaces through the later serde_json parse.
        index += 1;
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FrameKind {
    Object,
    Array,
}

struct ObjectFrame {
    kind: FrameKind,
    keys: HashSet<String>,
}

/// Parse one JSON string starting at the opening quote. Returns the
/// unescaped value and the byte offset just past the closing quote.
fn parse_json_string(
    text: &str,
    start: usize,
) -> Result<(String, usize), String> {
    let bytes = text.as_bytes();
    if bytes.get(start) != Some(&b'"') {
        return Err("expected opening quote".to_owned());
    }
    let mut output = String::new();
    let mut index = start + 1;
    while index < bytes.len() {
        let byte = bytes[index];
        if byte == b'"' {
            return Ok((output, index + 1));
        }
        if byte == b'\\' {
            index += 1;
            let escape = *bytes.get(index).ok_or_else(|| {
                "unterminated escape in JSON string".to_owned()
            })?;
            match escape {
                b'"' => output.push('"'),
                b'\\' => output.push('\\'),
                b'/' => output.push('/'),
                b'b' => output.push('\u{0008}'),
                b'f' => output.push('\u{000C}'),
                b'n' => output.push('\n'),
                b'r' => output.push('\r'),
                b't' => output.push('\t'),
                b'u' => {
                    if index + 4 >= bytes.len() {
                        return Err("unterminated unicode escape".to_owned());
                    }
                    let hex = &text[index + 1..index + 5];
                    let code = u32::from_str_radix(hex, 16)
                        .map_err(|_| "invalid unicode escape".to_owned())?;
                    let character = char::from_u32(code)
                        .ok_or_else(|| "invalid unicode scalar".to_owned())?;
                    output.push(character);
                    index += 4;
                }
                _ => {
                    return Err("invalid escape in JSON string".to_owned());
                }
            }
            index += 1;
        } else {
            let character = text[index..]
                .chars()
                .next()
                .ok_or_else(|| "unterminated JSON string".to_owned())?;
            output.push(character);
            index += character.len_utf8();
        }
    }
    Err("unterminated JSON string".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_ops_are_a_valid_no_op() {
        let envelope = parse_envelope(
            r#"{"schema_version":1,"expect":"sha256:abc","ops":[]}"#,
        )
        .expect("empty ops parse");
        assert!(envelope.ops.is_empty());
    }

    #[test]
    fn duplicate_keys_refuse_the_envelope() {
        let error = parse_envelope(
            r#"{"schema_version":1,"schema_version":1,"expect":"sha256:x","ops":[]}"#,
        )
        .expect_err("duplicate top-level keys refuse");
        assert_eq!(error.code, "envelope_duplicate_key");
        let nested = parse_envelope(
            r#"{"schema_version":1,"expect":"sha256:x","ops":[{"op":"field.move","field":"a","field":"b","position":0}]}"#,
        )
        .expect_err("duplicate op keys refuse");
        assert_eq!(nested.code, "envelope_duplicate_key");
    }

    #[test]
    fn unknown_versions_ops_and_keys_refuse() {
        let version = parse_envelope(
            r#"{"schema_version":2,"expect":"sha256:x","ops":[]}"#,
        )
        .expect_err("unknown version refuses");
        assert_eq!(version.code, "envelope_unknown_version");
        let op = parse_envelope(
            r#"{"schema_version":1,"expect":"sha256:x","ops":[{"op":"field.rename","field":"a"}]}"#,
        )
        .expect_err("unknown op refuses");
        assert_eq!(op.code, "envelope_unknown_op");
        let key = parse_envelope(
            r#"{"schema_version":1,"expect":"sha256:x","ops":[{"op":"field.move","field":"a","position":0,"extra":1}]}"#,
        )
        .expect_err("unknown key refuses");
        assert_eq!(key.code, "envelope_unknown_key");
    }

    #[test]
    fn wrong_types_and_malformed_input_refuse() {
        let wrong = parse_envelope(
            r#"{"schema_version":1,"expect":"sha256:x","ops":[{"op":"field.move","field":"a","position":"0"}]}"#,
        )
        .expect_err("string position refuses");
        assert_eq!(wrong.code, "envelope_wrong_type");
        let malformed = parse_envelope(r#"{"schema_version":1,"#)
            .expect_err("truncated JSON refuses");
        assert_eq!(malformed.code, "envelope_malformed");
    }

    #[test]
    fn stdin_convention_is_documented_for_the_command_layer() {
        // `-i -` reads stdin and omitted `-i` means the same; the
        // envelope itself carries no file semantics, only ops.
        let envelope = parse_envelope(
            r#"{"schema_version":1,"expect":"sha256:x","ops":[{"op":"field.move","field":"a","position":0}]}"#,
        )
        .expect("move parses");
        assert_eq!(envelope.ops[0].name(), "field.move");
    }

    #[test]
    fn restoration_metadata_is_typed_and_limited_to_schema_key_paths() {
        let envelope = parse_envelope(
            r#"{"schema_version":1,"expect":"sha256:x","ops":[{"op":"field.set","field":"status","set":{"label":"State"},"restore":{"spans":[{"key_path":"record.fields.status.label","expected":"      label: State\n","prior":"      label: \"Status\"\n"}]}}]}"#,
        )
        .expect("scoped restoration parses");
        let span = &envelope.restorations[0].as_ref().unwrap().spans[0];
        assert_eq!(span.key_path, "record.fields.status.label");
        assert_eq!(span.expected.as_deref(), Some("      label: State\n"));

        let forged = parse_envelope(
            r#"{"schema_version":1,"expect":"sha256:x","ops":[{"op":"field.set","field":"status","set":{"label":"State"},"restore":{"spans":[{"key_path":"../summary","expected":null,"prior":"x"}]}}]}"#,
        )
        .expect_err("arbitrary path refuses");
        assert_eq!(forged.code, "envelope_wrong_type");
    }
}
