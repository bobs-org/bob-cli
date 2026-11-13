use std::path::Path;

use super::{ConfigError, RawConfig};

/// Allowlist of plugin ids `bob plugins sync` deploys (`plugins:` block).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PluginsConfig {
    /// `None` means the list is unset; `Some` (possibly empty) is explicit.
    /// Order follows the config file; callers sort when they report.
    pub(crate) sync: Option<Vec<String>>,
}

pub(crate) fn load_plugins_config(
    path: &Path,
) -> Result<PluginsConfig, ConfigError> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(PluginsConfig { sync: None });
        }
        Err(error) => {
            return Err(ConfigError::Read(format!(
                "read {}: {error}",
                path.display()
            )));
        }
    };
    parse_plugins_config(&text, path)
}

pub(crate) fn parse_plugins_config(
    text: &str,
    path: &Path,
) -> Result<PluginsConfig, ConfigError> {
    let config: RawConfig = serde_yaml::from_str(text).map_err(|error| {
        ConfigError::Invalid(format!("parse {}: {error}", path.display()))
    })?;
    let path_display = path.display();
    let Some(plugins) = config.plugins else {
        return Ok(PluginsConfig { sync: None });
    };
    if plugins.is_null() {
        return Ok(PluginsConfig { sync: None });
    }
    let serde_yaml::Value::Mapping(mapping) = &plugins else {
        return Err(ConfigError::Invalid(format!(
            "plugins in {path_display} must be a mapping"
        )));
    };
    let key = serde_yaml::Value::String("sync".to_string());
    let Some(sync) = mapping.get(&key) else {
        return Ok(PluginsConfig { sync: None });
    };
    if sync.is_null() {
        return Ok(PluginsConfig { sync: None });
    }
    let serde_yaml::Value::Sequence(entries) = sync else {
        return Err(ConfigError::Invalid(format!(
            "plugins.sync in {path_display} must be a sequence; got {}",
            yaml_type(sync)
        )));
    };
    // Unknown sibling keys under `plugins:` are ignored so hosts on the
    // daily release train can share one config across bob versions.
    let mut ids = Vec::with_capacity(entries.len());
    let mut seen = std::collections::HashSet::new();
    for (index, entry) in entries.iter().enumerate() {
        let number = index + 1;
        let serde_yaml::Value::String(raw) = entry else {
            return Err(ConfigError::Invalid(format!(
                "plugins.sync entry {number} in {path_display} must be a string; got {}",
                yaml_type(entry)
            )));
        };
        let trimmed = raw.trim().to_string();
        if trimmed.is_empty() {
            return Err(ConfigError::Invalid(format!(
                "plugins.sync entry {number} in {path_display} must be a non-empty plugin id; got {raw:?}"
            )));
        }
        if !seen.insert(trimmed.clone()) {
            return Err(ConfigError::Invalid(format!(
                "plugins.sync entry {number} {trimmed:?} in {path_display} duplicates an earlier entry"
            )));
        }
        ids.push(trimmed);
    }
    Ok(PluginsConfig { sync: Some(ids) })
}

fn yaml_type(value: &serde_yaml::Value) -> String {
    match value {
        serde_yaml::Value::Null => "null".to_string(),
        serde_yaml::Value::Bool(value) => value.to_string(),
        serde_yaml::Value::Number(value) => value.to_string(),
        serde_yaml::Value::String(value) => format!("{value:?}"),
        serde_yaml::Value::Sequence(_) => "sequence".to_string(),
        serde_yaml::Value::Mapping(_) => "mapping".to_string(),
        serde_yaml::Value::Tagged(_) => "tagged value".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plugins_config_unset_cases() {
        let path = Path::new("/config.yml");
        assert_eq!(
            parse_plugins_config("", path).unwrap(),
            PluginsConfig { sync: None }
        );
        assert_eq!(
            parse_plugins_config("properties: []\n", path).unwrap(),
            PluginsConfig { sync: None }
        );
        assert_eq!(
            parse_plugins_config("plugins:\n", path).unwrap(),
            PluginsConfig { sync: None }
        );
        assert_eq!(
            parse_plugins_config("plugins: null\n", path).unwrap(),
            PluginsConfig { sync: None }
        );
        assert_eq!(
            parse_plugins_config("plugins:\n  sync:\n", path).unwrap(),
            PluginsConfig { sync: None }
        );
        assert_eq!(
            parse_plugins_config("plugins:\n  sync: null\n", path).unwrap(),
            PluginsConfig { sync: None }
        );
        assert_eq!(
            load_plugins_config(Path::new(
                "/definitely/missing/bob-config.yml"
            ))
            .unwrap(),
            PluginsConfig { sync: None }
        );
    }

    #[test]
    fn plugins_config_parses_list_trims_and_keeps_order() {
        let config = parse_plugins_config(
            "plugins:\n  sync:\n    - beta\n    - alpha\n",
            Path::new("/config.yml"),
        )
        .unwrap();
        assert_eq!(
            config.sync,
            Some(vec!["beta".to_string(), "alpha".to_string()])
        );

        let trimmed = parse_plugins_config(
            "plugins:\n  sync:\n    - \"  alpha  \"\n",
            Path::new("/config.yml"),
        )
        .unwrap();
        assert_eq!(trimmed.sync, Some(vec!["alpha".to_string()]));
    }

    #[test]
    fn plugins_config_empty_list_deploys_nothing() {
        let config = parse_plugins_config(
            "plugins:\n  sync: []\n",
            Path::new("/config.yml"),
        )
        .unwrap();
        assert_eq!(config.sync, Some(Vec::new()));
    }

    #[test]
    fn plugins_config_ignores_unknown_sibling_keys() {
        let config = parse_plugins_config(
            "plugins:\n  sync:\n    - alpha\n  future_key: true\n",
            Path::new("/config.yml"),
        )
        .unwrap();
        assert_eq!(config.sync, Some(vec!["alpha".to_string()]));
    }

    #[test]
    fn plugins_config_rejects_bad_shapes() {
        for (text, key) in [
            ("plugins: [true]\n", "plugins"),
            ("plugins:\n  sync: alpha\n", "plugins.sync"),
            ("plugins:\n  sync:\n    - 1\n", "plugins.sync"),
            ("plugins:\n  sync:\n    - true\n", "plugins.sync"),
            ("plugins:\n  sync:\n    - \"\"\n", "plugins.sync"),
            ("plugins:\n  sync:\n    - \"   \"\n", "plugins.sync"),
            (
                "plugins:\n  sync:\n    - alpha\n    - alpha\n",
                "plugins.sync",
            ),
            (
                "plugins:\n  sync:\n    - alpha\n    - \" alpha \"\n",
                "plugins.sync",
            ),
        ] {
            let error =
                parse_plugins_config(text, Path::new("/tmp/bob-config.yml"))
                    .expect_err("invalid plugins config");
            assert!(
                error.to_string().contains(key)
                    && error.to_string().contains("/tmp/bob-config.yml"),
                "{text:?}: {}",
                error
            );
        }
    }
}
