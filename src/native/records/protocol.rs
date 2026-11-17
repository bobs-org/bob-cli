//! Guarded JSON envelope for `bob records update -i`.
//!
//! This module parses the envelope once, following the
//! `bob albums update` protocol rules: duplicate keys anywhere refuse, and
//! unknown keys, versions, operations, fields, and wrong types are typed
//! refusals. It performs no vault I/O and never plans or writes. An empty
//! operation list is a valid no-op.

use std::collections::HashSet;

use serde_json::Value as JsonValue;

use super::super::albums::update::protocol::check_duplicate_keys as check_duplicates;

pub(crate) const SCHEMA_VERSION: u32 = 1;

/// Structured envelope failure. Semantic refusals from planning (unknown
/// field, invalid value, required empty, stale record) reuse this shape so
/// the update command prints one typed error object.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProtocolError {
    pub(crate) code: &'static str,
    pub(crate) message: String,
    pub(crate) op_index: Option<usize>,
    pub(crate) field: Option<String>,
    pub(crate) hint: Option<String>,
}

impl ProtocolError {
    fn envelope(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            op_index: None,
            field: None,
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
            field: None,
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
            field: None,
            hint: Some(hint.into()),
        }
    }

    fn field_hint(
        code: &'static str,
        index: usize,
        field: impl Into<String>,
        message: impl Into<String>,
        hint: impl Into<String>,
    ) -> Self {
        Self {
            code,
            message: message.into(),
            op_index: Some(index),
            field: Some(field.into()),
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
}

/// The two v1 operations. `field.set` carries the raw JSON value so the
/// caller converts it against the field schema (numbers stay numeric,
/// list members stay literal).
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Op {
    FieldSet { field: String, value: JsonValue },
    FieldUnset { field: String },
}

impl Op {
    pub(crate) fn name(&self) -> &'static str {
        match self {
            Self::FieldSet { .. } => "field.set",
            Self::FieldUnset { .. } => "field.unset",
        }
    }

    pub(crate) fn field(&self) -> &str {
        match self {
            Self::FieldSet { field, .. } | Self::FieldUnset { field } => field,
        }
    }
}

/// Parse and strictly validate the JSON envelope.
pub(crate) fn parse_envelope(text: &str) -> Result<Envelope, ProtocolError> {
    // The shared duplicate-key scanner reports album-protocol codes; the
    // record envelope keeps its own `envelope_*` code family.
    if let Err(error) = check_duplicates(text) {
        return Err(ProtocolError::envelope(
            "envelope_duplicate_key",
            error.message,
        ));
    }
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
                format!(
                    "unknown envelope key {key:?}; expected one of {ALLOWED_TOP:?}"
                ),
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
    let mut seen_fields = HashSet::new();
    for (index, raw) in ops_array.iter().enumerate() {
        let op = parse_op(raw, index)?;
        if !seen_fields.insert(op.field().to_owned()) {
            return Err(ProtocolError::op(
                "envelope_duplicate_field",
                index,
                format!(
                    "field {:?} appears in more than one op; each field may appear at most once",
                    op.field(),
                ),
            ));
        }
        ops.push(op);
    }
    Ok(Envelope {
        expect: expect.to_owned(),
        ops,
    })
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
        "field.set" => parse_field_set(mapping, index),
        "field.unset" => parse_field_unset(mapping, index),
        _ => Err(ProtocolError::op_hint(
            "envelope_unknown_op",
            index,
            format!("unknown op {op_name:?}"),
            "expected one of field.set, field.unset",
        )),
    }
}

