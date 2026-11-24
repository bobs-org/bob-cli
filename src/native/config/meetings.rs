use std::path::Path;

use super::{ConfigError, RawConfig};

/// Meeting notes reference configuration (`meetings:` block).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MeetingsConfig {
    pub(crate) producer: Option<String>,
    pub(crate) relation_label: String,
    pub(crate) status: String,
    pub(crate) window_days: u32,
}

impl Default for MeetingsConfig {
    fn default() -> Self {
        Self {
            producer: None,
            relation_label: "notes from".to_string(),
            status: "next".to_string(),
            window_days: 7,
        }
    }
}

impl MeetingsConfig {
    #[allow(dead_code)]
    pub(crate) fn producer(&self) -> Option<&str> {
        self.producer.as_deref()
    }

    pub(crate) fn relation_label(&self) -> &str {
        &self.relation_label
    }

    #[allow(dead_code)]
    pub(crate) fn status(&self) -> &str {
        &self.status
    }

    #[allow(dead_code)]
    pub(crate) fn window_days(&self) -> u32 {
        self.window_days
    }
}

pub(crate) fn load_meetings_config(
    path: &Path,
) -> Result<MeetingsConfig, ConfigError> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(MeetingsConfig::default());
        }
        Err(error) => {
            return Err(ConfigError::Read(format!(
                "read {}: {error}",
                path.display()
            )));
        }
    };
    parse_meetings_config(&text, path)
}

fn meetings_value(
    text: &str,
    path: &Path,
) -> Result<Option<serde_yaml::Value>, ConfigError> {
    let config: RawConfig = serde_yaml::from_str(text).map_err(|error| {
        ConfigError::Invalid(format!("parse {}: {error}", path.display()))
    })?;
    Ok(config.meetings)
}

pub(crate) fn parse_meetings_config(
    text: &str,
    path: &Path,
) -> Result<MeetingsConfig, ConfigError> {
    let value = meetings_value(text, path)?;
    let path_display = path.display();
    let defaults = MeetingsConfig::default();

    let Some(value) = value else {
        return Ok(defaults);
    };
    if value.is_null() {
        return Ok(defaults);
    }
    let serde_yaml::Value::Mapping(ref mapping) = value else {
        return Err(ConfigError::Invalid(format!(
            "meetings in {path_display} must be a mapping"
        )));
    };

    let get = |name: &str| {
        let key = serde_yaml::Value::String(name.to_string());
        mapping.get(&key).cloned()
    };

    let producer = match get("producer") {
        None | Some(serde_yaml::Value::Null) => None,
        Some(serde_yaml::Value::String(raw)) => {
            let trimmed = raw.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed.to_string())
            }
        }
        Some(other) => {
            return Err(ConfigError::Invalid(format!(
                "meetings.producer in {path_display} must be a string (got {})",
                serde_yaml::to_string(&other).unwrap_or_default().trim()
            )));
        }
    };

    let relation_label = match get("relation_label") {
        None | Some(serde_yaml::Value::Null) => defaults.relation_label.clone(),
        Some(serde_yaml::Value::String(raw)) => {
            match crate::native::task_relations::label::normalize_relation_label(
                &raw,
            ) {
                Ok(label) => label,
                Err(error) => {
                    return Err(ConfigError::Invalid(format!(
                        "meetings.relation_label in {path_display} is invalid: {}; fix or remove it",
                        error.as_str()
                    )));
                }
            }
        }
        Some(other) => {
            return Err(ConfigError::Invalid(format!(
                "meetings.relation_label in {path_display} must be a string (got {})",
                serde_yaml::to_string(&other).unwrap_or_default().trim()
            )));
        }
    };

    let status = match get("status") {
        None | Some(serde_yaml::Value::Null) => defaults.status.clone(),
        Some(serde_yaml::Value::String(raw)) => {
            let trimmed = raw.trim().to_ascii_lowercase();
            if ["next", "ready", "wip", "read"].contains(&trimmed.as_str()) {
                trimmed
            } else {
                return Err(ConfigError::Invalid(format!(
                    "meetings.status in {path_display} must be one of next, ready, wip, read (got '{raw}')"
                )));
            }
        }
        Some(other) => {
            return Err(ConfigError::Invalid(format!(
                "meetings.status in {path_display} must be a string (got {})",
                serde_yaml::to_string(&other).unwrap_or_default().trim()
            )));
        }
    };

    let window_days = match get("window_days") {
        None | Some(serde_yaml::Value::Null) => defaults.window_days,
        Some(serde_yaml::Value::Number(number)) => {
            let days = number.as_u64().ok_or_else(|| {
                ConfigError::Invalid(format!(
                    "meetings.window_days in {path_display} must be an integer from 1 to 365"
                ))
            })?;
            if (1..=365).contains(&days) {
                days as u32
            } else {
                return Err(ConfigError::Invalid(format!(
                    "meetings.window_days in {path_display} must be an integer from 1 to 365"
                )));
            }
        }
        Some(serde_yaml::Value::String(raw)) => {
            let parsed: u64 = raw.trim().parse().map_err(|_| {
                ConfigError::Invalid(format!(
                    "meetings.window_days in {path_display} must be an integer from 1 to 365"
                ))
            })?;
            if (1..=365).contains(&parsed) {
                parsed as u32
            } else {
                return Err(ConfigError::Invalid(format!(
                    "meetings.window_days in {path_display} must be an integer from 1 to 365"
                )));
            }
        }
        Some(_) => {
            return Err(ConfigError::Invalid(format!(
                "meetings.window_days in {path_display} must be an integer from 1 to 365"
            )));
        }
    };

    Ok(MeetingsConfig {
        producer,
        relation_label,
        status,
        window_days,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> Result<MeetingsConfig, ConfigError> {
        parse_meetings_config(text, Path::new("config.yml"))
    }

    #[test]
    fn defaults_when_missing() {
        let config = parse("highlights:\n  table_layout: auto\n")
            .expect("missing meetings uses defaults");
        assert_eq!(config, MeetingsConfig::default());
    }

    #[test]
    fn accepts_full_meetings_block() {
        let config = parse(
            "meetings:\n  producer: Wispr Flow\n  relation_label: notes from\n  status: next\n  window_days: 7\n",
        )
        .expect("valid meetings block parses");
        assert_eq!(config.producer.as_deref(), Some("Wispr Flow"));
        assert_eq!(config.relation_label, "notes from");
        assert_eq!(config.status, "next");
        assert_eq!(config.window_days, 7);
    }

    #[test]
    fn rejects_invalid_relation_label() {
        let error = parse("meetings:\n  relation_label: 'bad | label'\n")
            .expect_err("invalid label must fail");
        assert!(error.to_string().contains("meetings.relation_label"));
    }
}
