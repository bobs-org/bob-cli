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

/// Which clipboard history source `%c<N>` reads.
///
/// `auto` prefers Alfred, then Clipy, on macOS. `command` reads
/// `clipboard_history_command`. Parsed independently of
/// `max_saved_templates`: a bad value in one never changes how the other
/// loads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum ClipboardHistorySetting {
    #[default]
    Auto,
    Alfred,
    Clipy,
    Command,
    Off,
}

impl ClipboardHistorySetting {
    pub(crate) fn as_str(&self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Alfred => "alfred",
            Self::Clipy => "clipy",
            Self::Command => "command",
            Self::Off => "off",
        }
    }
}

/// The two `capture` clipboard keys, loaded independently of
/// `max_saved_templates`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ClipboardHistoryConfig {
    pub(crate) setting: ClipboardHistorySetting,
    pub(crate) command: Vec<String>,
}

impl Default for ClipboardHistoryConfig {
    fn default() -> Self {
        Self {
            setting: ClipboardHistorySetting::Auto,
            command: Vec::new(),
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

pub(crate) fn load_clipboard_history_config(
    path: &Path,
) -> Result<ClipboardHistoryConfig, super::ConfigError> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(ClipboardHistoryConfig::default());
        }
        Err(error) => {
            return Err(super::ConfigError::Read(format!(
                "read {}: {error}",
                path.display()
            )));
        }
    };
    parse_clipboard_history_config(&text, path)
}

