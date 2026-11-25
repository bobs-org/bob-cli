use std::path::Path;

use super::{ConfigError, RawConfig};

/// How `bob snooze wake` chooses a desktop nudge after the receipt.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum NotifyMode {
    /// Yield to a live Mac Capture claim, otherwise `desktop_notify`.
    #[default]
    Auto,
    /// Always use `desktop_notify`.
    Desktop,
    /// Perform no nudge. The receipt and Darwin post still run.
    Off,
}

/// `snooze:` configuration.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct SnoozeConfig {
    pub(crate) notify: NotifyMode,
}

pub(crate) fn load_snooze_config(
    path: &Path,
) -> Result<SnoozeConfig, ConfigError> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(SnoozeConfig::default());
        }
        Err(error) => {
            return Err(ConfigError::Read(format!(
                "read {}: {error}",
                path.display()
            )));
        }
    };
    parse_snooze_config(&text, path)
}

pub(crate) fn parse_snooze_config(
    text: &str,
    path: &Path,
) -> Result<SnoozeConfig, ConfigError> {
    let config: RawConfig = serde_yaml::from_str(text).map_err(|error| {
        ConfigError::Invalid(format!("parse {}: {error}", path.display()))
    })?;
    let path_display = path.display();
    let Some(value) = config.snooze else {
        return Ok(SnoozeConfig::default());
    };
    if value.is_null() {
        return Ok(SnoozeConfig::default());
    }
    let serde_yaml::Value::Mapping(mapping) = value else {
        return Err(ConfigError::Invalid(format!(
            "snooze in {path_display} must be a mapping"
        )));
    };
    let notify = match mapping.get(serde_yaml::Value::String("notify".into())) {
        None | Some(serde_yaml::Value::Null) => NotifyMode::Auto,
        Some(serde_yaml::Value::String(raw)) => match raw.trim() {
            "auto" => NotifyMode::Auto,
            "desktop" => NotifyMode::Desktop,
            "off" => NotifyMode::Off,
            other => {
                return Err(ConfigError::Invalid(format!(
                    "snooze.notify in {path_display} must be auto, desktop, or off (got {other:?})"
                )));
            }
        },
        Some(other) => {
            return Err(ConfigError::Invalid(format!(
                "snooze.notify in {path_display} must be auto, desktop, or off (got {})",
                serde_yaml::to_string(other).unwrap_or_default().trim()
            )));
        }
    };
    Ok(SnoozeConfig { notify })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> Result<SnoozeConfig, ConfigError> {
        parse_snooze_config(text, Path::new("config.yml"))
    }

    #[test]
    fn defaults_when_missing() {
        let config = parse("timezone: UTC\n").expect("missing snooze");
        assert_eq!(config, SnoozeConfig::default());
        assert_eq!(config.notify, NotifyMode::Auto);
    }

    #[test]
    fn accepts_each_notify_mode() {
        for (text, mode) in [
            ("snooze:\n  notify: auto\n", NotifyMode::Auto),
            ("snooze:\n  notify: desktop\n", NotifyMode::Desktop),
            ("snooze:\n  notify: off\n", NotifyMode::Off),
        ] {
            assert_eq!(parse(text).expect(text).notify, mode, "{text}");
        }
    }

    #[test]
    fn rejects_unknown_notify_value() {
        let error = parse("snooze:\n  notify: banner\n")
            .expect_err("unknown notify value");
        assert!(error.to_string().contains("snooze.notify"), "{error}");
    }
}
