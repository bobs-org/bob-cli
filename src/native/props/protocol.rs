//! Strict JSON envelope for `bob props update`.
//!
//! Duplicate object keys anywhere, unknown fields or ops, wrong types,
//! missing `expect`, and illegal positioning combinations are typed
//! refusals. This layer does no vault I/O and never plans or writes. An
//! empty operation list is a valid no-op.

use serde_json::Value as JsonValue;

use super::{document::PropertyKind, PropsError};

pub(crate) const SCHEMA_VERSION: u32 = 1;

/// Parsed envelope: the semantic fingerprint expectation plus ordered ops.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Envelope {
    pub(crate) expect: String,
    pub(crate) ops: Vec<Op>,
}

/// Neighbor used to place a new or moved entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Position {
    Before(String),
    After(String),
}

/// Narrow framing metadata used only to distinguish removing a newly
/// created frontmatter block from retaining an authored empty block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Framing {
    pub(crate) expected: String,
    pub(crate) prior: String,
}

/// Exact body preimage for the matching-H1 part of a reversible title edit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BodyRestore {
    pub(crate) expected: String,
    pub(crate) prior: String,
}

/// Property span operations and typed setting gestures.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Op {
    SettingSet {
        setting: String,
        value: JsonValue,
    },
    SettingClear {
        setting: String,
    },
    Set {
        key: String,
        value: JsonValue,
        kind: Option<PropertyKind>,
        position: Option<Position>,
    },
    Unset {
        key: String,
    },
    Rename {
        key: String,
        to: String,
    },
    Move {
        key: String,
        position: Position,
    },
    SetYaml {
        key: String,
        yaml: String,
        position: Option<Position>,
    },
    Restore {
        key: String,
        raw: Option<String>,
        position: Option<Position>,
        framing: Option<Framing>,
        body: Option<BodyRestore>,
    },
}

impl Op {
    pub(crate) fn name(&self) -> &'static str {
        match self {
            Self::SettingSet { .. } => "setting.set",
            Self::SettingClear { .. } => "setting.clear",
            Self::Set { .. } => "prop.set",
            Self::Unset { .. } => "prop.unset",
            Self::Rename { .. } => "prop.rename",
            Self::Move { .. } => "prop.move",
            Self::SetYaml { .. } => "prop.set_yaml",
            Self::Restore { .. } => "prop.restore",
        }
    }

    pub(crate) fn key(&self) -> &str {
        match self {
            Self::SettingSet { setting: key, .. }
            | Self::SettingClear { setting: key }
            | Self::Set { key, .. }
            | Self::Unset { key }
            | Self::Rename { key, .. }
            | Self::Move { key, .. }
            | Self::SetYaml { key, .. }
            | Self::Restore { key, .. } => key,
        }
    }

    pub(crate) fn to_json(&self) -> JsonValue {
        let mut mapping = serde_json::Map::new();
        mapping.insert("op".to_owned(), JsonValue::String(self.name().into()));
        match self {
            Self::SettingSet { setting, value } => {
                mapping.insert(
                    "setting".to_owned(),
                    JsonValue::String(setting.clone()),
                );
                mapping.insert("value".to_owned(), value.clone());
            }
            Self::SettingClear { setting } => {
                mapping.insert(
                    "setting".to_owned(),
                    JsonValue::String(setting.clone()),
                );
            }
            Self::Set {
                key,
                value,
                kind,
                position,
            } => {
                mapping
                    .insert("key".to_owned(), JsonValue::String(key.clone()));
                mapping.insert("value".to_owned(), value.clone());
                if let Some(kind) = kind {
                    mapping.insert(
                        "kind".to_owned(),
                        serde_json::to_value(kind).expect("kind is a string"),
                    );
                }
                insert_position(&mut mapping, position.as_ref());
            }
            Self::Unset { key } => {
                mapping
                    .insert("key".to_owned(), JsonValue::String(key.clone()));
            }
            Self::Rename { key, to } => {
                mapping
                    .insert("key".to_owned(), JsonValue::String(key.clone()));
                mapping.insert("to".to_owned(), JsonValue::String(to.clone()));
            }
            Self::Move { key, position } => {
                mapping
                    .insert("key".to_owned(), JsonValue::String(key.clone()));
                insert_position(&mut mapping, Some(position));
            }
            Self::SetYaml {
                key,
                yaml,
                position,
            } => {
                mapping
                    .insert("key".to_owned(), JsonValue::String(key.clone()));
                mapping
                    .insert("yaml".to_owned(), JsonValue::String(yaml.clone()));
                insert_position(&mut mapping, position.as_ref());
            }
            Self::Restore {
                key,
                raw,
                position,
                framing,
                body,
            } => {
                mapping
                    .insert("key".to_owned(), JsonValue::String(key.clone()));
                mapping.insert(
                    "raw".to_owned(),
                    match raw {
                        Some(text) => JsonValue::String(text.clone()),
                        None => JsonValue::Null,
                    },
                );
                insert_position(&mut mapping, position.as_ref());
                if let Some(framing) = framing {
                    mapping.insert(
                        "framing".to_owned(),
                        serde_json::json!({
                            "expected": framing.expected,
                            "prior": framing.prior,
                        }),
                    );
                }
                if let Some(body) = body {
                    mapping.insert(
                        "body".to_owned(),
                        serde_json::json!({
                            "expected": body.expected,
                            "prior": body.prior,
                        }),
                    );
                }
            }
        }
        JsonValue::Object(mapping)
    }
}

