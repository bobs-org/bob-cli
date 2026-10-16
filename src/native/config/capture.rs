//! Saved-template shelf capacity (`capture:` block).
use std::path::Path;

/// Default shelf capacity: fills the stash's one-key accelerator alphabet.
pub(crate) const DEFAULT_MAX_SAVED_TEMPLATES: u32 = 36;

/// `capture.max_saved_templates`, tolerant like the other capture-adjacent
/// loaders: missing or null means the default, `0` turns saving off.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CaptureConfig {
    pub(crate) max_saved_templates: u32,
}

impl Default for CaptureConfig {
    fn default() -> Self {
        Self {
            max_saved_templates: DEFAULT_MAX_SAVED_TEMPLATES,
        }
    }
}

impl CaptureConfig {
    pub(crate) fn max_saved_templates(&self) -> u32 {
        self.max_saved_templates
    }
}

pub(crate) fn load_capture_config(
    path: &Path,
) -> Result<CaptureConfig, super::ConfigError> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(CaptureConfig::default());
        }
        Err(error) => {
            return Err(super::ConfigError::Read(format!(
                "read {}: {error}",
                path.display()
            )));
        }
    };
    parse_capture_config(&text, path)
}

fn parse_capture_config(
    text: &str,
    path: &Path,
) -> Result<CaptureConfig, super::ConfigError> {
    let config: super::RawConfig =
        serde_yaml::from_str(text).map_err(|error| {
            super::ConfigError::Invalid(format!(
                "parse {}: {error}",
                path.display()
            ))
        })?;
    let path_display = path.display();
    let defaults = CaptureConfig::default();

    let Some(capture) = config.capture else {
        return Ok(defaults);
    };
    if capture.is_null() {
        return Ok(defaults);
    }
    let serde_yaml::Value::Mapping(ref mapping) = capture else {
        return Err(super::ConfigError::Invalid(format!(
            "capture in {path_display} must be a mapping"
        )));
    };

    let key = serde_yaml::Value::String("max_saved_templates".to_string());
    let value = mapping.get(&key).cloned();
    let Some(value) = value else {
        return Ok(defaults);
    };
    if value.is_null() {
        return Ok(defaults);
    }
    let number = match &value {
        serde_yaml::Value::Number(number) => {
            if let Some(int) = number.as_i64() {
                int
            } else if let Some(uint) = number.as_u64()
                && let Ok(int) = i64::try_from(uint)
            {
                int
            } else {
                return Err(super::ConfigError::Invalid(format!(
                    "capture.max_saved_templates in {path_display} must be an integer from 0 to 36; got {value:?}"
                )));
            }
        }
        _ => {
            return Err(super::ConfigError::Invalid(format!(
                "capture.max_saved_templates in {path_display} must be an integer from 0 to 36; got {}",
                render_scalar(&value)
            )));
        }
    };
    if !(0..=36).contains(&number) {
        return Err(super::ConfigError::Invalid(format!(
            "capture.max_saved_templates in {path_display} must be an integer from 0 to 36; got {number}"
        )));
    }
    Ok(CaptureConfig {
        max_saved_templates: u32::try_from(number).expect("range checked"),
    })
}

fn render_scalar(value: &serde_yaml::Value) -> String {
    match value {
        serde_yaml::Value::String(text) => format!("{text:?}"),
        serde_yaml::Value::Number(number) => number.to_string(),
        serde_yaml::Value::Bool(flag) => flag.to_string(),
        serde_yaml::Value::Null => "null".to_string(),
        serde_yaml::Value::Sequence(_) => "sequence".to_string(),
        serde_yaml::Value::Mapping(_) => "mapping".to_string(),
        serde_yaml::Value::Tagged(tagged) => {
            format!("tagged {}", tagged.tag)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn parse(text: &str) -> Result<CaptureConfig, super::super::ConfigError> {
        parse_capture_config(text, Path::new("test-config.yml"))
    }

    #[test]
    fn missing_capture_block_uses_default() {
        assert_eq!(
            parse("properties: []\n")
                .expect("parse")
                .max_saved_templates,
            36
        );
    }

    #[test]
    fn rejects_out_of_range_and_non_integer() {
        assert!(parse("capture:\n  max_saved_templates: 37\n").is_err());
        assert!(parse("capture:\n  max_saved_templates: -1\n").is_err());
        assert!(parse("capture:\n  max_saved_templates: many\n").is_err());
    }

    #[test]
    fn accepts_zero_through_cap() {
        assert_eq!(
            parse("capture:\n  max_saved_templates: 0\n")
                .expect("parse")
                .max_saved_templates,
            0
        );
        assert_eq!(
            parse("capture:\n  max_saved_templates: 36\n")
                .expect("parse")
                .max_saved_templates,
            36
        );
    }
}
