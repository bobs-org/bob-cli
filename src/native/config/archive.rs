use std::path::Path;

use super::ConfigError;

/// Keep-window configuration for `bob task archive` (`archive:` block).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ArchiveConfig {
    pub(crate) keep_days: u32,
    pub(crate) keep_max: u32,
}

impl Default for ArchiveConfig {
    fn default() -> Self {
        Self {
            keep_days: 7,
            keep_max: 10,
        }
    }
}

pub(crate) fn load_archive_config(
    path: &Path,
) -> Result<ArchiveConfig, ConfigError> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(ArchiveConfig::default());
        }
        Err(error) => {
            return Err(ConfigError::Read(format!(
                "read {}: {error}",
                path.display()
            )));
        }
    };
    parse_archive_config(&text, path)
}

pub(crate) fn parse_archive_config(
    text: &str,
    path: &Path,
) -> Result<ArchiveConfig, ConfigError> {
    let config: super::RawConfig =
        serde_yaml::from_str(text).map_err(|error| {
            ConfigError::Invalid(format!("parse {}: {error}", path.display()))
        })?;
    let defaults = ArchiveConfig::default();
    let Some(archive) = config.archive else {
        return Ok(defaults);
    };
    if archive.is_null() {
        return Ok(defaults);
    }
    let serde_yaml::Value::Mapping(mapping) = archive else {
        return Err(ConfigError::Invalid(format!(
            "archive in {} must be a mapping",
            path.display()
        )));
    };

    for key in mapping.keys() {
        let serde_yaml::Value::String(name) = key else {
            return Err(ConfigError::Invalid(format!(
                "archive.{key:?} in {} is an unknown key",
                path.display()
            )));
        };
        if !matches!(name.as_str(), "keep_days" | "keep_max") {
            return Err(ConfigError::Invalid(format!(
                "archive.{name} in {} is an unknown key",
                path.display()
            )));
        }
    }

    let parse = |name: &str, default: u32, maximum: u32| {
        let key = serde_yaml::Value::String(name.to_string());
        let Some(value) = mapping.get(&key) else {
            return Ok(default);
        };
        let number = match value {
            serde_yaml::Value::Number(number) => {
                number.as_i64().or_else(|| {
                    number.as_u64().and_then(|n| i64::try_from(n).ok())
                })
            }
            _ => None,
        };
        let Some(number) =
            number.filter(|number| (1..=i64::from(maximum)).contains(number))
        else {
            return Err(ConfigError::Invalid(format!(
                "archive.{name} in {} must be an integer from 1 to {maximum}; got {value:?}",
                path.display()
            )));
        };
        Ok(number as u32)
    };

    Ok(ArchiveConfig {
        keep_days: parse("keep_days", defaults.keep_days, 3650)?,
        keep_max: parse("keep_max", defaults.keep_max, 999)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn archive_config_defaults_and_accepts_custom_values() {
        let path = Path::new("/config.yml");
        assert_eq!(
            parse_archive_config("", path).unwrap(),
            ArchiveConfig::default()
        );
        assert_eq!(
            load_archive_config(Path::new(
                "/definitely/missing/bob-config.yml"
            ))
            .unwrap(),
            ArchiveConfig::default()
        );
        assert_eq!(
            parse_archive_config(
                "archive:\n  keep_days: 30\n  keep_max: 12\n",
                path
            )
            .unwrap(),
            ArchiveConfig {
                keep_days: 30,
                keep_max: 12
            }
        );
    }

    #[test]
    fn archive_config_rejects_invalid_values_and_unknown_keys_by_name() {
        for (text, key) in [
            ("archive:\n  keep_days: 0\n", "archive.keep_days"),
            ("archive:\n  keep_days: -1\n", "archive.keep_days"),
            ("archive:\n  keep_days: 3651\n", "archive.keep_days"),
            ("archive:\n  keep_days: many\n", "archive.keep_days"),
            ("archive:\n  keep_days: null\n", "archive.keep_days"),
            ("archive:\n  keep_max: 0\n", "archive.keep_max"),
            ("archive:\n  keep_max: -1\n", "archive.keep_max"),
            ("archive:\n  keep_max: 1000\n", "archive.keep_max"),
            ("archive:\n  keep_max: 1.5\n", "archive.keep_max"),
            ("archive:\n  keep_max: no\n", "archive.keep_max"),
            ("archive:\n  keep_window: 7\n", "archive.keep_window"),
        ] {
            let error = parse_archive_config(text, Path::new("/config.yml"))
                .unwrap_err()
                .to_string();
            assert!(error.contains(key), "{text:?}: {error}");
        }
    }
}
