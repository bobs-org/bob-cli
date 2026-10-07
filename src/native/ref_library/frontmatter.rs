//! Frontmatter parsing for the reference-library index.
//!
//! The block delimited by [`split_frontmatter`](super::super::highlights_ref::split_frontmatter)
//! is parsed with `serde_yaml` (what Dataview uses). Unquoted wikilinks such
//! as `parent: [[x]]` — which YAML reads as nested sequences — are normalized
//! back to `[[x]]`. On invalid YAML the module falls back to a line-by-line
//! parse and reports an `invalid_yaml` diagnostic instead of dropping the row.
use std::collections::BTreeMap;

/// A frontmatter scalar or a flattened list of scalars.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum FrontValue {
    Str(String),
    List(Vec<String>),
}

/// Parsed frontmatter plus any YAML failure that triggered the fallback.
#[derive(Debug, Clone, Default)]
pub(crate) struct ParsedFrontmatter {
    values: BTreeMap<String, FrontValue>,
    pub invalid_yaml: Option<String>,
}

impl ParsedFrontmatter {
    /// Parse raw frontmatter lines, falling back to line parsing on error.
    pub(crate) fn parse(raw_lines: &[String]) -> Self {
        let joined = raw_lines.join("\n");
        match serde_yaml::from_str::<serde_yaml::Value>(&joined) {
            Ok(value) => Self::from_yaml(value, None),
            Err(error) => Self::from_yaml(
                serde_yaml::Value::Null,
                Some(short_yaml_error(&error)),
            )
            .with_fallback(raw_lines),
        }
    }

    fn from_yaml(
        value: serde_yaml::Value,
        invalid_yaml: Option<String>,
    ) -> Self {
        let mut values = BTreeMap::new();
        if let serde_yaml::Value::Mapping(mapping) = value {
            for (key, item) in mapping {
                if let Some(name) = yaml_key(&key)
                    && let Some(front) = yaml_item_to_front(&item)
                {
                    values.insert(name, front);
                }
            }
        }
        Self {
            values,
            invalid_yaml,
        }
    }

    /// Line-by-line fallback that never fails. It only fills keys absent
    /// from the map, which is empty when invalid YAML fails to parse at
    /// all; a half-parsed mapping still contributes its own keys.
    fn with_fallback(mut self, raw_lines: &[String]) -> Self {
        for raw in raw_lines {
            let Some((key, value)) = raw.split_once(':') else {
                continue;
            };
            let name = normalize_name(key);
            if name.is_empty() || self.values.contains_key(&name) {
                continue;
            }
            self.values.insert(name, parse_line_value(value));
        }
        self
    }

    /// The first string for `key`: the scalar, or the first list entry.
    pub(crate) fn get_str(&self, key: &str) -> Option<&str> {
        match self.values.get(key) {
            Some(FrontValue::Str(value)) => Some(value.as_str()),
            Some(FrontValue::List(values)) => {
                values.first().map(String::as_str)
            }
            None => None,
        }
    }

    /// Every string for `key`: the scalar alone, or the whole list.
    pub(crate) fn get_all(&self, key: &str) -> Vec<String> {
        match self.values.get(key) {
            Some(FrontValue::Str(value)) => vec![value.clone()],
            Some(FrontValue::List(values)) => values.clone(),
            None => Vec::new(),
        }
    }

    /// The raw value for `key`, for callers that keep the shape.
    pub(crate) fn raw(&self, key: &str) -> Option<&FrontValue> {
        self.values.get(key)
    }
}

fn normalize_name(key: &str) -> String {
    key.trim()
        .chars()
        .map(|c| match c {
            '-' | ' ' => '_',
            other => other.to_ascii_lowercase(),
        })
        .collect()
}

fn yaml_key(key: &serde_yaml::Value) -> Option<String> {
    match key {
        serde_yaml::Value::String(name) => Some(normalize_name(name)),
        serde_yaml::Value::Number(name) => Some(name.to_string()),
        serde_yaml::Value::Bool(name) => Some(name.to_string()),
        _ => None,
    }
}

/// Convert one YAML mapping value; mappings and nulls carry no row field.
fn yaml_item_to_front(value: &serde_yaml::Value) -> Option<FrontValue> {
    match value {
        serde_yaml::Value::Null => None,
        serde_yaml::Value::Bool(flag) => {
            Some(FrontValue::Str(flag.to_string()))
        }
        serde_yaml::Value::Number(number) => {
            Some(FrontValue::Str(number.to_string()))
        }
        serde_yaml::Value::String(text) => Some(FrontValue::Str(text.clone())),
        serde_yaml::Value::Sequence(items) => {
            let mut flat = Vec::new();
            for item in items {
                flat.extend(yaml_sequence_element(item));
            }
            Some(FrontValue::List(flat))
        }
        serde_yaml::Value::Mapping(_) => None,
        serde_yaml::Value::Tagged(tagged) => yaml_item_to_front(&tagged.value),
    }
}

/// One sequence element: scalars pass through, and a nested single-element
/// sequence of scalars is the YAML reading of unquoted `[[wikilinks]]`.
fn yaml_sequence_element(value: &serde_yaml::Value) -> Vec<String> {
    match value {
        serde_yaml::Value::Null => Vec::new(),
        serde_yaml::Value::Bool(flag) => vec![flag.to_string()],
        serde_yaml::Value::Number(number) => vec![number.to_string()],
        serde_yaml::Value::String(text) => vec![text.clone()],
        serde_yaml::Value::Sequence(inner) => inner
            .iter()
            .filter_map(yaml_scalar_string)
            .map(|name| format!("[[{name}]]"))
            .collect(),
        serde_yaml::Value::Mapping(_) => Vec::new(),
        serde_yaml::Value::Tagged(tagged) => {
            yaml_sequence_element(&tagged.value)
        }
    }
}

fn yaml_scalar_string(value: &serde_yaml::Value) -> Option<String> {
    match value {
        serde_yaml::Value::String(text) => Some(text.clone()),
        serde_yaml::Value::Number(number) => Some(number.to_string()),
        serde_yaml::Value::Bool(flag) => Some(flag.to_string()),
        _ => None,
    }
}

/// One fallback line value: an inline `[...]` list or a bare scalar with
/// matching surrounding quotes stripped. A bare wikilink stays intact
/// (like the Highlights parser, which checks wikilinks before inline
/// lists); otherwise `parent: [[x]]` would read as the list `["[x]"]`.
fn parse_line_value(raw: &str) -> FrontValue {
    use crate::native::highlights_ref::is_wikilink;

    let trimmed = raw.trim();
    if is_wikilink(trimmed) {
        return FrontValue::Str(trimmed.to_string());
    }
    if let Some(inner) = trimmed
        .strip_prefix('[')
        .and_then(|rest| rest.strip_suffix(']'))
    {
        let items = inner
            .split(',')
            .map(|item| strip_surrounding_quotes(item.trim()))
            .filter(|item| !item.is_empty())
            .map(str::to_string)
            .collect::<Vec<_>>();
        return FrontValue::List(items);
    }
    let unquoted = strip_surrounding_quotes(trimmed);
    FrontValue::Str(unquoted.to_string())
}

fn strip_surrounding_quotes(value: &str) -> &str {
    if value.len() >= 2
        && ((value.starts_with('"') && value.ends_with('"'))
            || (value.starts_with('\'') && value.ends_with('\'')))
    {
        &value[1..value.len() - 1]
    } else {
        value
    }
}

fn short_yaml_error(error: &serde_yaml::Error) -> String {
    let message = error.to_string();
    message.lines().next().unwrap_or("invalid YAML").to_string()
}
