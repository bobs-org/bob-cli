//! Typed update envelope for the guarded bank config writer.
//!
//! Parses the JSON envelope once, rejects duplicate keys, unknown
//! versions/operations/keys, and wrong types with structured diagnostics.
//! Labels never reach argv. An empty `ops` list is a valid no-op.

use std::collections::HashSet;

use serde_json::Value as JsonValue;

use crate::native::bank::config;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Anchor {
    Before(String),
    After(String),
}

pub(crate) const SCHEMA_VERSION: u32 = 1;

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
            None => write!(formatter, "[{}]: {}", self.code, self.message),
        }
    }
}

impl std::error::Error for ProtocolError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Envelope {
    pub(crate) expect: String,
    pub(crate) ops: Vec<Op>,
    pub(crate) restorations: Vec<Option<Restoration>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct Restoration {
    pub(crate) spans: Vec<RestoreSpan>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RestoreSpan {
    pub(crate) key_path: String,
    pub(crate) expected: Option<String>,
    pub(crate) prior: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Op {
    CategoryAdd {
        label: String,
        group: Option<String>,
        hotkey: Option<char>,
        hidden: Option<bool>,
        place: Option<Anchor>,
        restore_key: Option<String>,
    },
    CategorySet {
        key: String,
        label: Option<String>,
        group: Option<String>,
        hotkey: Option<char>,
        hidden: Option<bool>,
        unset: Vec<UnsetField>,
    },
    CategoryReset {
        key: String,
    },
    CategoryMove {
        key: String,
        place: Anchor,
    },
    CategoryRemove {
        key: String,
    },
    GroupRename {
        group: String,
        to: String,
    },
    GroupMove {
        group: String,
        place: Anchor,
    },
}

impl Op {
    pub(crate) fn name(&self) -> &'static str {
        match self {
            Self::CategoryAdd { .. } => "category.add",
            Self::CategorySet { .. } => "category.set",
            Self::CategoryReset { .. } => "category.reset",
            Self::CategoryMove { .. } => "category.move",
            Self::CategoryRemove { .. } => "category.remove",
            Self::GroupRename { .. } => "group.rename",
            Self::GroupMove { .. } => "group.move",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum UnsetField {
    Hotkey,
    Group,
}

impl UnsetField {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Hotkey => "hotkey",
            Self::Group => "group",
        }
    }
}

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
                "restore paths must name a bank: entry",
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
        ["bank"] => true,
        ["bank", container] => {
            ["accounts", "categories", "rules", "stale_after"]
                .contains(container)
        }
        ["bank", "categories", key] => config::is_alias(key),
        ["bank", "categories", key, field] => {
            config::is_alias(key)
                && ["label", "hotkey", "group", "hidden"].contains(field)
        }
        ["bank", "accounts", alias] => config::is_alias(alias),
        ["bank", "rules", index] => {
            index.bytes().all(|byte| byte.is_ascii_digit())
        }
        _ => false,
    }
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
        "category.add" => parse_category_add(mapping, index),
        "category.set" => parse_category_set(mapping, index),
        "category.reset" => parse_category_reset(mapping, index),
        "category.move" => parse_category_move(mapping, index),
        "category.remove" => parse_category_remove(mapping, index),
        "group.rename" => parse_group_rename(mapping, index),
        "group.move" => parse_group_move(mapping, index),
        _ => Err(ProtocolError::op_hint(
            "envelope_unknown_op",
            index,
            format!("unknown op {op_name:?}"),
            "expected one of category.add, category.set, category.reset, category.move, category.remove, group.rename, group.move",
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
                    mapping.get("op").and_then(JsonValue::as_str).unwrap_or("?")
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
    let text = value.as_str().ok_or_else(|| {
        ProtocolError::op(
            "envelope_wrong_type",
            index,
            format!("{key} must be a string"),
        )
    })?;
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Err(ProtocolError::op(
            "envelope_wrong_type",
            index,
            format!("{key} must be a non-empty string"),
        ));
    }
    Ok(trimmed.to_owned())
}

fn optional_string(
    mapping: &serde_json::Map<String, JsonValue>,
    key: &str,
    index: usize,
) -> Result<Option<String>, ProtocolError> {
    match mapping.get(key) {
        None => Ok(None),
        Some(JsonValue::Null) => Ok(None),
        Some(value) => {
            let text = value.as_str().ok_or_else(|| {
                ProtocolError::op(
                    "envelope_wrong_type",
                    index,
                    format!("{key} must be a string"),
                )
            })?;
            let trimmed = text.trim();
            if trimmed.is_empty() {
                return Err(ProtocolError::op(
                    "envelope_wrong_type",
                    index,
                    format!("{key} must be a non-empty string"),
                ));
            }
            Ok(Some(trimmed.to_owned()))
        }
    }
}

fn optional_bool(
    mapping: &serde_json::Map<String, JsonValue>,
    key: &str,
    index: usize,
) -> Result<Option<bool>, ProtocolError> {
    match mapping.get(key) {
        None | Some(JsonValue::Null) => Ok(None),
        Some(value) => value.as_bool().map(Some).ok_or_else(|| {
            ProtocolError::op(
                "envelope_wrong_type",
                index,
                format!("{key} must be a boolean"),
            )
        }),
    }
}

fn optional_hotkey(
    mapping: &serde_json::Map<String, JsonValue>,
    key: &str,
    index: usize,
) -> Result<Option<char>, ProtocolError> {
    match mapping.get(key) {
        None | Some(JsonValue::Null) => Ok(None),
        Some(value) => parse_hotkey_json(value, key, index).map(Some),
    }
}

fn parse_hotkey_json(
    value: &JsonValue,
    key: &str,
    index: usize,
) -> Result<char, ProtocolError> {
    let ch = if let Some(text) = value.as_str() {
        let trimmed = text.trim();
        let mut chars = trimmed.chars();
        match (chars.next(), chars.next()) {
            (Some(ch), None) => ch,
            _ => {
                return Err(ProtocolError::op(
                    "hotkey_invalid",
                    index,
                    format!("{key} must be one character in [a-z0-9]"),
                ));
            }
        }
    } else if let Some(number) = value.as_u64() {
        if number > 9 {
            return Err(ProtocolError::op(
                "hotkey_invalid",
                index,
                format!("{key} must be one character in [a-z0-9]"),
            ));
        }
        char::from_digit(number as u32, 10).expect("0-9")
    } else {
        return Err(ProtocolError::op(
            "hotkey_invalid",
            index,
            format!("{key} must be one character in [a-z0-9]"),
        ));
    };
    if ch == config::RESERVED_HOTKEY {
        return Err(ProtocolError::op(
            "hotkey_reserved",
            index,
            "hotkey \"q\" is reserved",
        ));
    }
    if !config::is_hotkey_char(ch) {
        return Err(ProtocolError::op(
            "hotkey_invalid",
            index,
            format!("{key} must be one character in [a-z0-9]"),
        ));
    }
    Ok(ch)
}

fn parse_place(
    mapping: &serde_json::Map<String, JsonValue>,
    index: usize,
) -> Result<Option<Anchor>, ProtocolError> {
    let before = optional_string(mapping, "before", index)?;
    let after = optional_string(mapping, "after", index)?;
    match (before, after) {
        (None, None) => Ok(None),
        (Some(_), Some(_)) => Err(ProtocolError::op(
            "envelope_wrong_type",
            index,
            "op takes at most one of before or after",
        )),
        (Some(key), None) => Ok(Some(Anchor::Before(key))),
        (None, Some(key)) => Ok(Some(Anchor::After(key))),
    }
}

fn required_place(
    mapping: &serde_json::Map<String, JsonValue>,
    index: usize,
) -> Result<Anchor, ProtocolError> {
    parse_place(mapping, index)?.ok_or_else(|| {
        ProtocolError::op(
            "envelope_missing_key",
            index,
            "op requires before or after",
        )
    })
}

fn required_key(
    mapping: &serde_json::Map<String, JsonValue>,
    index: usize,
) -> Result<String, ProtocolError> {
    let key = required_string(mapping, "key", index)?;
    if !config::is_alias(&key) {
        return Err(ProtocolError::op(
            "envelope_wrong_type",
            index,
            format!("key {key:?} must match [a-z][a-z0-9_]*"),
        ));
    }
    Ok(key)
}

fn parse_category_add(
    mapping: &serde_json::Map<String, JsonValue>,
    index: usize,
) -> Result<Op, ProtocolError> {
    reject_unknown_op_keys(
        mapping,
        &[
            "op",
            "label",
            "group",
            "hotkey",
            "hidden",
            "after",
            "before",
            "restore_key",
        ],
        index,
    )?;
    let label = required_string(mapping, "label", index)?;
    let group = optional_string(mapping, "group", index)?;
    let hotkey = optional_hotkey(mapping, "hotkey", index)?;
    let hidden = optional_bool(mapping, "hidden", index)?;
    let place = parse_place(mapping, index)?;
    let restore_key = optional_string(mapping, "restore_key", index)?;
    if let Some(key) =
        restore_key.as_deref().filter(|key| !config::is_alias(key))
    {
        return Err(ProtocolError::op(
            "envelope_wrong_type",
            index,
            format!("restore_key {key:?} must match [a-z][a-z0-9_]*"),
        ));
    }
    Ok(Op::CategoryAdd {
        label,
        group,
        hotkey,
        hidden,
        place,
        restore_key,
    })
}

fn parse_category_set(
    mapping: &serde_json::Map<String, JsonValue>,
    index: usize,
) -> Result<Op, ProtocolError> {
    reject_unknown_op_keys(mapping, &["op", "key", "set", "unset"], index)?;
    let key = required_key(mapping, index)?;
    let (label, group, hotkey, hidden) = parse_set_object(mapping, index)?;
    let unset = parse_unset(mapping, index)?;
    if label.is_none()
        && group.is_none()
        && hotkey.is_none()
        && hidden.is_none()
        && unset.is_empty()
    {
        return Err(ProtocolError::op(
            "envelope_missing_key",
            index,
            "category.set requires set and/or unset",
        ));
    }
    if unset.contains(&UnsetField::Hotkey) && hotkey.is_some() {
        return Err(ProtocolError::op(
            "envelope_wrong_type",
            index,
            "cannot set and unset hotkey in the same op",
        ));
    }
    if unset.contains(&UnsetField::Group) && group.is_some() {
        return Err(ProtocolError::op(
            "envelope_wrong_type",
            index,
            "cannot set and unset group in the same op",
        ));
    }
    Ok(Op::CategorySet {
        key,
        label,
        group,
        hotkey,
        hidden,
        unset,
    })
}

fn parse_set_object(
    mapping: &serde_json::Map<String, JsonValue>,
    index: usize,
) -> Result<
    (Option<String>, Option<String>, Option<char>, Option<bool>),
    ProtocolError,
> {
    let Some(value) = mapping.get("set") else {
        return Ok((None, None, None, None));
    };
    let object = value.as_object().ok_or_else(|| {
        ProtocolError::op("envelope_wrong_type", index, "set must be an object")
    })?;
    const ALLOWED: &[&str] = &["label", "group", "hotkey", "hidden"];
    for key in object.keys() {
        if !ALLOWED.contains(&key.as_str()) {
            return Err(ProtocolError::op(
                "envelope_unknown_key",
                index,
                format!("unknown set key {key:?}; expected one of {ALLOWED:?}"),
            ));
        }
    }
    let label = optional_string(object, "label", index)?;
    let group = optional_string(object, "group", index)?;
    let hotkey = optional_hotkey(object, "hotkey", index)?;
    let hidden = optional_bool(object, "hidden", index)?;
    Ok((label, group, hotkey, hidden))
}

fn parse_unset(
    mapping: &serde_json::Map<String, JsonValue>,
    index: usize,
) -> Result<Vec<UnsetField>, ProtocolError> {
    let Some(value) = mapping.get("unset") else {
        return Ok(Vec::new());
    };
    let array = value.as_array().ok_or_else(|| {
        ProtocolError::op(
            "envelope_wrong_type",
            index,
            "unset must be an array of property names",
        )
    })?;
    let mut fields = Vec::new();
    for item in array {
        let name = item.as_str().ok_or_else(|| {
            ProtocolError::op(
                "envelope_wrong_type",
                index,
                "unset entries must be strings",
            )
        })?;
        let field = match name {
            "hotkey" => UnsetField::Hotkey,
            "group" => UnsetField::Group,
            other => {
                return Err(ProtocolError::op(
                    "envelope_unknown_key",
                    index,
                    format!(
                        "unknown unset field {other:?}; expected hotkey or group"
                    ),
                ));
            }
        };
        if fields.contains(&field) {
            return Err(ProtocolError::op(
                "envelope_duplicate_key",
                index,
                format!("unset lists {name:?} more than once"),
            ));
        }
        fields.push(field);
    }
    Ok(fields)
}

fn parse_category_reset(
    mapping: &serde_json::Map<String, JsonValue>,
    index: usize,
) -> Result<Op, ProtocolError> {
    reject_unknown_op_keys(mapping, &["op", "key"], index)?;
    Ok(Op::CategoryReset {
        key: required_key(mapping, index)?,
    })
}

fn parse_category_move(
    mapping: &serde_json::Map<String, JsonValue>,
    index: usize,
) -> Result<Op, ProtocolError> {
    reject_unknown_op_keys(mapping, &["op", "key", "before", "after"], index)?;
    Ok(Op::CategoryMove {
        key: required_key(mapping, index)?,
        place: required_place(mapping, index)?,
    })
}

fn parse_category_remove(
    mapping: &serde_json::Map<String, JsonValue>,
    index: usize,
) -> Result<Op, ProtocolError> {
    reject_unknown_op_keys(mapping, &["op", "key"], index)?;
    Ok(Op::CategoryRemove {
        key: required_key(mapping, index)?,
    })
}

fn parse_group_rename(
    mapping: &serde_json::Map<String, JsonValue>,
    index: usize,
) -> Result<Op, ProtocolError> {
    reject_unknown_op_keys(mapping, &["op", "group", "to"], index)?;
    Ok(Op::GroupRename {
        group: required_string(mapping, "group", index)?,
        to: required_string(mapping, "to", index)?,
    })
}

fn parse_group_move(
    mapping: &serde_json::Map<String, JsonValue>,
    index: usize,
) -> Result<Op, ProtocolError> {
    reject_unknown_op_keys(
        mapping,
        &["op", "group", "before", "after"],
        index,
    )?;
    Ok(Op::GroupMove {
        group: required_string(mapping, "group", index)?,
        place: required_place(mapping, index)?,
    })
}

#[derive(Debug)]
struct ObjectFrame {
    kind: FrameKind,
    keys: HashSet<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FrameKind {
    Object,
    Array,
}

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
        index += 1;
    }
    Ok(())
}

fn parse_json_string(
    text: &str,
    start: usize,
) -> Result<(String, usize), String> {
    let bytes = text.as_bytes();
    let mut index = start + 1;
    let mut out = String::new();
    while index < bytes.len() {
        let byte = bytes[index];
        if byte == b'"' {
            return Ok((out, index + 1));
        }
        if byte == b'\\' {
            index += 1;
            let escaped = bytes.get(index).copied().ok_or_else(|| {
                "unterminated string escape in envelope".to_owned()
            })?;
            match escaped {
                b'"' | b'\\' | b'/' => {
                    out.push(char::from(escaped));
                }
                b'b' => out.push('\u{0008}'),
                b'f' => out.push('\u{000c}'),
                b'n' => out.push('\n'),
                b'r' => out.push('\r'),
                b't' => out.push('\t'),
                b'u' => {
                    let hex =
                        text.get(index + 1..index + 5).ok_or_else(|| {
                            "unterminated unicode escape in envelope".to_owned()
                        })?;
                    let code = u32::from_str_radix(hex, 16).map_err(|_| {
                        "invalid unicode escape in envelope".to_owned()
                    })?;
                    out.push(char::from_u32(code).ok_or_else(|| {
                        "invalid unicode escape in envelope".to_owned()
                    })?);
                    index += 4;
                }
                _ => {
                    return Err("invalid string escape in envelope".to_owned());
                }
            }
            index += 1;
            continue;
        }
        let ch = text[index..]
            .chars()
            .next()
            .ok_or_else(|| "unterminated string in envelope".to_owned())?;
        out.push(ch);
        index += ch.len_utf8();
    }
    Err("unterminated string in envelope".to_owned())
}
