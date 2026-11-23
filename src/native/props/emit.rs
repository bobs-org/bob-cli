//! Scalar, list, and mapping emitters for property writes.
//!
//! Quoting follows the props serialization contract: wikilinks, typed
//! lookalikes, edge whitespace, and YAML punctuation are quoted. New lists
//! are block style; a short existing one-line flow list stays flow when it
//! still fits. Multiline text uses literal block scalars with the chomping
//! indicator that preserves trailing newlines.

use serde_json::Value as JsonValue;
use serde_yaml::{Mapping, Value as YamlValue};

use super::{
    document::{
        datetime_parts, is_date, is_datetime, yaml_contains_merge,
        PropertyKind, ValueStyle,
    },
    PropsError,
};
use crate::native::yaml_spans;

/// Rendered flow-list inner length at or below which an existing one-line
/// flow list is retained.
pub(crate) const SHORT_FLOW_LIST_MAX: usize = 60;

pub(crate) fn json_to_yaml(value: &JsonValue) -> YamlValue {
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

pub(crate) struct TypedValue {
    pub(crate) yaml: YamlValue,
    pub(crate) kind: PropertyKind,
}

/// Interpret a JSON value as a typed property, applying `kind` when given.
pub(crate) fn typed_from_json(
    key: &str,
    value: &JsonValue,
    kind: Option<PropertyKind>,
    op_index: usize,
) -> Result<TypedValue, PropsError> {
    let inferred = infer_kind(key, value);
    let kind = kind.unwrap_or(inferred);
    if kind == PropertyKind::Opaque {
        return Err(bad_value(
            key,
            op_index,
            "opaque values cannot be written with prop.set; use prop.set_yaml or prop.restore",
        ));
    }
    let yaml = coerce_json(key, value, kind, op_index)?;
    Ok(TypedValue { yaml, kind })
}

fn infer_kind(key: &str, value: &JsonValue) -> PropertyKind {
    match value {
        JsonValue::Null => PropertyKind::Null,
        JsonValue::Bool(_) => PropertyKind::Checkbox,
        JsonValue::Number(_) => PropertyKind::Number,
        JsonValue::Array(_) => match key {
            "tags" => PropertyKind::Tags,
            "aliases" => PropertyKind::Aliases,
            _ => PropertyKind::List,
        },
        JsonValue::Object(_) => PropertyKind::Map,
        JsonValue::String(text) => {
            if is_date(text) {
                PropertyKind::Date
            } else if is_datetime(text) {
                PropertyKind::Datetime
            } else {
                PropertyKind::Text
            }
        }
    }
}

fn coerce_json(
    key: &str,
    value: &JsonValue,
    kind: PropertyKind,
    op_index: usize,
) -> Result<YamlValue, PropsError> {
    match kind {
        PropertyKind::Null => {
            if value.is_null() {
                Ok(YamlValue::Null)
            } else {
                Err(bad_value(key, op_index, "null kind requires JSON null"))
            }
        }
        PropertyKind::Checkbox => {
            value.as_bool().map(YamlValue::Bool).ok_or_else(|| {
                bad_value(
                    key,
                    op_index,
                    "checkbox kind requires a JSON boolean",
                )
            })
        }
        PropertyKind::Number => match value {
            JsonValue::Number(_) => Ok(json_to_yaml(value)),
            _ => Err(bad_value(
                key,
                op_index,
                "number kind requires a JSON number",
            )),
        },
        PropertyKind::Text => value
            .as_str()
            .map(|text| YamlValue::String(text.to_owned()))
            .ok_or_else(|| {
                bad_value(key, op_index, "text kind requires a JSON string")
            }),
        PropertyKind::Date => {
            let text = value.as_str().ok_or_else(|| {
                bad_value(key, op_index, "date kind requires a JSON string")
            })?;
            if is_date(text) {
                Ok(YamlValue::String(text.to_owned()))
            } else {
                Err(bad_value(key, op_index, "date values must be YYYY-MM-DD"))
            }
        }
        PropertyKind::Datetime => {
            let text = value.as_str().ok_or_else(|| {
                bad_value(key, op_index, "datetime kind requires a JSON string")
            })?;
            if is_datetime(text) {
                Ok(YamlValue::String(text.to_owned()))
            } else {
                Err(bad_value(
                    key,
                    op_index,
                    "datetime values must be an ISO-like date and time",
                ))
            }
        }
        PropertyKind::List | PropertyKind::Tags | PropertyKind::Aliases => {
            coerce_sequence(key, value, kind, op_index)
        }
        PropertyKind::Map => match value {
            JsonValue::Object(_) => Ok(json_to_yaml(value)),
            _ => {
                Err(bad_value(key, op_index, "map kind requires a JSON object"))
            }
        },
        PropertyKind::Opaque => unreachable!("opaque refused above"),
    }
}

fn coerce_sequence(
    key: &str,
    value: &JsonValue,
    kind: PropertyKind,
    op_index: usize,
) -> Result<YamlValue, PropsError> {
    let items = match value {
        JsonValue::Array(items) => items.clone(),
        JsonValue::String(text)
            if matches!(kind, PropertyKind::Tags | PropertyKind::Aliases) =>
        {
            vec![JsonValue::String(text.clone())]
        }
        _ => {
            return Err(bad_value(
                key,
                op_index,
                "list kinds require a JSON array",
            ));
        }
    };
    let mut sequence = Vec::with_capacity(items.len());
    for item in items {
        if matches!(kind, PropertyKind::Tags | PropertyKind::Aliases) {
            let text = item.as_str().ok_or_else(|| {
                bad_value(
                    key,
                    op_index,
                    "tags and aliases members must be strings",
                )
            })?;
            let stripped = if kind == PropertyKind::Tags {
                text.strip_prefix('#').unwrap_or(text)
            } else {
                text
            };
            sequence.push(YamlValue::String(stripped.to_owned()));
        } else {
            sequence.push(json_to_yaml(&item));
        }
    }
    Ok(YamlValue::Sequence(sequence))
}

fn bad_value(key: &str, op_index: usize, message: &str) -> PropsError {
    PropsError {
        code: "prop_bad_value".to_owned(),
        message: message.to_owned(),
        hint: "Supply a JSON value that matches the requested kind.".to_owned(),
        key: Some(key.to_owned()),
        line: None,
        op_index: Some(op_index),
    }
}

/// Quote a mapping key so integer-like names and punctuation stay strings.
pub(crate) fn emit_key(key: &str) -> String {
    emit_string_scalar(key, true)
}

/// Render a YAML value for splicing after `key:`.
pub(crate) fn emit_value(
    value: &YamlValue,
    kind: PropertyKind,
    style: Option<&ValueStyle>,
    retain_flow_list: bool,
    newline: &str,
) -> String {
    match value {
        YamlValue::Null => "null".to_owned(),
        YamlValue::Bool(flag) => flag.to_string(),
        YamlValue::Number(number) => number.to_string(),
        YamlValue::String(text) => match kind {
            PropertyKind::Datetime => emit_datetime(text, style),
            PropertyKind::Date => text.clone(),
            _ => emit_string_value(text, newline),
        },
        YamlValue::Sequence(items) => {
            emit_sequence(items, kind, retain_flow_list, newline)
        }
        YamlValue::Mapping(mapping) => emit_mapping(mapping, style, newline),
        YamlValue::Tagged(tagged) => {
            emit_value(&tagged.value, kind, style, retain_flow_list, newline)
        }
    }
}

fn emit_string_value(text: &str, newline: &str) -> String {
    if text.contains('\n') || text.contains('\r') {
        emit_block_scalar(text, newline)
    } else {
        emit_string_scalar(text, false)
    }
}

fn emit_string_scalar(text: &str, is_key: bool) -> String {
    let needs_quote = text.is_empty()
        || text != text.trim()
        || text.contains('\n')
        || text.contains('\r')
        || looks_wikilink(text)
        || starts_with_special(text)
        || text.contains(": ")
        || text.contains(" #")
        || looks_coercible(text)
        || (is_key && looks_like_number_or_bool(text));
    if !needs_quote && yaml_accepts_plain_scalar(text) {
        return text.to_owned();
    }
    serde_json::to_string(text).expect("JSON string quoting never fails")
}

fn yaml_accepts_plain_scalar(text: &str) -> bool {
    let Ok(YamlValue::String(parsed)) = serde_yaml::from_str::<YamlValue>(text)
    else {
        return false;
    };
    if parsed != text {
        return false;
    }
    let Ok(YamlValue::Mapping(map)) =
        serde_yaml::from_str::<YamlValue>(&format!("k: {text}"))
    else {
        return false;
    };
    map.get(YamlValue::String("k".to_owned()))
        .and_then(YamlValue::as_str)
        == Some(text)
}

fn looks_wikilink(text: &str) -> bool {
    text.starts_with("[[") && text.contains("]]")
}

fn starts_with_special(text: &str) -> bool {
    text.starts_with(|ch: char| {
        matches!(
            ch,
            '#' | '@'
                | '%'
                | '&'
                | '*'
                | '!'
                | '?'
                | '|'
                | '>'
                | '-'
                | ':'
                | '`'
                | '['
                | '{'
                | ']'
                | '}'
                | ','
                | '\''
                | '"'
        )
    })
}

fn looks_coercible(text: &str) -> bool {
    looks_like_number_or_bool(text) || is_date(text) || is_datetime(text)
}

fn looks_like_number_or_bool(text: &str) -> bool {
    matches!(
        text.to_ascii_lowercase().as_str(),
        "true"
            | "false"
            | "null"
            | "yes"
            | "no"
            | "on"
            | "off"
            | "y"
            | "n"
            | "~"
    ) || (text
        .chars()
        .next()
        .is_some_and(|ch| ch.is_ascii_digit() || matches!(ch, '+' | '-' | '.'))
        && !text.contains(char::is_whitespace)
        && serde_yaml::from_str::<YamlValue>(text)
            .ok()
            .is_some_and(|value| {
                matches!(value, YamlValue::Number(_) | YamlValue::Bool(_))
            }))
}

fn emit_datetime(text: &str, style: Option<&ValueStyle>) -> String {
    let Some((separator, offset)) = datetime_parts(text) else {
        return emit_string_scalar(text, false);
    };
    let Some(style) = style else {
        return text.to_owned();
    };
    let separator = style
        .datetime_separator
        .as_deref()
        .unwrap_or(separator.as_str());
    let date_time = text.split_once(['T', ' ']).map(|(date, rest)| {
        let time = if let Some(stripped) = rest.strip_suffix('Z') {
            stripped
        } else if let Some(index) = rest
            .char_indices()
            .skip(1)
            .find_map(|(i, ch)| matches!(ch, '+' | '-').then_some(i))
        {
            &rest[..index]
        } else {
            rest
        };
        (date, time)
    });
    let Some((date, time)) = date_time else {
        return text.to_owned();
    };
    let restyled = restyle_offset(&offset, style.datetime_offset.as_deref());
    format!("{date}{separator}{time}{restyled}")
}

fn restyle_offset(offset: &str, existing: Option<&str>) -> String {
    let Some(existing) = existing else {
        return offset.to_owned();
    };
    if existing.is_empty() {
        return offset.to_owned();
    }
    if existing == "Z" {
        if offset.is_empty()
            || offset == "Z"
            || offset == "+0000"
            || offset == "+00:00"
        {
            return "Z".to_owned();
        }
        return compact_or_colon(offset, false);
    }
    if existing.contains(':') {
        return compact_or_colon(offset, true);
    }
    compact_or_colon(offset, false)
}

fn compact_or_colon(offset: &str, colon: bool) -> String {
    if offset.is_empty() {
        return String::new();
    }
    if offset == "Z" {
        return if colon {
            "+00:00".to_owned()
        } else {
            "+0000".to_owned()
        };
    }
    let Some(sign) = offset.chars().next() else {
        return offset.to_owned();
    };
    let digits: String = offset
        .chars()
        .skip(1)
        .filter(char::is_ascii_digit)
        .collect();
    if digits.len() < 4 {
        return offset.to_owned();
    }
    let hours = &digits[..2];
    let minutes = &digits[2..4];
    if colon {
        format!("{sign}{hours}:{minutes}")
    } else {
        format!("{sign}{hours}{minutes}")
    }
}

fn emit_block_scalar(text: &str, newline: &str) -> String {
    let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
    let trailing = trailing_newline_count(&normalized);
    let body = normalized.trim_end_matches('\n');
    if body.is_empty() {
        // Clip of an empty body is `""`. Keep chomping needs an indented
        // empty line so a following `---` fence does not eat the newline.
        let mut out = String::from("|2+");
        for _ in 0..trailing {
            out.push_str(newline);
            out.push_str("  ");
        }
        out.push_str(newline);
        return out;
    }
    let needs_indent_indicator = body
        .split('\n')
        .any(|line| line.starts_with(' ') || line.starts_with('\t'));
    let chomp = match trailing {
        0 => "-",
        1 => "",
        _ => "+",
    };
    let mut out = String::from("|");
    if needs_indent_indicator {
        out.push('2');
    }
    out.push_str(chomp);
    for line in body.split('\n') {
        out.push_str(newline);
        if !line.is_empty() {
            out.push_str("  ");
            out.push_str(line);
        }
    }
    // Terminate the last content line. Clip keeps that one newline; keep
    // chomping adds `trailing - 1` extra blank lines after it.
    for _ in 0..trailing.max(1) {
        out.push_str(newline);
    }
    out
}

fn trailing_newline_count(text: &str) -> usize {
    text.bytes().rev().take_while(|byte| *byte == b'\n').count()
}

fn emit_sequence(
    items: &[YamlValue],
    kind: PropertyKind,
    retain_flow_list: bool,
    newline: &str,
) -> String {
    if items.is_empty() {
        return "[]".to_owned();
    }
    if retain_flow_list && can_keep_flow_list(items) {
        let inner = items
            .iter()
            .map(|item| emit_flow_item(item, kind))
            .collect::<Vec<_>>()
            .join(", ");
        if inner.len() <= SHORT_FLOW_LIST_MAX {
            return format!("[{inner}]");
        }
    }
    let mut out = String::new();
    for item in items {
        out.push_str(newline);
        out.push_str("  - ");
        out.push_str(&emit_block_item(item, kind, newline));
    }
    out
}

fn can_keep_flow_list(items: &[YamlValue]) -> bool {
    items.iter().all(|item| match item {
        YamlValue::Null | YamlValue::Bool(_) | YamlValue::Number(_) => true,
        YamlValue::String(text) => !text.contains('\n'),
        YamlValue::Sequence(_)
        | YamlValue::Mapping(_)
        | YamlValue::Tagged(_) => false,
    })
}

fn emit_flow_item(item: &YamlValue, kind: PropertyKind) -> String {
    match item {
        YamlValue::String(text) if kind == PropertyKind::Tags => {
            emit_string_scalar(text.strip_prefix('#').unwrap_or(text), false)
        }
        YamlValue::String(text) => emit_string_scalar(text, false),
        YamlValue::Null => "null".to_owned(),
        YamlValue::Bool(flag) => flag.to_string(),
        YamlValue::Number(number) => number.to_string(),
        other => emit_value(other, PropertyKind::List, None, false, "\n"),
    }
}

fn emit_block_item(
    item: &YamlValue,
    kind: PropertyKind,
    newline: &str,
) -> String {
    match item {
        YamlValue::Mapping(mapping) => {
            let mut nested = emit_mapping(mapping, None, newline);
            if nested.starts_with(newline) {
                nested = nested[newline.len()..].to_owned();
            }
            nested.replace(newline, &format!("{newline}    "))
        }
        YamlValue::Sequence(items) => {
            let mut nested =
                emit_sequence(items, PropertyKind::List, false, newline);
            if nested.starts_with(newline) {
                nested = nested[newline.len()..].to_owned();
            }
            nested.replace(newline, &format!("{newline}    "))
        }
        YamlValue::String(text) if kind == PropertyKind::Tags => {
            emit_string_scalar(text.strip_prefix('#').unwrap_or(text), false)
        }
        other => emit_value(other, kind, None, false, newline),
    }
}

fn emit_mapping(
    mapping: &Mapping,
    style: Option<&ValueStyle>,
    newline: &str,
) -> String {
    if mapping.is_empty() {
        return "{}".to_owned();
    }
    if style.is_some_and(|style| style.flow) && can_keep_flow_map(mapping) {
        let inner = mapping
            .iter()
            .map(|(key, value)| {
                let key = key.as_str().unwrap_or_default();
                format!(
                    "{}: {}",
                    emit_key(key),
                    emit_value(
                        value,
                        infer_yaml_kind(key, value),
                        None,
                        true,
                        newline
                    )
                )
            })
            .collect::<Vec<_>>()
            .join(", ");
        if inner.len() + 2 <= SHORT_FLOW_LIST_MAX {
            return format!("{{{inner}}}");
        }
    }
    let mut out = String::new();
    for (key, value) in mapping {
        let name = match key {
            YamlValue::String(text) => emit_key(text),
            other => crate::native::yaml_spans::render_scalar(other),
        };
        let rendered = emit_value(
            value,
            infer_yaml_kind(key.as_str().unwrap_or(""), value),
            None,
            false,
            newline,
        );
        out.push_str(newline);
        out.push_str("  ");
        out.push_str(&name);
        if rendered.starts_with(newline) || rendered.starts_with('|') {
            if rendered.starts_with(newline) {
                out.push(':');
                out.push_str(&indent_emitted(&rendered, "  ", newline));
            } else {
                out.push_str(": ");
                out.push_str(&indent_emitted(&rendered, "  ", newline));
            }
        } else {
            out.push_str(": ");
            out.push_str(&rendered);
        }
    }
    out
}

fn can_keep_flow_map(mapping: &Mapping) -> bool {
    mapping.iter().all(|(key, value)| {
        key.as_str().is_some()
            && match value {
                YamlValue::Null | YamlValue::Bool(_) | YamlValue::Number(_) => {
                    true
                }
                YamlValue::String(text) => !text.contains('\n'),
                YamlValue::Mapping(nested) => nested.is_empty(),
                YamlValue::Sequence(items) => items.is_empty(),
                YamlValue::Tagged(_) => false,
            }
    })
}

fn infer_yaml_kind(key: &str, value: &YamlValue) -> PropertyKind {
    match value {
        YamlValue::Null => PropertyKind::Null,
        YamlValue::Bool(_) => PropertyKind::Checkbox,
        YamlValue::Number(_) => PropertyKind::Number,
        YamlValue::String(text) if is_date(text) => PropertyKind::Date,
        YamlValue::String(text) if is_datetime(text) => PropertyKind::Datetime,
        YamlValue::String(_) => PropertyKind::Text,
        YamlValue::Sequence(_) if key == "tags" => PropertyKind::Tags,
        YamlValue::Sequence(_) if key == "aliases" => PropertyKind::Aliases,
        YamlValue::Sequence(_) => PropertyKind::List,
        YamlValue::Mapping(_) => PropertyKind::Map,
        YamlValue::Tagged(_) => PropertyKind::Opaque,
    }
}

fn indent_emitted(rendered: &str, extra: &str, newline: &str) -> String {
    rendered.replace(newline, &format!("{newline}{extra}"))
}

/// Parse a `prop.set_yaml` fragment as a single YAML value.
pub(crate) fn parse_yaml_fragment(
    yaml: &str,
    key: &str,
    op_index: usize,
) -> Result<YamlValue, PropsError> {
    if yaml.lines().any(|line| {
        let trimmed = line.trim();
        trimmed == "---" || trimmed == "..."
    }) {
        return Err(bad_value(
            key,
            op_index,
            "yaml fragments cannot contain document markers or frontmatter fences",
        ));
    }
    let value: YamlValue = serde_yaml::from_str(yaml).map_err(|error| {
        bad_value(
            key,
            op_index,
            &format!("yaml fragment is not valid YAML: {error}"),
        )
    })?;
    refuse_incoming_opaque(yaml, &value, key, op_index)?;
    Ok(value)
}

fn refuse_incoming_opaque(
    yaml: &str,
    value: &YamlValue,
    key: &str,
    op_index: usize,
) -> Result<(), PropsError> {
    if matches!(value, YamlValue::Tagged(_)) || yaml_contains_merge(value) {
        return Err(opaque_fragment(key, op_index));
    }
    let trimmed = yaml.trim_end_matches(['\n', '\r']);
    let wrapped = format!("---\n{key}: {trimmed}\n---\n");
    let parsed = super::document::PropsDocument::parse(&wrapped);
    if parsed.entries.iter().any(|entry| entry.key != key) {
        return Err(bad_value(
            key,
            op_index,
            "yaml fragment would inject sibling keys",
        ));
    }
    let Some(entry) = parsed.entries.iter().find(|entry| entry.key == key)
    else {
        return Ok(());
    };
    let feature_source = match value {
        YamlValue::String(_)
        | YamlValue::Null
        | YamlValue::Bool(_)
        | YamlValue::Number(_) => {
            let header = wrapped
                .get(entry.colon_range.end..)
                .unwrap_or("")
                .split(['\n', '\r'])
                .next()
                .unwrap_or("");
            header.to_owned()
        }
        YamlValue::Mapping(_)
        | YamlValue::Sequence(_)
        | YamlValue::Tagged(_) => wrapped.clone(),
    };
    if yaml_spans::has_yaml_feature(&yaml_spans::split_lines(&feature_source)) {
        return Err(opaque_fragment(key, op_index));
    }
    Ok(())
}

fn opaque_fragment(key: &str, op_index: usize) -> PropsError {
    bad_value(
        key,
        op_index,
        "yaml fragments cannot contain anchors, aliases, tags, or merge keys",
    )
}

/// True when two YAML values are semantically equal for a no-op set.
pub(crate) fn yaml_eq(left: &YamlValue, right: &YamlValue) -> bool {
    left == right
}

/// True when an existing one-line flow list should be retained.
pub(crate) fn existing_is_short_flow_list(
    source: &str,
    value_range: &std::ops::Range<usize>,
    style: &ValueStyle,
) -> bool {
    if !style.flow {
        return false;
    }
    let token = source.get(value_range.clone()).unwrap_or("").trim();
    token.starts_with('[')
        && crate::native::yaml_spans::flow_is_closed(token)
        && !token.contains('\n')
        && token.len() <= SHORT_FLOW_LIST_MAX + 2
}

#[cfg(test)]
mod tests {
    use super::{
        emit_key, emit_string_scalar, emit_value, typed_from_json,
        SHORT_FLOW_LIST_MAX,
    };
    use crate::native::props::document::PropertyKind;
    use serde_json::json;
    use serde_yaml::Value as YamlValue;

    #[test]
    fn quoting_covers_wikilinks_typed_text_and_punctuation() {
        assert_eq!(emit_string_scalar("[[Home]]", false), "\"[[Home]]\"");
        assert_eq!(emit_string_scalar("true", false), "\"true\"");
        assert_eq!(emit_string_scalar("2026-11-22", false), "\"2026-11-22\"");
        assert_eq!(emit_string_scalar("a: b", false), "\"a: b\"");
        assert_eq!(emit_string_scalar("a # b", false), "\"a # b\"");
        assert_eq!(emit_string_scalar(" padded ", false), "\" padded \"");
        assert_eq!(emit_string_scalar("plain", false), "plain");
        assert_eq!(emit_key("10"), "\"10\"");
        assert_eq!(emit_key("café"), "café");
    }

    #[test]
    fn new_lists_are_block_and_short_flow_can_stay() {
        let list = YamlValue::Sequence(vec![
            YamlValue::String("a".into()),
            YamlValue::String("b".into()),
        ]);
        let block = emit_value(&list, PropertyKind::List, None, false, "\n");
        assert_eq!(block, "\n  - a\n  - b");
        let flow = emit_value(&list, PropertyKind::List, None, true, "\n");
        assert_eq!(flow, "[a, b]");
        assert!(flow.len() <= SHORT_FLOW_LIST_MAX + 2);
    }

    #[test]
    fn tags_strip_hash_prefixes() {
        let value = typed_from_json(
            "tags",
            &json!(["#work", "home"]),
            Some(PropertyKind::Tags),
            0,
        )
        .unwrap();
        let rendered =
            emit_value(&value.yaml, PropertyKind::Tags, None, true, "\n");
        assert_eq!(rendered, "[work, home]");
    }

    fn round_trip_block(text: &str) -> String {
        let emitted = emit_value(
            &YamlValue::String(text.into()),
            PropertyKind::Text,
            None,
            false,
            "\n",
        );
        let wrapped = format!(
            "---\nk: {emitted}{}---\n",
            if emitted.ends_with('\n') { "" } else { "\n" }
        );
        let document =
            crate::native::props::document::PropsDocument::parse(&wrapped);
        document
            .entries
            .iter()
            .find(|entry| entry.key == "k")
            .and_then(|entry| entry.raw_value.as_ref())
            .and_then(YamlValue::as_str)
            .unwrap_or_else(|| {
                panic!(
                    "expected string in\n{wrapped}\nentries={:?}",
                    document
                        .entries
                        .iter()
                        .map(|e| (&e.key, &e.raw_value))
                        .collect::<Vec<_>>()
                )
            })
            .to_owned()
    }

    #[test]
    fn multiline_text_uses_literal_block_chomping() {
        let none = emit_value(
            &YamlValue::String("hello".into()),
            PropertyKind::Text,
            None,
            false,
            "\n",
        );
        assert_eq!(none, "hello");
        let one = emit_value(
            &YamlValue::String("hello\n".into()),
            PropertyKind::Text,
            None,
            false,
            "\n",
        );
        assert_eq!(one, "|\n  hello\n");
        let two = emit_value(
            &YamlValue::String("hello\n\n".into()),
            PropertyKind::Text,
            None,
            false,
            "\n",
        );
        assert!(two.starts_with("|+"));
        let inner = emit_value(
            &YamlValue::String("hello\nworld".into()),
            PropertyKind::Text,
            None,
            false,
            "\n",
        );
        assert_eq!(inner, "|-\n  hello\n  world\n");
        assert_eq!(round_trip_block("hello\n"), "hello\n");
        assert_eq!(round_trip_block("hello\n\n"), "hello\n\n");
        assert_eq!(round_trip_block("hello\nworld"), "hello\nworld");
        assert_eq!(round_trip_block("\n"), "\n");
        assert_eq!(round_trip_block("\n\n"), "\n\n");
        assert_eq!(round_trip_block("  hello\n"), "  hello\n");
        let leading = emit_value(
            &YamlValue::String("  hello\n".into()),
            PropertyKind::Text,
            None,
            false,
            "\n",
        );
        assert!(
            leading.starts_with("|2"),
            "leading spaces need an indent indicator: {leading}"
        );
    }

    #[test]
    fn set_yaml_refuses_incoming_anchors_aliases_and_merges() {
        use super::parse_yaml_fragment;
        for yaml in ["*summary", "&id value", "!!str tagged", "<<: {a: 1}"] {
            let error = parse_yaml_fragment(yaml, "notes", 0).expect_err(yaml);
            assert_eq!(error.code, "prop_bad_value", "{yaml}");
        }
        assert!(parse_yaml_fragment("\"*summary\"", "notes", 0).is_ok());
        assert!(parse_yaml_fragment("|\n  *not_an_alias", "notes", 0).is_ok());
    }

    #[test]
    fn empty_containers_and_null_stay_distinct() {
        assert_eq!(
            emit_value(&YamlValue::Null, PropertyKind::Null, None, false, "\n"),
            "null"
        );
        assert_eq!(
            emit_value(
                &YamlValue::String(String::new()),
                PropertyKind::Text,
                None,
                false,
                "\n"
            ),
            "\"\""
        );
        assert_eq!(
            emit_value(
                &YamlValue::Sequence(Vec::new()),
                PropertyKind::List,
                None,
                false,
                "\n"
            ),
            "[]"
        );
        assert_eq!(
            emit_value(
                &YamlValue::Mapping(Default::default()),
                PropertyKind::Map,
                None,
                false,
                "\n"
            ),
            "{}"
        );
    }
}
