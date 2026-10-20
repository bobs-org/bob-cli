use std::path::Path;

use serde_yaml::Value;

use super::{ConfigError, RawConfig};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RelationsConfig {
    pub(crate) linked_from_lines: bool,
}

impl Default for RelationsConfig {
    fn default() -> Self {
        Self {
            linked_from_lines: true,
        }
    }
}

#[allow(dead_code)]
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

fn parse_relations_config(
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
    let mapping = relations.as_mapping().ok_or_else(|| {
        ConfigError::Invalid(format!(
            "relations in {path_display} must be a mapping"
        ))
    })?;
    let key = Value::String("linked_from_lines".to_string());
    let linked_from_lines = match mapping.get(&key) {
        None | Some(Value::Null) => true,
        Some(Value::Bool(value)) => *value,
        Some(value) => {
            return Err(ConfigError::Invalid(format!(
                "relations.linked_from_lines in {path_display} must be a boolean; got {}",
                yaml_type(value)
            )));
        }
    };
    Ok(RelationsConfig { linked_from_lines })
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
    fn rejects_invalid_relations_values() {
        for text in [
            "relations: [true]\n",
            "relations:\n  linked_from_lines: \"yes\"\n",
            "relations:\n  linked_from_lines: 1\n",
        ] {
            assert!(
                matches!(
                    parse_relations_config(text, Path::new("/config.yml")),
                    Err(ConfigError::Invalid(_))
                ),
                "{text:?}"
            );
        }
    }

    #[test]
    fn reports_invalid_linked_from_lines_path_and_type() {
        let error = parse_relations_config(
            "relations:\n  linked_from_lines: 1\n",
            Path::new("/tmp/bob-config.yml"),
        )
        .expect_err("integer is invalid");
        assert_eq!(
            error.message(),
            "relations.linked_from_lines in /tmp/bob-config.yml must be a boolean; got 1"
        );
    }

    #[test]
    fn missing_file_loads_defaults() {
        let settings =
            load_relations_config(Path::new("/definitely/missing/config.yml"))
                .expect("missing file gives defaults");
        assert_eq!(settings, RelationsConfig::default());
    }
}
