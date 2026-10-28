use std::path::Path;

use serde_yaml::Value;

use super::{ConfigError, RawConfig};
use crate::native::{
    relation_labels::relation_label_key,
    task_relations::label::normalize_relation_label,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CancelLabel {
    pub(crate) key: String,
    pub(crate) name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RelationsConfig {
    pub(crate) linked_from_lines: bool,
    pub(crate) cancel_labels: Vec<CancelLabel>,
    pub(crate) warnings: Vec<String>,
}

impl Default for RelationsConfig {
    fn default() -> Self {
        Self {
            linked_from_lines: true,
            cancel_labels: Vec::new(),
            warnings: Vec::new(),
        }
    }
}

impl RelationsConfig {
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn is_cancel_label(&self, key: &str) -> bool {
        self.cancel_labels.iter().any(|label| label.key == key)
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn cancel_label_for_inverse(&self, key: &str) -> Option<&str> {
        self.cancel_labels.iter().find_map(|label| {
            crate::native::task_relations::label::inverse_phrase(&label.name)
                .filter(|inverse| relation_label_key(inverse) == key)
                .map(|_| label.name.as_str())
        })
    }
}

pub(crate) fn load_relations_config(
    path: &Path,
) -> Result<RelationsConfig, ConfigError> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(RelationsConfig::default());
        }
        Err(error) => {
            return Err(ConfigError::Read(format!(
                "read {}: {error}",
                path.display()
            )));
        }
    };
    parse_relations_config(&text, path)
}

pub(crate) fn parse_relations_config(
    text: &str,
    path: &Path,
) -> Result<RelationsConfig, ConfigError> {
    let config: RawConfig = serde_yaml::from_str(text).map_err(|error| {
        ConfigError::Invalid(format!("parse {}: {error}", path.display()))
    })?;
    let Some(relations) = config.relations else {
        return Ok(RelationsConfig::default());
    };
    if relations.is_null() {
        return Ok(RelationsConfig::default());
    }
    let path_display = path.display();
    let Some(mapping) = relations.as_mapping() else {
        return Err(ConfigError::Invalid(format!(
            "relations in {path_display} must be a mapping"
        )));
    };
    let key = Value::String("linked_from_lines".to_string());
    let cancel_key = Value::String("cancel_labels".to_string());
    let mut warnings = Vec::new();
    let linked_from_lines = match mapping.get(&key) {
        None | Some(Value::Null) => true,
        Some(Value::Bool(value)) => *value,
        Some(value) => {
            warnings.push(format!(
                "relations.linked_from_lines in {path_display} must be a boolean; got {}; using relations.linked_from_lines=true",
                yaml_type(value)
            ));
            true
        }
    };
    let cancel_labels = parse_cancel_labels(
        mapping.get(&cancel_key),
        &path_display.to_string(),
        &mut warnings,
    );
    Ok(RelationsConfig {
        linked_from_lines,
        cancel_labels,
        warnings,
    })
}

fn parse_cancel_labels(
    value: Option<&Value>,
    path: &str,
    warnings: &mut Vec<String>,
) -> Vec<CancelLabel> {
    let Some(value) = value else {
        return Vec::new();
    };
    if value.is_null() {
        return Vec::new();
    }
    let Some(entries) = value.as_sequence() else {
        warnings.push(format!(
            "invalid relations.cancel_labels in {path}: expected a list of relation labels; cancelling nothing"
        ));
        return Vec::new();
    };

    let mut labels = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    for (index, entry) in entries.iter().enumerate() {
        let Some(raw) = entry.as_str() else {
            warnings.push(format!(
                "invalid relations.cancel_labels entry {} in {path}: expected a string; skipping",
                index + 1
            ));
            continue;
        };
        let name = match normalize_relation_label(raw) {
            Ok(name) => name,
            Err(error) => {
                warnings.push(format!(
                    "invalid relations.cancel_labels entry {} in {path}: {error:?}; skipping",
                    index + 1
                ));
                continue;
            }
        };
        let key = relation_label_key(&name);
        if seen.insert(key.clone()) {
            labels.push(CancelLabel { key, name });
        }
    }
    labels
}

