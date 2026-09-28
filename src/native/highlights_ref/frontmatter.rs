//! Note frontmatter parsing and projection snapshots.
use super::*;

pub(super) fn read_note(path: &Path) -> Result<ParsedNote> {
    match fs::read_to_string(path) {
        Ok(contents) => Ok(parse_note(&contents)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            Ok(ParsedNote::empty())
        }
        Err(error) => Err(CommandError::new(format!(
            "read note {}: {error}",
            path.display()
        ))),
    }
}

pub(super) fn parse_note(contents: &str) -> ParsedNote {
    let Some((frontmatter, body)) = split_frontmatter(contents) else {
        return ParsedNote {
            frontmatter: Vec::new(),
            body: contents.to_string(),
            original: Some(contents.to_string()),
        };
    };
    ParsedNote {
        frontmatter: frontmatter
            .into_iter()
            .map(|raw| parse_frontmatter_entry(&raw))
            .collect(),
        body,
        original: Some(contents.to_string()),
    }
}

pub(super) fn split_frontmatter(
    contents: &str,
) -> Option<(Vec<String>, String)> {
    let marker_len = if contents.starts_with("---\r\n") {
        5
    } else if contents.starts_with("---\n") {
        4
    } else {
        return None;
    };

    let mut offset = marker_len;
    let mut lines = Vec::new();
    while offset < contents.len() {
        let remaining = &contents[offset..];
        let line_len = remaining
            .find('\n')
            .map(|index| index + 1)
            .unwrap_or(remaining.len());
        let line = &remaining[..line_len];
        let trimmed_line = trim_line_ending(line);
        offset += line_len;
        if trimmed_line == "---" {
            return Some((lines, contents[offset..].to_string()));
        }
        lines.push(trimmed_line.to_string());
    }
    None
}

pub(super) fn parse_frontmatter_entry(raw: &str) -> FrontmatterEntry {
    let Some((key, value)) = raw.split_once(':') else {
        return FrontmatterEntry {
            key: None,
            value: None,
            raw: raw.to_string(),
        };
    };
    let key = normalize_key(key);
    if key.is_empty() {
        return FrontmatterEntry {
            key: None,
            value: None,
            raw: raw.to_string(),
        };
    }
    FrontmatterEntry {
        key: Some(key),
        value: Some(parse_value(value)),
        raw: raw.to_string(),
    }
}

pub(super) fn trim_line_ending(line: &str) -> &str {
    line.strip_suffix("\r\n")
        .or_else(|| line.strip_suffix('\n'))
        .unwrap_or(line)
}

pub(super) fn value_as_string_set(
    value: &MarkerValue,
) -> Option<BTreeSet<String>> {
    match value {
        MarkerValue::List(values) => Some(
            values
                .iter()
                .filter_map(|value| value.as_string().map(normalize_key))
                .filter(|value| !value.is_empty())
                .collect(),
        ),
        MarkerValue::String(value) => {
            let normalized = normalize_key(value);
            (!normalized.is_empty()).then(|| BTreeSet::from([normalized]))
        }
        _ => None,
    }
}

pub(super) fn is_pipeline_field(key: &str) -> bool {
    PIPELINE_FIELDS.contains(&key)
}

pub(super) fn is_command_managed_field(key: &str) -> bool {
    COMMAND_MANAGED_FIELDS.contains(&key)
}

pub(super) fn is_managed_frontmatter_field(key: &str) -> bool {
    is_pipeline_field(key) || is_command_managed_field(key)
}

pub(super) fn is_standard_user_field(key: &str) -> bool {
    MARKER_REQUIRED_KEYS.contains(&key) || COMMON_USER_FIELDS.contains(&key)
}

pub(super) fn unknown_synced_fields(projection: &Projection) -> Vec<String> {
    projection
        .keys()
        .filter(|key| !is_standard_user_field(key))
        .cloned()
        .collect()
}

pub(super) fn projection_snapshot_json(projection: &Projection) -> String {
    let mut object = serde_json::Map::new();
    for (key, value) in projection {
        object.insert(key.clone(), marker_value_to_json(value));
    }
    serde_json::to_string(&serde_json::Value::Object(object))
        .expect("serializing marker projection snapshot cannot fail")
}

pub(super) fn projection_from_snapshot_json(
    contents: &str,
) -> Result<Projection> {
    let value = serde_json::from_str::<serde_json::Value>(contents).map_err(
        |error| {
            CommandError::new(format!(
                "parse {FIELD_MARKER_BASE} JSON projection: {error}"
            ))
        },
    )?;
    let serde_json::Value::Object(object) = value else {
        return Err(CommandError::new(format!(
            "{FIELD_MARKER_BASE} must be a JSON object"
        )));
    };

    let mut projection = Projection::new();
    for (key, value) in object {
        let key = normalize_key(&key);
        if key.is_empty() {
            return Err(CommandError::new(format!(
                "{FIELD_MARKER_BASE} contains an empty key"
            )));
        }
        projection.insert(key, marker_value_from_json(value)?);
    }
    canonicalize_parent(&mut projection, FIELD_MARKER_BASE)?;
    Ok(projection)
}

pub(super) fn marker_value_to_json(value: &MarkerValue) -> serde_json::Value {
    match value {
        MarkerValue::Null => serde_json::Value::Null,
        MarkerValue::Bool(value) => serde_json::Value::Bool(*value),
        MarkerValue::Number(value) => {
            serde_json::from_str::<serde_json::Value>(value)
                .unwrap_or_else(|_| serde_json::Value::String(value.clone()))
        }
        MarkerValue::String(value) => serde_json::Value::String(value.clone()),
        MarkerValue::List(values) => serde_json::Value::Array(
            values.iter().map(marker_value_to_json).collect(),
        ),
    }
}

pub(super) fn marker_value_from_json(
    value: serde_json::Value,
) -> Result<MarkerValue> {
    match value {
        serde_json::Value::Null => Ok(MarkerValue::Null),
        serde_json::Value::Bool(value) => Ok(MarkerValue::Bool(value)),
        serde_json::Value::Number(value) => {
            Ok(MarkerValue::Number(value.to_string()))
        }
        serde_json::Value::String(value) => Ok(MarkerValue::String(value)),
        serde_json::Value::Array(values) => values
            .into_iter()
            .map(marker_value_from_json)
            .collect::<Result<Vec<_>>>()
            .map(MarkerValue::List),
        serde_json::Value::Object(_) => Err(CommandError::new(format!(
            "{FIELD_MARKER_BASE} contains a nested object, which marker projections do not support"
        ))),
    }
}

pub(super) fn projection_hash(projection: &Projection) -> Result<String> {
    let canonical = serde_json::to_vec(projection).map_err(|error| {
        CommandError::new(format!("serialize marker projection: {error}"))
    })?;
    Ok(hex::encode(Sha256::digest(canonical)))
}