fn reject_unknown_op_keys(
    mapping: &serde_json::Map<String, JsonValue>,
    allowed: &[&str],
    index: usize,
) -> Result<(), ProtocolError> {
    for key in mapping.keys() {
        if !allowed.contains(&key.as_str()) {
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

fn required_field(
    mapping: &serde_json::Map<String, JsonValue>,
    index: usize,
) -> Result<String, ProtocolError> {
    let value = mapping.get("field").ok_or_else(|| {
        ProtocolError::op(
            "envelope_missing_key",
            index,
            "op requires \"field\"",
        )
    })?;
    let field = value.as_str().ok_or_else(|| {
        ProtocolError::op(
            "envelope_wrong_type",
            index,
            "field must be a string",
        )
    })?;
    if field.trim().is_empty() {
        return Err(ProtocolError::op(
            "envelope_wrong_type",
            index,
            "field must be a non-empty string",
        ));
    }
    Ok(field.to_owned())
}

fn parse_field_set(
    mapping: &serde_json::Map<String, JsonValue>,
    index: usize,
) -> Result<Op, ProtocolError> {
    reject_unknown_op_keys(mapping, &["op", "field", "value"], index)?;
    let field = required_field(mapping, index)?;
    let value = mapping.get("value").ok_or_else(|| {
        ProtocolError::op(
            "envelope_missing_key",
            index,
            "field.set requires \"value\"",
        )
    })?;
    if let Some(text) = value.as_str()
        && text.is_empty()
    {
        return Err(ProtocolError::field_hint(
            "envelope_empty_value",
            index,
            field.clone(),
            format!(
                "field {field:?} has an empty string value; use field.unset to clear it"
            ),
            format!(
                "replace with {{\"op\":\"field.unset\",\"field\":{field:?}}}"
            ),
        ));
    }
    Ok(Op::FieldSet {
        field,
        value: value.clone(),
    })
}

fn parse_field_unset(
    mapping: &serde_json::Map<String, JsonValue>,
    index: usize,
) -> Result<Op, ProtocolError> {
    reject_unknown_op_keys(mapping, &["op", "field"], index)?;
    let field = required_field(mapping, index)?;
    Ok(Op::FieldUnset { field })
}

/// Convert parsed ops into planner inputs against the record schema.
/// Values are typed JSON: strings for text, choice keys, dates, links, and
/// URLs; a number or numeric string for numbers; a string array for list
/// fields (numbers allowed inside for number lists). List members stay
/// literal and are never comma-split. Unknown fields become plain inputs so
/// planning refuses `record_unknown_field`; empty strings never reach here.
pub(crate) fn ops_to_inputs(
    record_type: &super::schema::RecordType,
    ops: &[Op],
) -> Result<Vec<super::schema::RecordFieldInput>, ProtocolError> {
    ops.iter()
        .enumerate()
        .map(|(index, op)| match op {
            Op::FieldUnset { field } => Ok(super::schema::RecordFieldInput {
                key: field.clone(),
                state: super::schema::FieldInputState::SuppliedEmpty,
                source_range: None,
            }),
            Op::FieldSet { field, value } => {
                value_state(record_type, field, value, index).map(|state| {
                    super::schema::RecordFieldInput {
                        key: field.clone(),
                        state,
                        source_range: None,
                    }
                })
            }
        })
        .collect()
}

fn value_state(
    record_type: &super::schema::RecordType,
    field: &str,
    value: &JsonValue,
    index: usize,
) -> Result<super::schema::FieldInputState, ProtocolError> {
    use super::schema::FieldInputState;
    let Some(definition) = record_type.field(field) else {
        // Unknown fields refuse later as `record_unknown_field`; render
        // the JSON value plainly without splitting.
        let rendered = match value {
            JsonValue::String(text) => text.clone(),
            JsonValue::Array(_) => value.to_string(),
            _ => value.to_string(),
        };
        return Ok(FieldInputState::SuppliedValues(vec![rendered]));
    };
    if definition.list {
        let items = value.as_array().ok_or_else(|| {
            ProtocolError::op(
                "envelope_wrong_type",
                index,
                format!("list field {field:?} needs a string array"),
            )
        })?;
        let mut members = Vec::with_capacity(items.len());
        for item in items {
            match item {
                JsonValue::String(text) => members.push(text.clone()),
                JsonValue::Number(number)
                    if matches!(
                        definition.kind,
                        super::schema::FieldKind::Number
                    ) =>
                {
                    members.push(number.to_string());
                }
                _ => {
                    return Err(ProtocolError::op(
                        "envelope_wrong_type",
                        index,
                        format!("list field {field:?} needs a string array"),
                    ));
                }
            }
        }
        return Ok(FieldInputState::SuppliedLiteralValues(members));
    }
    match value {
        JsonValue::String(text) => {
            Ok(FieldInputState::SuppliedValues(vec![text.clone()]))
        }
        JsonValue::Number(number)
            if matches!(definition.kind, super::schema::FieldKind::Number) =>
        {
            Ok(FieldInputState::SuppliedValues(vec![number.to_string()]))
        }
        _ => Err(ProtocolError::op(
            "envelope_wrong_type",
            index,
            format!(
                "{} field {field:?} needs a {}",
                definition.kind.kind_name(),
                expected_json_shape(definition),
            ),
        )),
    }
}

fn expected_json_shape(
    definition: &super::schema::RecordField,
) -> &'static str {
    match definition.kind {
        super::schema::FieldKind::Number => "number or numeric string value",
        _ => "string value",
    }
}

#[cfg(test)]
mod tests {
    use super::parse_envelope;

    fn envelope(ops: &str) -> String {
        format!(
            "{{\"schema_version\":1,\"expect\":\"sha256:abc\",\"ops\":[{ops}]}}"
        )
    }

    #[test]
    fn empty_ops_is_a_valid_noop() {
        let parsed = parse_envelope(&envelope("")).expect("empty ops");
        assert!(parsed.ops.is_empty());
    }

    #[test]
    fn duplicate_json_keys_refuse() {
        let text = "{\"schema_version\":1,\"expect\":\"sha256:abc\",\"ops\":[],\"ops\":[]}";
        let error = parse_envelope(text).expect_err("duplicate key");
        assert_eq!(error.code, "envelope_duplicate_key");
    }

    #[test]
    fn duplicate_fields_refuse() {
        let text = envelope(
            "{\"op\":\"field.set\",\"field\":\"status\",\"value\":\"liked\"},{\"op\":\"field.unset\",\"field\":\"status\"}",
        );
        let error = parse_envelope(&text).expect_err("duplicate field");
        assert_eq!(error.code, "envelope_duplicate_field");
    }

    #[test]
    fn unknown_op_version_and_keys_refuse() {
        let error =
            parse_envelope(&envelope("{\"op\":\"field.add\",\"field\":\"x\"}"))
                .expect_err("unknown op");
        assert_eq!(error.code, "envelope_unknown_op");
        let versioned =
            "{\"schema_version\":2,\"expect\":\"sha256:abc\",\"ops\":[]}";
        let error = parse_envelope(versioned).expect_err("unknown version");
        assert_eq!(error.code, "envelope_unknown_version");
        let keyed = "{\"schema_version\":1,\"expect\":\"sha256:abc\",\"ops\":[],\"extra\":1}";
        let error = parse_envelope(keyed).expect_err("unknown key");
        assert_eq!(error.code, "envelope_unknown_key");
    }

    #[test]
    fn empty_string_set_points_to_unset() {
        let text =
            envelope("{\"op\":\"field.set\",\"field\":\"url\",\"value\":\"\"}");
        let error = parse_envelope(&text).expect_err("empty string");
        assert_eq!(error.code, "envelope_empty_value");
        assert_eq!(error.field.as_deref(), Some("url"));
        assert!(error.hint.as_deref().unwrap_or("").contains("field.unset"));
    }

    #[test]
    fn unset_with_value_refuses() {
        let text = envelope(
            "{\"op\":\"field.unset\",\"field\":\"url\",\"value\":\"x\"}",
        );
        let error = parse_envelope(&text).expect_err("unset value");
        assert_eq!(error.code, "envelope_unknown_key");
    }
}
