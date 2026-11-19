//! Guarded JSON envelope for `bob periods update -i`.
//!
//! Parsing is side-effect free. Duplicate keys are rejected before
//! `serde_json` can collapse them, and every operation has a closed shape.

use serde_json::{Map, Value};

pub(crate) const SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProtocolError {
    pub(crate) code: &'static str,
    pub(crate) message: String,
    pub(crate) op_index: Option<usize>,
    pub(crate) hint: Option<String>,
}

impl std::fmt::Display for ProtocolError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.op_index {
            Some(index) => write!(
                formatter,
                "op {index} [{}]: {}",
                self.code, self.message
            ),
            None => write!(formatter, "[{}]: {}", self.code, self.message),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Envelope {
    pub(crate) expect: Option<String>,
    pub(crate) ops: Vec<Op>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Op {
    IntentionsSet {
        values: Vec<String>,
    },
    IntentionsInsert {
        index: usize,
        value: String,
    },
    IntentionsReplace {
        index: usize,
        value: String,
    },
    IntentionsMove {
        from: usize,
        to: usize,
    },
    IntentionsRemove {
        index: usize,
    },
    ReflectionSet {
        value: String,
    },
    ReflectionUnset,
    SummarySet {
        value: String,
    },
    SummaryUnset,
    /// Emitted in `inverse_ops` so Undo can restore an earlier summary and
    /// its agent stamps as one guarded operation.
    SummaryRestore {
        present: bool,
        source: String,
        value: Option<String>,
        digest: Option<String>,
        drafted: Option<String>,
    },
}

pub(crate) fn parse(
    text: &str,
    allow_missing_expect: bool,
) -> Result<Envelope, ProtocolError> {
    crate::native::albums::update::protocol::check_duplicate_keys(text)
        .map_err(|error| {
            envelope_error("envelope_duplicate_key", error.message)
        })?;
    let value: Value = serde_json::from_str(text).map_err(|error| {
        envelope_error(
            "envelope_malformed",
            format!("malformed JSON envelope: {error}"),
        )
    })?;
    let object = value.as_object().ok_or_else(|| {
        envelope_error("envelope_wrong_type", "envelope must be a JSON object")
    })?;
    reject_unknown(object, &["schema_version", "expect", "ops"], None)?;

    let version = object
        .get("schema_version")
        .and_then(Value::as_u64)
        .ok_or_else(|| {
            envelope_error(
                "envelope_wrong_type",
                "schema_version must be an integer",
            )
        })?;
    if version != u64::from(SCHEMA_VERSION) {
        return Err(envelope_error(
            "envelope_unknown_version",
            format!("unsupported schema_version {version}; expected {SCHEMA_VERSION}"),
        ));
    }

    let expect = match object.get("expect") {
        Some(value) => {
            let expect = value.as_str().ok_or_else(|| {
                envelope_error(
                    "envelope_wrong_type",
                    "expect must be a string semantic fingerprint",
                )
            })?;
            if expect.trim().is_empty() || !expect.starts_with("sha256:") {
                return Err(envelope_error(
                    "envelope_wrong_type",
                    "expect must look like sha256:…",
                ));
            }
            Some(expect.to_owned())
        }
        None if allow_missing_expect => None,
        None => {
            return Err(envelope_error(
                "envelope_missing_key",
                "an applied update requires expect; dry runs may omit it",
            ));
        }
    };
    let raw_ops = object.get("ops").ok_or_else(|| {
        envelope_error("envelope_missing_key", "envelope requires ops")
    })?;
    let raw_ops = raw_ops.as_array().ok_or_else(|| {
        envelope_error("envelope_wrong_type", "ops must be an array")
    })?;
    let ops = raw_ops
        .iter()
        .enumerate()
        .map(|(index, raw)| parse_op(raw, index))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Envelope { expect, ops })
}

fn parse_op(raw: &Value, index: usize) -> Result<Op, ProtocolError> {
    let object = raw.as_object().ok_or_else(|| {
        op_error(
            index,
            "envelope_wrong_type",
            "each op must be a JSON object",
        )
    })?;
    let name = object.get("op").and_then(Value::as_str).ok_or_else(|| {
        op_error(
            index,
            "envelope_missing_key",
            "each op requires a string op name",
        )
    })?;
    match name {
        "intentions.set" => {
            reject_unknown(object, &["op", "values"], Some(index))?;
            let values = object
                .get("values")
                .and_then(Value::as_array)
                .ok_or_else(|| {
                    op_error(
                        index,
                        "envelope_wrong_type",
                        "values must be an array of strings",
                    )
                })?
                .iter()
                .map(|value| {
                    value.as_str().map(str::to_owned).ok_or_else(|| {
                        op_error(
                            index,
                            "envelope_wrong_type",
                            "each intention value must be a string",
                        )
                    })
                })
                .collect::<Result<_, _>>()?;
            Ok(Op::IntentionsSet { values })
        }
        "intentions.insert" => {
            reject_unknown(object, &["op", "index", "value"], Some(index))?;
            Ok(Op::IntentionsInsert {
                index: required_index(object, "index", index)?,
                value: required_string(object, "value", index)?,
            })
        }
        "intentions.replace" => {
            reject_unknown(object, &["op", "index", "value"], Some(index))?;
            Ok(Op::IntentionsReplace {
                index: required_index(object, "index", index)?,
                value: required_string(object, "value", index)?,
            })
        }
        "intentions.move" => {
            reject_unknown(object, &["op", "from", "to"], Some(index))?;
            Ok(Op::IntentionsMove {
                from: required_index(object, "from", index)?,
                to: required_index(object, "to", index)?,
            })
        }
        "intentions.remove" => {
            reject_unknown(object, &["op", "index"], Some(index))?;
            Ok(Op::IntentionsRemove {
                index: required_index(object, "index", index)?,
            })
        }
        "reflection.set" => {
            reject_unknown(object, &["op", "value"], Some(index))?;
            Ok(Op::ReflectionSet {
                value: required_string(object, "value", index)?,
            })
        }
        "reflection.unset" => {
            reject_unknown(object, &["op"], Some(index))?;
            Ok(Op::ReflectionUnset)
        }
        "summary.set" => {
            reject_unknown(object, &["op", "value"], Some(index))?;
            Ok(Op::SummarySet {
                value: required_string(object, "value", index)?,
            })
        }
        "summary.unset" => {
            reject_unknown(object, &["op"], Some(index))?;
            Ok(Op::SummaryUnset)
        }
        "summary.restore" => {
            reject_unknown(
                object,
                &["op", "present", "source", "value", "digest", "drafted"],
                Some(index),
            )?;
            let present = object
                .get("present")
                .and_then(Value::as_bool)
                .ok_or_else(|| {
                    op_error(
                        index,
                        "envelope_wrong_type",
                        "present must be a boolean",
                    )
                })?;
            let source = required_string(object, "source", index)?;
            if !["absent", "frontmatter", "legacy", "legacy_review"]
                .contains(&source.as_str())
            {
                return Err(op_error(
                    index,
                    "envelope_wrong_type",
                    "source is not a supported summary source",
                ));
            }
            Ok(Op::SummaryRestore {
                present,
                source,
                value: nullable_string(object, "value", index)?,
                digest: nullable_string(object, "digest", index)?,
                drafted: nullable_string(object, "drafted", index)?,
            })
        }
        _ => Err(ProtocolError {
            code: "envelope_unknown_op",
            message: format!("unknown op {name:?}"),
            op_index: Some(index),
            hint: Some(
                "See `bob periods update --help` for supported operations."
                    .to_owned(),
            ),
        }),
    }
}

pub(crate) fn op_json(op: &Op) -> Value {
    match op {
        Op::IntentionsSet { values } => {
            serde_json::json!({"op":"intentions.set","values":values})
        }
        Op::IntentionsInsert { index, value } => {
            serde_json::json!({"op":"intentions.insert","index":index,"value":value})
        }
        Op::IntentionsReplace { index, value } => {
            serde_json::json!({"op":"intentions.replace","index":index,"value":value})
        }
        Op::IntentionsMove { from, to } => {
            serde_json::json!({"op":"intentions.move","from":from,"to":to})
        }
        Op::IntentionsRemove { index } => {
            serde_json::json!({"op":"intentions.remove","index":index})
        }
        Op::ReflectionSet { value } => {
            serde_json::json!({"op":"reflection.set","value":value})
        }
        Op::ReflectionUnset => serde_json::json!({"op":"reflection.unset"}),
        Op::SummarySet { value } => {
            serde_json::json!({"op":"summary.set","value":value})
        }
        Op::SummaryUnset => serde_json::json!({"op":"summary.unset"}),
        Op::SummaryRestore {
            present,
            source,
            value,
            digest,
            drafted,
        } => serde_json::json!({
            "op":"summary.restore", "present":present, "source":source,
            "value":value, "digest":digest, "drafted":drafted,
        }),
    }
}

fn reject_unknown(
    object: &Map<String, Value>,
    allowed: &[&str],
    index: Option<usize>,
) -> Result<(), ProtocolError> {
    if let Some(key) =
        object.keys().find(|key| !allowed.contains(&key.as_str()))
    {
        return Err(match index {
            Some(index) => op_error(
                index,
                "envelope_unknown_key",
                format!("unknown op key {key:?}"),
            ),
            None => envelope_error(
                "envelope_unknown_key",
                format!("unknown envelope key {key:?}"),
            ),
        });
    }
    Ok(())
}

fn required_string(
    object: &Map<String, Value>,
    key: &str,
    index: usize,
) -> Result<String, ProtocolError> {
    object
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| {
            op_error(
                index,
                "envelope_wrong_type",
                format!("{key} must be a string"),
            )
        })
}