fn insert_position(
    mapping: &mut serde_json::Map<String, JsonValue>,
    position: Option<&Position>,
) {
    match position {
        Some(Position::Before(key)) => {
            mapping.insert("before".to_owned(), JsonValue::String(key.clone()));
        }
        Some(Position::After(key)) => {
            mapping.insert("after".to_owned(), JsonValue::String(key.clone()));
        }
        None => {}
    }
}

/// Parse and strictly validate the JSON envelope.
pub(crate) fn parse_envelope(text: &str) -> Result<Envelope, PropsError> {
    if let Err(error) =
        crate::native::albums::update::protocol::check_duplicate_keys(text)
    {
        return Err(envelope_error(error.code, error.message));
    }
    let value: JsonValue = serde_json::from_str(text).map_err(|error| {
        envelope_error(
            "envelope_malformed",
            format!("malformed JSON envelope: {error}"),
        )
    })?;
    let mapping = value.as_object().ok_or_else(|| {
        envelope_error("envelope_wrong_type", "envelope must be a JSON object")
    })?;
    const ALLOWED_TOP: &[&str] = &["schema_version", "expect", "ops"];
    for key in mapping.keys() {
        if !ALLOWED_TOP.contains(&key.as_str()) {
            return Err(envelope_error(
                "envelope_unknown_key",
                format!(
                    "unknown envelope key {key:?}; expected one of {ALLOWED_TOP:?}"
                ),
            ));
        }
    }
    let version_value = mapping.get("schema_version").ok_or_else(|| {
        envelope_error(
            "envelope_missing_key",
            "envelope requires schema_version",
        )
    })?;
    let version = version_value.as_u64().ok_or_else(|| {
        envelope_error(
            "envelope_wrong_type",
            "schema_version must be an integer",
        )
    })?;
    if version != u64::from(SCHEMA_VERSION) {
        return Err(envelope_error(
            "envelope_unknown_version",
            format!(
                "unsupported schema_version {version}; expected {SCHEMA_VERSION}"
            ),
        ));
    }
    let expect_value = mapping.get("expect").ok_or_else(|| {
        envelope_error("envelope_missing_key", "envelope requires expect")
    })?;
    let expect = expect_value.as_str().ok_or_else(|| {
        envelope_error(
            "envelope_wrong_type",
            "expect must be a string semantic fingerprint",
        )
    })?;
    if expect.trim().is_empty() {
        return Err(envelope_error(
            "envelope_wrong_type",
            "expect must be a non-empty fingerprint string",
        ));
    }
    if !expect.starts_with("sha256:") {
        return Err(envelope_error(
            "envelope_wrong_type",
            "expect must look like sha256:…",
        ));
    }
    let ops_value = mapping.get("ops").ok_or_else(|| {
        envelope_error("envelope_missing_key", "envelope requires ops")
    })?;
    let ops_array = ops_value.as_array().ok_or_else(|| {
        envelope_error("envelope_wrong_type", "ops must be an array")
    })?;
    let mut ops = Vec::with_capacity(ops_array.len());
    for (index, raw) in ops_array.iter().enumerate() {
        ops.push(parse_op(raw, index)?);
    }
    Ok(Envelope {
        expect: expect.to_owned(),
        ops,
    })
}