fn parse_clipboard_history_config(
    text: &str,
    path: &Path,
) -> Result<ClipboardHistoryConfig, super::ConfigError> {
    let config: super::RawConfig =
        serde_yaml::from_str(text).map_err(|error| {
            super::ConfigError::Invalid(format!(
                "parse {}: {error}",
                path.display()
            ))
        })?;
    let path_display = path.display();
    let defaults = ClipboardHistoryConfig::default();

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

    let setting = match mapping
        .get(serde_yaml::Value::String("clipboard_history".to_string()))
        .cloned()
    {
        None => ClipboardHistorySetting::Auto,
        Some(value) if value.is_null() => ClipboardHistorySetting::Auto,
        Some(serde_yaml::Value::String(text)) => match text.as_str() {
            "auto" => ClipboardHistorySetting::Auto,
            "alfred" => ClipboardHistorySetting::Alfred,
            "clipy" => ClipboardHistorySetting::Clipy,
            "command" => ClipboardHistorySetting::Command,
            "off" => ClipboardHistorySetting::Off,
            _ => {
                return Err(super::ConfigError::Invalid(format!(
                    "capture.clipboard_history in {path_display} must be one of auto, alfred, clipy, command, off; got {text:?}"
                )));
            }
        },
        Some(value) => {
            return Err(super::ConfigError::Invalid(format!(
                "capture.clipboard_history in {path_display} must be one of auto, alfred, clipy, command, off; got {}",
                render_scalar(&value)
            )));
        }
    };

    // The command list is ignored under any other value.
    if setting != ClipboardHistorySetting::Command {
        return Ok(ClipboardHistoryConfig {
            setting,
            command: Vec::new(),
        });
    }
    let Some(value) = mapping
        .get(serde_yaml::Value::String(
            "clipboard_history_command".to_string(),
        ))
        .cloned()
    else {
        return Err(super::ConfigError::Invalid(format!(
            "capture.clipboard_history is `command` in {path_display} but capture.clipboard_history_command is missing; set it to a non-empty YAML list of non-empty argv strings"
        )));
    };
    let serde_yaml::Value::Sequence(items) = value else {
        return Err(super::ConfigError::Invalid(format!(
            "capture.clipboard_history_command in {path_display} must be a non-empty YAML list of non-empty argv strings"
        )));
    };
    if items.is_empty() {
        return Err(super::ConfigError::Invalid(format!(
            "capture.clipboard_history_command in {path_display} must be a non-empty YAML list of non-empty argv strings"
        )));
    }
    let mut command = Vec::with_capacity(items.len());
    for item in items {
        let serde_yaml::Value::String(arg) = item else {
            return Err(super::ConfigError::Invalid(format!(
                "capture.clipboard_history_command in {path_display} must be a non-empty YAML list of non-empty argv strings"
            )));
        };
        if arg.is_empty() {
            return Err(super::ConfigError::Invalid(format!(
                "capture.clipboard_history_command in {path_display} must be a non-empty YAML list of non-empty argv strings"
            )));
        }
        command.push(arg);
    }
    Ok(ClipboardHistoryConfig { setting, command })
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

    fn parse_history(
        text: &str,
    ) -> Result<ClipboardHistoryConfig, super::super::ConfigError> {
        parse_clipboard_history_config(text, Path::new("test-config.yml"))
    }

    #[test]
    fn clipboard_history_defaults_to_auto() {
        for text in [
            "properties: []\n",
            "capture:\n",
            "capture: null\n",
            "capture:\n  max_saved_templates: 10\n",
            "capture:\n  clipboard_history: null\n",
        ] {
            let config = parse_history(text).expect("parse");
            assert_eq!(config.setting, ClipboardHistorySetting::Auto);
            assert!(config.command.is_empty(), "{text:?}");
        }
    }

    #[test]
    fn clipboard_history_accepts_every_setting() {
        for (text, setting) in [
            ("auto", ClipboardHistorySetting::Auto),
            ("alfred", ClipboardHistorySetting::Alfred),
            ("clipy", ClipboardHistorySetting::Clipy),
            ("off", ClipboardHistorySetting::Off),
        ] {
            let config = parse_history(&format!(
                "capture:\n  clipboard_history: {text}\n"
            ))
            .expect("parse");
            assert_eq!(config.setting, setting, "{text}");
        }
        let config = parse_history(
            "capture:\n  clipboard_history: command\n  clipboard_history_command:\n    - hist\n    - --flag\n",
        )
        .expect("parse");
        assert_eq!(config.setting, ClipboardHistorySetting::Command);
        assert_eq!(config.command, ["hist", "--flag"]);
    }

    #[test]
    fn clipboard_history_rejects_invalid_scalars() {
        for text in [
            "capture:\n  clipboard_history: spotlight\n",
            "capture:\n  clipboard_history: 3\n",
            "capture:\n  clipboard_history: true\n",
            "capture:\n  clipboard_history:\n    - alfred\n",
        ] {
            let error = parse_history(text).expect_err("invalid setting");
            assert!(
                error.to_string().contains("capture.clipboard_history"),
                "{error:?}"
            );
        }
    }

    #[test]
    fn clipboard_history_command_requires_a_nonempty_argv_list() {
        for text in [
            "capture:\n  clipboard_history: command\n",
            "capture:\n  clipboard_history: command\n  clipboard_history_command: hist\n",
            "capture:\n  clipboard_history: command\n  clipboard_history_command: []\n",
            "capture:\n  clipboard_history: command\n  clipboard_history_command:\n    - hist\n    - ''\n",
            "capture:\n  clipboard_history: command\n  clipboard_history_command:\n    - 3\n",
        ] {
            let error = parse_history(text).expect_err("invalid command");
            assert!(
                error
                    .to_string()
                    .contains("capture.clipboard_history_command"),
                "{error:?}"
            );
        }
    }

    #[test]
    fn clipboard_command_list_is_ignored_under_other_values() {
        let config = parse_history(
            "capture:\n  clipboard_history: alfred\n  clipboard_history_command:\n    - hist\n",
        )
        .expect("parse");
        assert_eq!(config.setting, ClipboardHistorySetting::Alfred);
        assert!(config.command.is_empty());
        // Even a malformed list is ignored when the setting is not `command`.
        let config = parse_history(
            "capture:\n  clipboard_history: alfred\n  clipboard_history_command: []\n",
        )
        .expect("parse");
        assert_eq!(config.setting, ClipboardHistorySetting::Alfred);
    }

    #[test]
    fn clipboard_keys_load_independently_of_max_saved_templates() {
        // A bad shelf cap still leaves the clipboard keys readable.
        assert!(parse("capture:\n  max_saved_templates: 99\n").is_err());
        let config = parse_history(
            "capture:\n  max_saved_templates: 99\n  clipboard_history: alfred\n",
        )
        .expect("clipboard keys load despite a bad cap");
        assert_eq!(config.setting, ClipboardHistorySetting::Alfred);
        // A bad clipboard value still leaves the shelf cap readable, with
        // its established error text unchanged.
        let error = parse_history("capture:\n  clipboard_history: spotlight\n")
            .expect_err("bad clipboard setting");
        assert!(error.to_string().contains("capture.clipboard_history"));
        assert_eq!(
            parse(
                "capture:\n  max_saved_templates: 12\n  clipboard_history: spotlight\n"
            )
            .expect("shelf cap loads despite a bad clipboard value")
            .max_saved_templates,
            12
        );
    }
}