fn nullable_string(
    object: &Map<String, Value>,
    key: &str,
    index: usize,
) -> Result<Option<String>, ProtocolError> {
    let value = object.get(key).ok_or_else(|| {
        op_error(
            index,
            "envelope_missing_key",
            format!("summary.restore requires {key}"),
        )
    })?;
    match value {
        Value::Null => Ok(None),
        Value::String(value) => Ok(Some(value.clone())),
        _ => Err(op_error(
            index,
            "envelope_wrong_type",
            format!("{key} must be a string or null"),
        )),
    }
}

fn required_index(
    object: &Map<String, Value>,
    key: &str,
    index: usize,
) -> Result<usize, ProtocolError> {
    object
        .get(key)
        .and_then(Value::as_u64)
        .and_then(|value| usize::try_from(value).ok())
        .ok_or_else(|| {
            op_error(
                index,
                "envelope_wrong_type",
                format!("{key} must be a non-negative integer"),
            )
        })
}

fn envelope_error(
    code: &'static str,
    message: impl Into<String>,
) -> ProtocolError {
    ProtocolError {
        code,
        message: message.into(),
        op_index: None,
        hint: None,
    }
}

fn op_error(
    index: usize,
    code: &'static str,
    message: impl Into<String>,
) -> ProtocolError {
    ProtocolError {
        code,
        message: message.into(),
        op_index: Some(index),
        hint: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_ordered_operations_and_empty_no_op() {
        let envelope = parse(
            r#"{"schema_version":1,"expect":"sha256:abc","ops":[{"op":"intentions.insert","index":0,"value":"Plan"},{"op":"reflection.set","value":"Meaning"}]}"#,
            false,
        )
        .unwrap();
        assert_eq!(envelope.ops.len(), 2);
        let empty = parse(r#"{"schema_version":1,"ops":[]}"#, true).unwrap();
        assert!(empty.expect.is_none());
        assert!(empty.ops.is_empty());
    }

    #[test]
    fn refuses_duplicate_unknown_and_missing_expect_keys() {
        for (text, allow_missing, code) in [
            (
                r#"{"schema_version":1,"schema_version":1,"ops":[]}"#,
                true,
                "envelope_duplicate_key",
            ),
            (
                r#"{"schema_version":1,"ops":[],"extra":true}"#,
                true,
                "envelope_unknown_key",
            ),
            (
                r#"{"schema_version":1,"ops":[]}"#,
                false,
                "envelope_missing_key",
            ),
            (
                r#"{"schema_version":1,"ops":[{"op":"intentions.set","values":[],"extra":1}]}"#,
                true,
                "envelope_unknown_key",
            ),
        ] {
            assert_eq!(parse(text, allow_missing).unwrap_err().code, code);
        }
    }
}