fn parse_op(raw: &JsonValue, index: usize) -> Result<Op, PropsError> {
    let mapping = raw.as_object().ok_or_else(|| {
        op_error(
            "envelope_wrong_type",
            index,
            "each op must be a JSON object",
        )
    })?;
    let op_value = mapping.get("op").ok_or_else(|| {
        op_error("envelope_missing_key", index, "each op requires an op name")
    })?;
    let op_name = op_value.as_str().ok_or_else(|| {
        op_error("envelope_wrong_type", index, "op must be a string")
    })?;
    match op_name {
        "setting.set" | "setting.clear" => parse_setting(mapping, index, op_name == "setting.set"),
        "prop.set" => parse_set(mapping, index),
        "prop.unset" => parse_unset(mapping, index),
        "prop.rename" => parse_rename(mapping, index),
        "prop.move" => parse_move(mapping, index),
        "prop.set_yaml" => parse_set_yaml(mapping, index),
        "prop.restore" => parse_restore(mapping, index),
        _ => Err(op_error(
            "envelope_unknown_op",
            index,
            format!("unknown op {op_name:?}"),
        )
        .with_hint("expected one of prop.set, prop.unset, prop.rename, prop.move, prop.set_yaml, prop.restore, setting.set, setting.clear")),
    }
}

fn parse_setting(
    mapping: &serde_json::Map<String, JsonValue>,
    index: usize,
    set: bool,
) -> Result<Op, PropsError> {
    reject_unknown_op_keys(
        mapping,
        if set {
            &["op", "setting", "value"]
        } else {
            &["op", "setting"]
        },
        index,
    )?;
    let setting = required_string(mapping, index, "setting")?;
    if !super::contract::settings::known_id(&setting) {
        return Err(super::contract::error("prop_bad_value", &setting, index, format!("unknown setting {setting:?}"), "Use an area, project, or reference setting id such as summary, parent, color, task_type, clock, root, ready_cap, task_refresh, project_name_aliases, title, author, published, source, aliases, reading, or file_name."));
    }
    if set {
        let value = mapping.get("value").cloned().ok_or_else(|| {
            op_error(
                "envelope_missing_key",
                index,
                "setting.set requires value",
            )
        })?;
        Ok(Op::SettingSet { setting, value })
    } else {
        Ok(Op::SettingClear { setting })
    }
}

fn parse_set(
    mapping: &serde_json::Map<String, JsonValue>,
    index: usize,
) -> Result<Op, PropsError> {
    reject_unknown_op_keys(
        mapping,
        &["op", "key", "value", "kind", "before", "after"],
        index,
    )?;
    let key = required_key(mapping, index)?;
    let value = mapping.get("value").cloned().ok_or_else(|| {
        op_error("envelope_missing_key", index, "prop.set requires \"value\"")
    })?;
    let kind = optional_kind(mapping, index)?;
    let position = optional_position(mapping, index)?;
    Ok(Op::Set {
        key,
        value,
        kind,
        position,
    })
}

fn parse_unset(
    mapping: &serde_json::Map<String, JsonValue>,
    index: usize,
) -> Result<Op, PropsError> {
    reject_unknown_op_keys(mapping, &["op", "key"], index)?;
    Ok(Op::Unset {
        key: required_key(mapping, index)?,
    })
}

