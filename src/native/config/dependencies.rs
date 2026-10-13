use std::path::Path;

use serde_yaml::Value;

use super::{ConfigError, RawConfig};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DependenciesConfig {
    pub(crate) blocks_lines: bool,
}

impl Default for DependenciesConfig {
    fn default() -> Self {
        Self { blocks_lines: true }
    }
}

pub(crate) fn load_dependencies_config(
    path: &Path,
) -> Result<DependenciesConfig, ConfigError> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(DependenciesConfig::default());
        }
        Err(error) => {
            return Err(ConfigError::Read(format!(
                "read {}: {error}",
                path.display()
            )));
        }
    };
    parse_dependencies_config(&text, path)
}

fn parse_dependencies_config(
    text: &str,
    path: &Path,
) -> Result<DependenciesConfig, ConfigError> {
    let config: RawConfig = serde_yaml::from_str(text).map_err(|error| {
        ConfigError::Invalid(format!("parse {}: {error}", path.display()))
    })?;
    let Some(dependencies) = config.dependencies else {
        return Ok(DependenciesConfig::default());
    };
    if dependencies.is_null() {
        return Ok(DependenciesConfig::default());
    }
    let path_display = path.display();
    let mapping = dependencies.as_mapping().ok_or_else(|| {
        ConfigError::Invalid(format!(
            "dependencies in {path_display} must be a mapping"
        ))
    })?;
    let key = Value::String("blocks_lines".to_string());
    let blocks_lines = match mapping.get(&key) {
        None | Some(Value::Null) => true,
        Some(Value::Bool(value)) => *value,
        Some(value) => {
            return Err(ConfigError::Invalid(format!(
                "dependencies.blocks_lines in {path_display} must be a boolean; got {}",
                yaml_type(value)
            )));
        }
    };
    Ok(DependenciesConfig { blocks_lines })
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
    fn blocks_lines_defaults_true_and_parses_false() {
        let defaulted = parse_dependencies_config(
            "properties: []\n",
            Path::new("/config.yml"),
        )
        .expect("absent dependencies block");
        assert!(defaulted.blocks_lines);

        let null_block = parse_dependencies_config(
            "dependencies:\n",
            Path::new("/config.yml"),
        )
        .expect("null dependencies block");
        assert!(null_block.blocks_lines);

        let null_value = parse_dependencies_config(
            "dependencies:\n  blocks_lines: null\n",
            Path::new("/config.yml"),
        )
        .expect("null key falls back to default");
        assert!(null_value.blocks_lines);

        let enabled = parse_dependencies_config(
            "dependencies:\n  blocks_lines: true\n",
            Path::new("/config.yml"),
        )
        .expect("explicit true");
        assert!(enabled.blocks_lines);

        let disabled = parse_dependencies_config(
            "dependencies:\n  blocks_lines: false\n",
            Path::new("/config.yml"),
        )
        .expect("explicit false");
        assert!(!disabled.blocks_lines);
    }

    #[test]
    fn rejects_invalid_dependencies_values() {
        for text in [
            "dependencies: [true]\n",
            "dependencies:\n  blocks_lines: \"yes\"\n",
            "dependencies:\n  blocks_lines: 1\n",
        ] {
            assert!(
                matches!(
                    parse_dependencies_config(text, Path::new("/config.yml")),
                    Err(ConfigError::Invalid(_))
                ),
                "{text:?}"
            );
        }
    }

    #[test]
    fn reports_invalid_blocks_lines_path_and_type() {
        let error = parse_dependencies_config(
            "dependencies:\n  blocks_lines: 1\n",
            Path::new("/tmp/bob-config.yml"),
        )
        .expect_err("integer is invalid");
        assert_eq!(
            error.message(),
            "dependencies.blocks_lines in /tmp/bob-config.yml must be a boolean; got 1"
        );
    }
}
