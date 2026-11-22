//! Semantic frontmatter fingerprints for `bob props`.
//!
//! Ordinary string-keyed YAML uses the existing records canonicalizer. If a
//! YAML mapping contains non-string keys or a tagged node, the whole value is
//! represented by a deterministic, type-tagged JSON tree. That fallback
//! retains every YAML key/value and prevents a lossy conversion from making
//! two different documents share a fingerprint.

use serde_json::{json, Number, Value as JsonValue};
use serde_yaml::Value as YamlValue;
use sha2::{Digest, Sha256};

pub(crate) fn fingerprint(mapping: &YamlValue) -> String {
    let canonical = if json_conversion_is_lossless(mapping) {
        crate::native::records::fingerprint::yaml_to_canonical_json(mapping)
    } else {
        typed_yaml_json(mapping)
    };
    let bytes = serde_json::to_vec(&canonical)
        .expect("canonical props frontmatter is JSON serializable");
    format!("sha256:{}", hex::encode(Sha256::digest(bytes)))
}

pub(crate) fn empty_mapping_fingerprint() -> String {
    fingerprint(&YamlValue::Mapping(serde_yaml::Mapping::new()))
}

fn json_conversion_is_lossless(value: &YamlValue) -> bool {
    match value {
        YamlValue::Mapping(mapping) => mapping.iter().all(|(key, value)| {
            key.as_str().is_some() && json_conversion_is_lossless(value)
        }),
        YamlValue::Sequence(values) => {
            values.iter().all(json_conversion_is_lossless)
        }
        YamlValue::Tagged(_) => false,
        YamlValue::Number(number) => {
            number.as_i64().is_some()
                || number.as_u64().is_some()
                || number.as_f64().is_some_and(f64::is_finite)
        }
        YamlValue::Null | YamlValue::Bool(_) | YamlValue::String(_) => true,
    }
}

fn typed_yaml_json(value: &YamlValue) -> JsonValue {
    match value {
        YamlValue::Null => json!({"$yaml": "null"}),
        YamlValue::Bool(value) => json!({"$yaml": "bool", "value": value}),
        YamlValue::Number(value) => json!({
            "$yaml": "number",
            "value": value.as_i64().map(JsonValue::from)
                .or_else(|| value.as_u64().map(JsonValue::from))
                .or_else(|| value.as_f64().and_then(Number::from_f64).map(JsonValue::Number))
                .unwrap_or_else(|| JsonValue::String(value.to_string())),
        }),
        YamlValue::String(value) => {
            json!({"$yaml": "string", "value": value})
        }
        YamlValue::Sequence(values) => json!({
            "$yaml": "sequence",
            "items": values.iter().map(typed_yaml_json).collect::<Vec<_>>(),
        }),
        YamlValue::Mapping(mapping) => {
            let mut entries: Vec<_> = mapping
                .iter()
                .map(|(key, value)| {
                    let key = typed_yaml_json(key);
                    let value = typed_yaml_json(value);
                    let sort_key = serde_json::to_vec(&key)
                        .expect("typed YAML key is JSON serializable");
                    (sort_key, key, value)
                })
                .collect();
            entries.sort_by(|left, right| left.0.cmp(&right.0));
            let entries = entries
                .into_iter()
                .map(|(_, key, value)| json!([key, value]))
                .collect::<Vec<_>>();
            json!({"$yaml": "mapping", "entries": entries})
        }
        YamlValue::Tagged(tagged) => json!({
            "$yaml": "tagged",
            "tag": tagged.tag.to_string(),
            "value": typed_yaml_json(&tagged.value),
        }),
    }
}

#[cfg(test)]
mod tests {
    use serde_yaml::Value as YamlValue;

    use super::fingerprint;

    fn parsed(yaml: &str) -> YamlValue {
        serde_yaml::from_str(yaml).expect("fixture YAML")
    }

    #[test]
    fn ordinary_mapping_fingerprint_ignores_order_and_formatting() {
        let left = parsed("a: 1\nb: [x, false]\n");
        let right = parsed("b: [x,false]\na: 1\n");
        assert_eq!(fingerprint(&left), fingerprint(&right));
    }

    #[test]
    fn string_property_names_that_look_numeric_are_preserved() {
        let left = parsed("'10': ten\n'2': two\n");
        let right = parsed("'2': two\n'10': ten\n");
        assert_eq!(fingerprint(&left), fingerprint(&right));
        assert_ne!(fingerprint(&left), fingerprint(&parsed("'2': two\n")));
    }

    #[test]
    fn unsupported_mapping_keys_use_a_stable_lossless_fallback() {
        let left = parsed("outer: {1: number, '1': text}\n");
        let reordered = parsed("outer: {'1': text, 1: number}\n");
        let dropped = parsed("outer: {'1': text}\n");
        assert_eq!(fingerprint(&left), fingerprint(&reordered));
        assert_ne!(fingerprint(&left), fingerprint(&dropped));
    }
}