fn parse_rename(
    mapping: &serde_json::Map<String, JsonValue>,
    index: usize,
) -> Result<Op, PropsError> {
    reject_unknown_op_keys(mapping, &["op", "key", "to"], index)?;
    let key = required_key(mapping, index)?;
    let to = required_string(mapping, index, "to")?;
    Ok(Op::Rename { key, to })
}

fn parse_move(
    mapping: &serde_json::Map<String, JsonValue>,
    index: usize,
) -> Result<Op, PropsError> {
    reject_unknown_op_keys(mapping, &["op", "key", "before", "after"], index)?;
    let key = required_key(mapping, index)?;
    let position = required_position(mapping, index)?;
    Ok(Op::Move { key, position })
}

fn parse_set_yaml(
    mapping: &serde_json::Map<String, JsonValue>,
    index: usize,
) -> Result<Op, PropsError> {
    reject_unknown_op_keys(
        mapping,
        &["op", "key", "yaml", "before", "after"],
        index,
    )?;
    let key = required_key(mapping, index)?;
    let yaml = required_string(mapping, index, "yaml")?;
    let position = optional_position(mapping, index)?;
    Ok(Op::SetYaml {
        key,
        yaml,
        position,
    })
}

fn parse_restore(
    mapping: &serde_json::Map<String, JsonValue>,
    index: usize,
) -> Result<Op, PropsError> {
    reject_unknown_op_keys(
        mapping,
        &["op", "key", "raw", "before", "after", "framing", "body"],
        index,
    )?;
    let key = required_key(mapping, index)?;
    let raw_value = mapping.get("raw").ok_or_else(|| {
        op_error(
            "envelope_missing_key",
            index,
            "prop.restore requires \"raw\"",
        )
    })?;
    let raw = match raw_value {
        JsonValue::Null => None,
        JsonValue::String(text) => Some(text.clone()),
        _ => {
            return Err(op_error(
                "envelope_wrong_type",
                index,
                "raw must be a string or null",
            ));
        }
    };
    let position = optional_position(mapping, index)?;
    let framing = optional_framing(mapping, index)?;
    let body = optional_body_restore(mapping, index)?;
    Ok(Op::Restore {
        key,
        raw,
        position,
        framing,
        body,
    })
}

fn optional_body_restore(
    mapping: &serde_json::Map<String, JsonValue>,
    index: usize,
) -> Result<Option<BodyRestore>, PropsError> {
    let Some(value) = mapping.get("body") else {
        return Ok(None);
    };
    let object = value.as_object().ok_or_else(|| {
        op_error("envelope_wrong_type", index, "body must be an object")
    })?;
    if object.len() != 2
        || object
            .keys()
            .any(|key| !matches!(key.as_str(), "expected" | "prior"))
    {
        return Err(op_error(
            "envelope_unknown_key",
            index,
            "body restore requires exactly expected and prior strings",
        ));
    }
    let expected = object
        .get("expected")
        .and_then(JsonValue::as_str)
        .ok_or_else(|| {
            op_error(
                "envelope_wrong_type",
                index,
                "body.expected must be a string",
            )
        })?;
    let prior =
        object
            .get("prior")
            .and_then(JsonValue::as_str)
            .ok_or_else(|| {
                op_error(
                    "envelope_wrong_type",
                    index,
                    "body.prior must be a string",
                )
            })?;
    Ok(Some(BodyRestore {
        expected: expected.to_owned(),
        prior: prior.to_owned(),
    }))
}

