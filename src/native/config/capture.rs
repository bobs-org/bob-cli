//! Saved-template shelf capacity and editor type leads (`capture:` block).
use std::{collections::BTreeMap, path::Path};

use crate::native::{
    capture_language::{parse_for_editor, EditorMode, SpanKind},
    clock_tags,
    plan_budget::normalize_component,
};

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
    /// Whether `capture.clipboard_history` was authored in the config.
    /// Status reports use this to distinguish the `auto` default from an
    /// explicit config selection.
    pub(crate) setting_configured: bool,
    pub(crate) command: Vec<String>,
}

/// A configured editor-only title lead for a task type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TypeLead {
    /// The spelling used as the key in `capture.type_leads`.
    pub(crate) name: String,
    pub(crate) text: String,
}

/// Independently loaded `capture.type_leads` entries and bounded warnings.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct TypeLeadsConfig {
    pub(crate) leads: BTreeMap<String, TypeLead>,
    pub(crate) warnings: Vec<String>,
}

const TYPE_LEAD_WARNING_LIMIT: usize = 24;
const TYPE_LEAD_WARNING_CHARS: usize = 180;

impl Default for ClipboardHistoryConfig {
    fn default() -> Self {
        Self {
            setting: ClipboardHistorySetting::Auto,
            setting_configured: false,
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

/// Read `capture.type_leads` without allowing its errors to affect any other
/// config consumer. A missing file and a syntactically invalid YAML file both
/// produce no leads; the latter is already reported by the config loaders
/// that own the whole-file parse.
pub(crate) fn load_type_leads_config(path: &Path) -> TypeLeadsConfig {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return TypeLeadsConfig::default();
        }
        Err(error) => {
            let mut config = TypeLeadsConfig::default();
            push_type_lead_warning(
                &mut config.warnings,
                format!("capture.type_leads: could not read config: {error}"),
            );
            return config;
        }
    };
    parse_type_leads_config(&text)
}

fn parse_type_leads_config(text: &str) -> TypeLeadsConfig {
    let Ok(root) = serde_yaml::from_str::<serde_yaml::Value>(text) else {
        return TypeLeadsConfig::default();
    };
    let Some(root) = root.as_mapping() else {
        return TypeLeadsConfig::default();
    };
    let capture_key = serde_yaml::Value::String("capture".to_string());
    let Some(capture) = root
        .get(&capture_key)
        .and_then(serde_yaml::Value::as_mapping)
    else {
        return TypeLeadsConfig::default();
    };
    let leads_key = serde_yaml::Value::String("type_leads".to_string());
    let Some(raw_leads) = capture.get(&leads_key) else {
        return TypeLeadsConfig::default();
    };
    if raw_leads.is_null() {
        return TypeLeadsConfig::default();
    }

    let mut config = TypeLeadsConfig::default();
    let serde_yaml::Value::Mapping(raw_leads) = raw_leads else {
        push_type_lead_warning(
            &mut config.warnings,
            "capture.type_leads must be a mapping; ignoring all leads".into(),
        );
        return config;
    };

    let mut entries = Vec::new();
    let mut names_by_key: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (raw_name, raw_text) in raw_leads {
        let serde_yaml::Value::String(name) = raw_name else {
            let shown = render_scalar(raw_name);
            push_type_lead_warning(
                &mut config.warnings,
                format!(
                    "capture.type_leads.<{shown}> must use a string type name; skipping"
                ),
            );
            continue;
        };
        let key = normalize_component(name);
        if key.is_empty() {
            push_type_lead_warning(
                &mut config.warnings,
                format!(
                    "capture.type_leads.{name} must normalize to a non-empty type name; skipping"
                ),
            );
            continue;
        }
        if clock_tags::is_reserved_type_name(name) {
            push_type_lead_warning(
                &mut config.warnings,
                format!(
                    "capture.type_leads.{name} is reserved for clock tags; skipping"
                ),
            );
            continue;
        }
        names_by_key
            .entry(key.clone())
            .or_default()
            .push(name.clone());
        entries.push((key, name.clone(), raw_text));
    }

    let mut duplicated = BTreeMap::new();
    for (key, names) in names_by_key {
        if names.len() > 1 {
            let labels = names
                .iter()
                .map(|name| format!("capture.type_leads.{name}"))
                .collect::<Vec<_>>()
                .join(" and ");
            push_type_lead_warning(
                &mut config.warnings,
                format!(
                    "{labels} normalize to the same type key {key:?}; skipping all"
                ),
            );
            duplicated.insert(key, ());
        }
    }

    for (key, name, raw_text) in entries {
        if duplicated.contains_key(&key) {
            continue;
        }
        if raw_text.is_null() {
            continue;
        }
        let serde_yaml::Value::String(raw_text) = raw_text else {
            push_type_lead_warning(
                &mut config.warnings,
                format!(
                    "capture.type_leads.{name} must be a string or null; skipping"
                ),
            );
            continue;
        };
        let lead = raw_text.trim();
        if lead.is_empty() {
            push_type_lead_warning(
                &mut config.warnings,
                format!(
                    "capture.type_leads.{name} must not be empty; skipping"
                ),
            );
            continue;
        }
        if lead.chars().count() > 64 {
            push_type_lead_warning(
                &mut config.warnings,
                format!(
                    "capture.type_leads.{name} must be at most 64 characters; skipping"
                ),
            );
            continue;
        }
        if lead
            .chars()
            .any(|ch| matches!(ch, '\n' | '\r' | '\u{2028}' | '\u{2029}'))
        {
            push_type_lead_warning(
                &mut config.warnings,
                format!("capture.type_leads.{name} must be one line; skipping"),
            );
            continue;
        }
        if lead.chars().any(char::is_control) {
            push_type_lead_warning(
                &mut config.warnings,
                format!(
                    "capture.type_leads.{name} must not contain control characters; skipping"
                ),
            );
            continue;
        }
        if !is_plain_title_lead(lead) {
            push_type_lead_warning(
                &mut config.warnings,
                format!(
                    "capture.type_leads.{name} must read as plain title text; skipping"
                ),
            );
            continue;
        }
        config.leads.insert(
            key,
            TypeLead {
                name,
                text: lead.to_string(),
            },
        );
    }
    config
}

fn is_plain_title_lead(text: &str) -> bool {
    [
        (format!("#bob-lead-probe {text}"), text.to_string()),
        (format!("#bob-lead-probe {text} x"), format!("{text} x")),
    ]
    .into_iter()
    .all(|(probe, expected_body)| {
        let parsed = parse_for_editor(&probe);
        let [item] = parsed.items.as_slice() else {
            return false;
        };
        item.mode == EditorMode::Task
            && parsed.diagnostics.is_empty()
            && item.diagnostics.is_empty()
            && item
                .spans
                .iter()
                .filter(|span| span.kind == SpanKind::TaskType)
                .count()
                == 1
            && item.body == expected_body
    })
}

fn push_type_lead_warning(warnings: &mut Vec<String>, warning: String) {
    if warnings.len() >= TYPE_LEAD_WARNING_LIMIT {
        return;
    }
    let bounded = warning
        .chars()
        .take(TYPE_LEAD_WARNING_CHARS)
        .collect::<String>();
    warnings.push(if warning.chars().count() > TYPE_LEAD_WARNING_CHARS {
        format!("{bounded}…")
    } else {
        bounded
    });
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

    let raw_setting = mapping
        .get(serde_yaml::Value::String("clipboard_history".to_string()))
        .cloned();
    let setting_configured =
        raw_setting.as_ref().is_some_and(|value| !value.is_null());
    let setting = match raw_setting {
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
            setting_configured,
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
    Ok(ClipboardHistoryConfig {
        setting,
        setting_configured,
        command,
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

    fn parse_leads(text: &str) -> TypeLeadsConfig {
        parse_type_leads_config(text)
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

    #[test]
    fn type_leads_missing_null_and_empty_maps_are_silent() {
        for text in [
            "properties: []\n",
            "capture: null\n",
            "capture: {}\n",
            "capture:\n  type_leads: null\n",
            "capture:\n  type_leads: {}\n",
            "capture:\n  type_leads: [FIX]\n",
        ] {
            let config = parse_leads(text);
            if text.contains("[FIX]") {
                assert!(config.leads.is_empty());
                assert_eq!(config.warnings.len(), 1);
                assert!(config.warnings[0].contains("capture.type_leads"));
            } else {
                assert!(config.leads.is_empty(), "{text}");
                assert!(config.warnings.is_empty(), "{text}");
            }
        }
        let malformed = parse_leads("capture:\n  type_leads: [\n");
        assert!(malformed.leads.is_empty());
        assert!(malformed.warnings.is_empty());
    }

    #[test]
    fn type_leads_validate_values_and_parser_probes_per_entry() {
        // NOTE: no backslash-newline continuations below: a `\` at the end
        // of a line strips the following line's indentation, which would
        // un-nest `type_leads` out of `capture:`.
        let config = parse_leads(&format!(
            r#"capture:
  type_leads:
    TRIM: '  Repair the hinge  '
    SKIP: null
    NUMBER: 7
    EMPTY: '   '
    MULTI: |
      two
      lines
    LONG: "{}"
    CONTROL: "two\twords"
    AT: '@home'
    TILDE: '~x'
    FIELD: 'x='
    TEMPLATE: '%t'
    CLOCK: '#now'
"#,
            "x".repeat(65)
        ));
        assert_eq!(config.leads.len(), 1);
        assert_eq!(config.leads["trim"].name, "TRIM");
        assert_eq!(config.leads["trim"].text, "Repair the hinge");
        assert_eq!(config.warnings.len(), 10);
        for name in [
            "NUMBER", "EMPTY", "MULTI", "LONG", "CONTROL", "AT", "TILDE",
            "FIELD", "TEMPLATE", "CLOCK",
        ] {
            assert!(
                config.warnings.iter().any(|warning| warning
                    .contains(&format!("capture.type_leads.{name}"))),
                "missing warning for {name}: {:?}",
                config.warnings
            );
        }
    }

    #[test]
    fn type_leads_reject_reserved_and_duplicate_normalized_keys() {
        let config = parse_leads(
            "capture:\n  type_leads:\n    OFF: Away\n    FIX: Repair\n    fix: Fix\n",
        );
        assert!(config.leads.is_empty());
        assert_eq!(config.warnings.len(), 2);
        assert!(config.warnings.iter().any(|warning| {
            warning.contains("capture.type_leads.OFF")
                && warning.contains("reserved")
        }));
        let duplicate = config
            .warnings
            .iter()
            .find(|warning| warning.contains("normalize to the same type key"))
            .expect("duplicate warning");
        assert!(duplicate.contains("capture.type_leads.FIX"));
        assert!(duplicate.contains("capture.type_leads.fix"));
    }

    #[test]
    fn type_leads_accept_multibyte_text_and_reject_non_string_keys() {
        let config = parse_leads(
            "capture:\n  type_leads:\n    REPAIR: 'Réparer'\n    42: Nope\n",
        );
        assert_eq!(config.leads["repair"].text, "Réparer");
        assert!(config.warnings.iter().any(|warning| {
            warning.contains("capture.type_leads.<42>")
                && warning.contains("string type name")
        }));
    }

    #[test]
    fn type_leads_are_independent_of_other_capture_and_vocabulary_settings() {
        let text = "types: {FIX: red}\ncapture:\n  type_leads:\n    FIX: Fix\n  max_saved_templates: 99\n  clipboard_history: spotlight\n";
        assert_eq!(parse_leads(text).leads["fix"].text, "Fix");
        assert!(parse(text).is_err());
        assert!(parse_history(text).is_err());

        let other_settings = "capture:\n  type_leads: []\n  max_saved_templates: 12\n  clipboard_history: alfred\n";
        assert_eq!(parse(other_settings).unwrap().max_saved_templates, 12);
        assert_eq!(
            parse_history(other_settings).unwrap().setting,
            ClipboardHistorySetting::Alfred
        );

        let invalid_leads = "types: {FIX: red}\ncapture:\n  type_leads: []\n";
        assert!(super::super::themes::parse_themes_config(
            invalid_leads,
            Path::new("test-config.yml")
        )
        .unwrap()
        .is_some());
    }
}