fn yaml_type(value: &Value) -> String {
    match value {
        Value::Null => "null".to_string(),
        Value::Bool(value) => value.to_string(),
        Value::Number(value) => value.to_string(),
        Value::String(value) => format!("{value:?}"),
        Value::Sequence(_) => "sequence".to_string(),
        Value::Mapping(_) => "mapping".to_string(),
        Value::Tagged(_) => "tagged value".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn linked_from_lines_defaults_true_and_parses_false() {
        let defaulted = parse_relations_config(
            "properties: []\n",
            Path::new("/config.yml"),
        )
        .expect("absent relations block");
        assert!(defaulted.linked_from_lines);

        let null_block =
            parse_relations_config("relations:\n", Path::new("/config.yml"))
                .expect("null relations block");
        assert!(null_block.linked_from_lines);

        let null_value = parse_relations_config(
            "relations:\n  linked_from_lines: null\n",
            Path::new("/config.yml"),
        )
        .expect("null key falls back to default");
        assert!(null_value.linked_from_lines);

        let enabled = parse_relations_config(
            "relations:\n  linked_from_lines: true\n",
            Path::new("/config.yml"),
        )
        .expect("explicit true");
        assert!(enabled.linked_from_lines);

        let disabled = parse_relations_config(
            "relations:\n  linked_from_lines: false\n",
            Path::new("/config.yml"),
        )
        .expect("explicit false");
        assert!(!disabled.linked_from_lines);
    }

    #[test]
    fn whole_relations_section_still_requires_a_mapping() {
        assert!(matches!(
            parse_relations_config(
                "relations: [true]\n",
                Path::new("/config.yml")
            ),
            Err(ConfigError::Invalid(_))
        ));
    }

    #[test]
    fn reports_invalid_linked_from_lines_path_and_type() {
        let parsed = parse_relations_config(
            "relations:\n  linked_from_lines: 1\n",
            Path::new("/tmp/bob-config.yml"),
        )
        .expect("invalid mirror flag uses conservative default");
        assert!(parsed.linked_from_lines);
        assert_eq!(
            parsed.warnings,
            ["relations.linked_from_lines in /tmp/bob-config.yml must be a boolean; got 1; using relations.linked_from_lines=true"]
        );
    }

    #[test]
    fn cancel_labels_are_ordered_normalized_and_deduplicated() {
        let parsed = parse_relations_config(
            "relations:\n  cancel_labels:\n    - ' Duplicate  Of:'\n    - DUPLICATE OF\n    - 'superseded by'\n",
            Path::new("/config.yml"),
        )
        .expect("valid policy");
        assert_eq!(
            parsed.cancel_labels,
            [
                CancelLabel {
                    key: "duplicate of".into(),
                    name: "Duplicate Of".into(),
                },
                CancelLabel {
                    key: "superseded by".into(),
                    name: "superseded by".into(),
                },
            ]
        );
        assert!(parsed.is_cancel_label("duplicate of"));
        assert_eq!(
            parsed.cancel_label_for_inverse("duplicated by"),
            Some("Duplicate Of")
        );
        assert!(!parsed.is_cancel_label("duplicated by"));
    }

    #[test]
    fn invalid_cancel_entries_warn_and_keep_valid_peers() {
        let parsed = parse_relations_config(
            "relations:\n  linked_from_lines: 'yes'\n  cancel_labels:\n    - duplicate of\n    - duplicate of: true\n    - 1\n    - 'bad | label'\n",
            Path::new("/config.yml"),
        )
        .expect("cancel-label problems are not config errors");
        assert!(parsed.linked_from_lines);
        assert_eq!(parsed.cancel_labels.len(), 1);
        assert_eq!(parsed.cancel_labels[0].key, "duplicate of");
        assert_eq!(parsed.warnings.len(), 4);
        assert!(parsed.warnings[0].starts_with("relations.linked_from_lines"));
    }

    #[test]
    fn missing_file_loads_defaults() {
        let settings =
            load_relations_config(Path::new("/definitely/missing/config.yml"))
                .expect("missing file gives defaults");
        assert_eq!(settings, RelationsConfig::default());
    }
}