fn optional_framing(
    mapping: &serde_json::Map<String, JsonValue>,
    index: usize,
) -> Result<Option<Framing>, PropsError> {
    let Some(value) = mapping.get("framing") else {
        return Ok(None);
    };
    let object = value.as_object().ok_or_else(|| {
        op_error("envelope_wrong_type", index, "framing must be an object")
    })?;
    for key in object.keys() {
        if key != "expected" && key != "prior" {
            return Err(op_error(
                "envelope_unknown_key",
                index,
                format!("unknown framing key {key:?}; expected \"expected\" or \"prior\""),
            ));
        }
    }
    let expected = object
        .get("expected")
        .and_then(JsonValue::as_str)
        .ok_or_else(|| {
            op_error(
                "envelope_wrong_type",
                index,
                "framing.expected must be a string",
            )
        })?
        .to_owned();
    let prior = object
        .get("prior")
        .and_then(JsonValue::as_str)
        .ok_or_else(|| {
            op_error(
                "envelope_wrong_type",
                index,
                "framing.prior must be a string",
            )
        })?
        .to_owned();
    Ok(Some(Framing { expected, prior }))
}

fn optional_kind(
    mapping: &serde_json::Map<String, JsonValue>,
    index: usize,
) -> Result<Option<PropertyKind>, PropsError> {
    let Some(value) = mapping.get("kind") else {
        return Ok(None);
    };
    let text = value.as_str().ok_or_else(|| {
        op_error("envelope_wrong_type", index, "kind must be a string")
    })?;
    serde_json::from_value(JsonValue::String(text.to_owned()))
        .map(Some)
        .map_err(|_| {
            op_error(
                "envelope_wrong_type",
                index,
                format!("unknown property kind {text:?}"),
            )
        })
}

fn required_key(
    mapping: &serde_json::Map<String, JsonValue>,
    index: usize,
) -> Result<String, PropsError> {
    required_string(mapping, index, "key")
}

fn required_string(
    mapping: &serde_json::Map<String, JsonValue>,
    index: usize,
    field: &str,
) -> Result<String, PropsError> {
    let value = mapping.get(field).ok_or_else(|| {
        op_error(
            "envelope_missing_key",
            index,
            format!("op requires {field:?}"),
        )
    })?;
    let text = value.as_str().ok_or_else(|| {
        op_error(
            "envelope_wrong_type",
            index,
            format!("{field} must be a string"),
        )
    })?;
    if text.is_empty() {
        return Err(op_error(
            "envelope_wrong_type",
            index,
            format!("{field} must be a non-empty string"),
        ));
    }
    Ok(text.to_owned())
}

fn optional_position(
    mapping: &serde_json::Map<String, JsonValue>,
    index: usize,
) -> Result<Option<Position>, PropsError> {
    match (mapping.get("before"), mapping.get("after")) {
        (None, None) => Ok(None),
        (Some(_), Some(_)) => Err(op_error(
            "envelope_wrong_type",
            index,
            "before and after cannot both be set",
        )),
        (Some(value), None) => Ok(Some(Position::Before(position_key(
            value, index, "before",
        )?))),
        (None, Some(value)) => {
            Ok(Some(Position::After(position_key(value, index, "after")?)))
        }
    }
}

fn required_position(
    mapping: &serde_json::Map<String, JsonValue>,
    index: usize,
) -> Result<Position, PropsError> {
    optional_position(mapping, index)?.ok_or_else(|| {
        op_error(
            "envelope_missing_key",
            index,
            "prop.move requires before or after",
        )
    })
}

fn position_key(
    value: &JsonValue,
    index: usize,
    field: &str,
) -> Result<String, PropsError> {
    let text = value.as_str().ok_or_else(|| {
        op_error(
            "envelope_wrong_type",
            index,
            format!("{field} must be a string"),
        )
    })?;
    if text.is_empty() {
        return Err(op_error(
            "envelope_wrong_type",
            index,
            format!("{field} must be a non-empty string"),
        ));
    }
    Ok(text.to_owned())
}

fn reject_unknown_op_keys(
    mapping: &serde_json::Map<String, JsonValue>,
    allowed: &[&str],
    index: usize,
) -> Result<(), PropsError> {
    for key in mapping.keys() {
        if !allowed.contains(&key.as_str()) {
            return Err(op_error(
                "envelope_unknown_key",
                index,
                format!(
                    "unknown key {key:?} for op {:?}; expected one of {allowed:?}",
                    mapping.get("op").and_then(JsonValue::as_str)
                ),
            ));
        }
    }
    Ok(())
}

