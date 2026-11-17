//! Semantic fingerprint for one record note.
//!
//! The fingerprint hashes the parsed frontmatter mapping (canonical JSON
//! with sorted keys and typed scalars kept distinct) together with the
//! album `schema_fingerprint`. Body bytes, YAML formatting, and key order
//! do not affect it. `show` and `update` share this module.

use serde_json::{json, Value as JsonValue};
use serde_yaml::{Mapping, Value as YamlValue};
use sha2::{Digest, Sha256};

/// `sha256:` plus 64 lowercase hex digits over the record's canonical
/// frontmatter JSON and the album schema fingerprint.
pub(crate) fn record_fingerprint(
    frontmatter: &Mapping,
    schema_fingerprint: &str,
) -> String {
    let canonical =
        yaml_to_canonical_json(&YamlValue::Mapping(frontmatter.clone()));
    fingerprint_canonical(&canonical, schema_fingerprint)
}

/// Fingerprint an absent or unparseable frontmatter block as an empty
/// mapping so callers never invent a record.
pub(crate) fn empty_record_fingerprint(schema_fingerprint: &str) -> String {
    fingerprint_canonical(&json!({}), schema_fingerprint)
}

fn fingerprint_canonical(
    canonical_frontmatter: &JsonValue,
    schema_fingerprint: &str,
) -> String {
    let semantic = json!([canonical_frontmatter, schema_fingerprint]);
    let bytes = serde_json::to_vec(&semantic)
        .expect("record fingerprint semantic model is serializable");
    format!("sha256:{}", hex::encode(Sha256::digest(bytes)))
}

/// Strip a UTF-8 BOM so YAML and frontmatter scanners see the same
/// opening `---` that a BOM-less note would.
pub(crate) fn strip_bom(contents: &str) -> &str {
    contents.strip_prefix('\u{feff}').unwrap_or(contents)
}

/// Parse the YAML mapping between the opening and closing `---` lines.
/// Returns `None` when the note has no closed frontmatter or the YAML is
/// not a mapping.
pub(crate) fn parse_frontmatter_mapping(contents: &str) -> Option<Mapping> {
    let contents = strip_bom(contents);
    let (front, _) =
        crate::native::highlights_ref::split_frontmatter(contents)?;
    let joined = front.join("\n");
    let value = serde_yaml::from_str::<YamlValue>(&joined).ok()?;
    value.as_mapping().cloned()
}

/// Convert a YAML value to JSON, sorting mapping keys at every level so
/// key order is not part of the fingerprint. Typed scalars stay distinct.
pub(crate) fn yaml_to_canonical_json(value: &YamlValue) -> JsonValue {
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
            JsonValue::Array(items.iter().map(yaml_to_canonical_json).collect())
        }
        YamlValue::Mapping(map) => {
            let mut keys: Vec<String> = map
                .keys()
                .filter_map(|key| key.as_str().map(str::to_owned))
                .collect();
            keys.sort();
            let mut out = serde_json::Map::new();
            for key in keys {
                let yaml_key = YamlValue::String(key.clone());
                if let Some(item) = map.get(&yaml_key) {
                    out.insert(key, yaml_to_canonical_json(item));
                }
            }
            JsonValue::Object(out)
        }
        YamlValue::Tagged(tagged) => yaml_to_canonical_json(&tagged.value),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        empty_record_fingerprint, parse_frontmatter_mapping,
        record_fingerprint, strip_bom, yaml_to_canonical_json,
    };
    use serde_yaml::Value as YamlValue;

    const SCHEMA: &str = "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const OTHER_SCHEMA: &str = "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

    fn mapping(yaml: &str) -> serde_yaml::Mapping {
        serde_yaml::from_str::<YamlValue>(yaml)
            .expect("test YAML")
            .as_mapping()
            .cloned()
            .expect("mapping")
    }

    fn note(front: &str, body: &str) -> String {
        format!("---\n{front}---\n{body}")
    }

    #[test]
    fn fingerprint_ignores_key_order_and_yaml_formatting() {
        let block = mapping(
            "parent: \"[[eat]]\"\ntype: \"[[record]]\"\nstatus: liked\nrating: 4\n",
        );
        let reordered = mapping(
            "rating: 4\nstatus: liked\ntype: \"[[record]]\"\nparent: \"[[eat]]\"\n",
        );
        let flow = mapping(
            "{parent: \"[[eat]]\", type: \"[[record]]\", status: liked, rating: 4}",
        );
        let expected = record_fingerprint(&block, SCHEMA);
        assert_eq!(expected, record_fingerprint(&reordered, SCHEMA));
        assert_eq!(expected, record_fingerprint(&flow, SCHEMA));
        assert!(expected.starts_with("sha256:"));
        assert_eq!(expected.len(), "sha256:".len() + 64);
        let hex = &expected["sha256:".len()..];
        assert!(hex.chars().all(|ch| matches!(ch, '0'..='9' | 'a'..='f')));
    }

    #[test]
    fn fingerprint_keeps_typed_scalars_distinct() {
        let number = mapping("year: 2002\n");
        let string = mapping("year: \"2002\"\n");
        assert_ne!(
            record_fingerprint(&number, SCHEMA),
            record_fingerprint(&string, SCHEMA)
        );
    }

    #[test]
    fn fingerprint_changes_with_value_or_schema() {
        let liked = mapping("status: liked\n");
        let to_try = mapping("status: to_try\n");
        assert_ne!(
            record_fingerprint(&liked, SCHEMA),
            record_fingerprint(&to_try, SCHEMA)
        );
        assert_ne!(
            record_fingerprint(&liked, SCHEMA),
            record_fingerprint(&liked, OTHER_SCHEMA)
        );
    }

    #[test]
    fn parse_frontmatter_mapping_ignores_body_and_bom() {
        let body_edit = note("status: liked\n", "# Title\n\n## Notes\n- one\n");
        let other_body =
            note("status: liked\n", "# Title\n\n## Notes\n- two\n");
        let bom = format!("\u{feff}{}", body_edit);
        let left = parse_frontmatter_mapping(&body_edit).expect("left");
        let right = parse_frontmatter_mapping(&other_body).expect("right");
        let bom_map = parse_frontmatter_mapping(&bom).expect("bom");
        assert_eq!(
            record_fingerprint(&left, SCHEMA),
            record_fingerprint(&right, SCHEMA)
        );
        assert_eq!(
            record_fingerprint(&left, SCHEMA),
            record_fingerprint(&bom_map, SCHEMA)
        );
        assert_eq!(strip_bom(&bom), body_edit);
    }

    #[test]
    fn empty_fingerprint_matches_empty_mapping() {
        assert_eq!(
            empty_record_fingerprint(SCHEMA),
            record_fingerprint(&serde_yaml::Mapping::new(), SCHEMA)
        );
    }

    #[test]
    fn canonical_json_sorts_nested_keys() {
        let value =
            serde_yaml::from_str::<YamlValue>("outer: {b: 1, a: [2, 1]}\n")
                .expect("yaml");
        assert_eq!(
            yaml_to_canonical_json(&value).to_string(),
            r#"{"outer":{"a":[2,1],"b":1}}"#
        );
    }
}