fn envelope_error(code: &str, message: impl Into<String>) -> PropsError {
    PropsError {
        code: code.to_owned(),
        message: message.into(),
        hint: "Repair the JSON envelope and retry.".to_owned(),
        key: None,
        line: None,
        op_index: None,
    }
}

fn op_error(
    code: &str,
    index: usize,
    message: impl Into<String>,
) -> PropsError {
    PropsError {
        code: code.to_owned(),
        message: message.into(),
        hint: "Repair the failing operation and retry.".to_owned(),
        key: None,
        line: None,
        op_index: Some(index),
    }
}

trait WithHint {
    fn with_hint(self, hint: impl Into<String>) -> Self;
}

impl WithHint for PropsError {
    fn with_hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = hint.into();
        self
    }
}

#[cfg(test)]
mod tests {
    use super::{parse_envelope, Op, Position};
    use serde_json::json;

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
    fn unknown_op_version_and_keys_refuse() {
        let error = parse_envelope(&envelope(
            "{\"op\":\"prop.add\",\"key\":\"x\",\"value\":1}",
        ))
        .expect_err("unknown op");
        assert_eq!(error.code, "envelope_unknown_op");
        assert_eq!(error.op_index, Some(0));
        let versioned =
            "{\"schema_version\":2,\"expect\":\"sha256:abc\",\"ops\":[]}";
        let error = parse_envelope(versioned).expect_err("unknown version");
        assert_eq!(error.code, "envelope_unknown_version");
        let keyed = "{\"schema_version\":1,\"expect\":\"sha256:abc\",\"ops\":[],\"extra\":1}";
        let error = parse_envelope(keyed).expect_err("unknown key");
        assert_eq!(error.code, "envelope_unknown_key");
    }

    #[test]
    fn absent_expect_refuses() {
        let text = "{\"schema_version\":1,\"ops\":[]}";
        let error = parse_envelope(text).expect_err("missing expect");
        assert_eq!(error.code, "envelope_missing_key");
    }

    #[test]
    fn before_and_after_together_refuse() {
        let text = envelope(
            "{\"op\":\"prop.set\",\"key\":\"n\",\"value\":1,\"before\":\"a\",\"after\":\"b\"}",
        );
        let error = parse_envelope(&text).expect_err("both positions");
        assert_eq!(error.code, "envelope_wrong_type");
    }

    #[test]
    fn move_requires_a_neighbor() {
        let text = envelope("{\"op\":\"prop.move\",\"key\":\"n\"}");
        let error = parse_envelope(&text).expect_err("move position");
        assert_eq!(error.code, "envelope_missing_key");
    }

    #[test]
    fn restore_accepts_null_raw_and_framing() {
        let text = envelope(
            "{\"op\":\"prop.restore\",\"key\":\"n\",\"raw\":null,\"framing\":{\"expected\":\"---\\n---\\n\",\"prior\":\"\"}}",
        );
        let parsed = parse_envelope(&text).expect("restore");
        match &parsed.ops[0] {
            Op::Restore { raw, framing, .. } => {
                assert!(raw.is_none());
                assert_eq!(framing.as_ref().unwrap().prior, "");
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn parsed_ops_round_trip_to_json() {
        let text = envelope(
            "{\"op\":\"prop.set\",\"key\":\"title\",\"value\":\"Hi\",\"kind\":\"text\",\"before\":\"created\"}",
        );
        let parsed = parse_envelope(&text).expect("set");
        assert_eq!(
            parsed.ops[0].to_json(),
            json!({
                "op": "prop.set",
                "key": "title",
                "value": "Hi",
                "kind": "text",
                "before": "created",
            })
        );
        match &parsed.ops[0] {
            Op::Set {
                position: Some(Position::Before(key)),
                ..
            } => assert_eq!(key, "created"),
            other => panic!("unexpected {other:?}"),
        }
    }
}
