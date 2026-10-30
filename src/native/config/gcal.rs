use std::collections::BTreeMap;
use std::path::Path;

use super::ConfigError;

/// Parsed `gcal:` settings before command-level defaults are applied.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct GcalSettings {
    pub(crate) account: Option<String>,
    pub(crate) writer: Option<String>,
    pub(crate) root: Option<String>,
    pub(crate) calendars: Option<Vec<GcalCalendarSettings>>,
    pub(crate) colors: BTreeMap<String, String>,
    pub(crate) tentative: Option<GcalTentative>,
    pub(crate) timeout_secs: Option<u64>,
}

/// One configured calendar in `gcal.calendars`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GcalCalendarSettings {
    pub(crate) id: String,
    pub(crate) root: Option<String>,
}

/// How Maybe RSVPs are classified.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum GcalTentative {
    Context,
    Task,
}

pub(crate) fn load_gcal_config(
    path: &Path,
) -> Result<GcalSettings, ConfigError> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(GcalSettings::default());
        }
        Err(error) => {
            return Err(ConfigError::Read(format!(
                "read {}: {error}",
                path.display()
            )));
        }
    };
    parse_gcal_config(&text, path)
}

pub(crate) fn parse_gcal_config(
    text: &str,
    path: &Path,
) -> Result<GcalSettings, ConfigError> {
    let config: super::RawConfig =
        serde_yaml::from_str(text).map_err(|error| {
            ConfigError::Invalid(format!("parse {}: {error}", path.display()))
        })?;
    parse_gcal_value(config.gcal, path)
}

fn parse_gcal_value(
    gcal: Option<serde_yaml::Value>,
    path: &Path,
) -> Result<GcalSettings, ConfigError> {
    let path_display = path.display();
    let Some(gcal) = gcal else {
        return Ok(GcalSettings::default());
    };
    if gcal.is_null() {
        return Ok(GcalSettings::default());
    }
    let serde_yaml::Value::Mapping(ref mapping) = gcal else {
        return Err(ConfigError::Invalid(format!(
            "gcal in {path_display} must be a mapping; got {}",
            render_scalar(&gcal)
        )));
    };

    let get = |name: &str| {
        let key = serde_yaml::Value::String(name.to_string());
        mapping.get(&key).cloned()
    };

    Ok(GcalSettings {
        account: optional_string("account", get("account"), path)?,
        writer: optional_string("writer", get("writer"), path)?,
        root: optional_string("root", get("root"), path)?,
        calendars: parse_calendars(get("calendars"), path)?,
        colors: parse_colors(get("colors"), path)?,
        tentative: parse_tentative(get("tentative"), path)?,
        timeout_secs: parse_timeout(get("timeout_secs"), path)?,
    })
}

fn optional_string(
    name: &str,
    value: Option<serde_yaml::Value>,
    path: &Path,
) -> Result<Option<String>, ConfigError> {
    let Some(value) = value else {
        return Ok(None);
    };
    if value.is_null() {
        return Ok(None);
    }
    match value {
        serde_yaml::Value::String(text) => {
            let trimmed = text.trim();
            if trimmed.is_empty() {
                return Ok(None);
            }
            Ok(Some(trimmed.to_string()))
        }
        other => Err(ConfigError::Invalid(format!(
            "gcal.{name} in {} must be a string; got {}",
            path.display(),
            render_scalar(&other)
        ))),
    }
}

fn parse_timeout(
    value: Option<serde_yaml::Value>,
    path: &Path,
) -> Result<Option<u64>, ConfigError> {
    let Some(value) = value else {
        return Ok(None);
    };
    if value.is_null() {
        return Ok(None);
    }
    let number = match &value {
        serde_yaml::Value::Number(number) => {
            if let Some(int) = number.as_i64() {
                int
            } else if let Some(uint) = number.as_u64() {
                i64::try_from(uint).map_err(|_| {
                    ConfigError::Invalid(format!(
                        "gcal.timeout_secs in {} must be an integer > 0; got {value:?}",
                        path.display()
                    ))
                })?
            } else {
                return Err(ConfigError::Invalid(format!(
                    "gcal.timeout_secs in {} must be an integer > 0; got {value:?}",
                    path.display()
                )));
            }
        }
        other => {
            return Err(ConfigError::Invalid(format!(
                "gcal.timeout_secs in {} must be an integer > 0; got {}",
                path.display(),
                render_scalar(other)
            )));
        }
    };
    if number <= 0 {
        return Err(ConfigError::Invalid(format!(
            "gcal.timeout_secs in {} must be an integer > 0; got {number}",
            path.display()
        )));
    }
    u64::try_from(number).map(Some).map_err(|_| {
        ConfigError::Invalid(format!(
            "gcal.timeout_secs in {} must be an integer > 0; got {number}",
            path.display()
        ))
    })
}

fn parse_tentative(
    value: Option<serde_yaml::Value>,
    path: &Path,
) -> Result<Option<GcalTentative>, ConfigError> {
    let Some(value) = value else {
        return Ok(None);
    };
    if value.is_null() {
        return Ok(None);
    }
    let serde_yaml::Value::String(text) = value else {
        return Err(ConfigError::Invalid(format!(
            "gcal.tentative in {} must be context or task; got {}",
            path.display(),
            render_scalar(&value)
        )));
    };
    match text.trim() {
        "context" => Ok(Some(GcalTentative::Context)),
        "task" => Ok(Some(GcalTentative::Task)),
        other => Err(ConfigError::Invalid(format!(
            "gcal.tentative in {} must be context or task; got {other:?}",
            path.display()
        ))),
    }
}

fn parse_colors(
    value: Option<serde_yaml::Value>,
    path: &Path,
) -> Result<BTreeMap<String, String>, ConfigError> {
    let Some(value) = value else {
        return Ok(BTreeMap::new());
    };
    if value.is_null() {
        return Ok(BTreeMap::new());
    }
    let serde_yaml::Value::Mapping(mapping) = value else {
        return Err(ConfigError::Invalid(format!(
            "gcal.colors in {} must be a mapping of colour names to roots; got {}",
            path.display(),
            render_scalar(&value)
        )));
    };
    let mut colors = BTreeMap::new();
    let mut color_ids = std::collections::BTreeSet::new();
    for (key, mapped) in mapping {
        let name = match key {
            serde_yaml::Value::String(name) if !name.trim().is_empty() => {
                name.trim().to_string()
            }
            serde_yaml::Value::Number(number) => number
                .as_u64()
                .filter(|number| (1..=11).contains(number))
                .map(|number| number.to_string())
                .ok_or_else(|| {
                    ConfigError::Invalid(format!(
                        "gcal.colors in {} keys must be Google event colour names or ids (1–11)",
                        path.display()
                    ))
                })?,
            other => {
                return Err(ConfigError::Invalid(format!(
                    "gcal.colors in {} keys must be strings; got {}",
                    path.display(),
                    render_scalar(&other)
                )));
            }
        };
        let Some(color_id) = color_id_for_key(&name) else {
            return Err(ConfigError::Invalid(format!(
                "gcal.colors.{name} in {} is not a Google event colour name or id (1–11)",
                path.display()
            )));
        };
        if !color_ids.insert(color_id) {
            return Err(ConfigError::Invalid(format!(
                "gcal.colors in {} maps Google event colour {color_id} more than once",
                path.display()
            )));
        }
        let root = match mapped {
            serde_yaml::Value::String(root) if !root.trim().is_empty() => {
                root.trim().to_string()
            }
            other => {
                return Err(ConfigError::Invalid(format!(
                    "gcal.colors.{name} in {} must be a string; got {}",
                    path.display(),
                    render_scalar(&other)
                )));
            }
        };
        colors.insert(name, root);
    }
    Ok(colors)
}

/// Return Google's numeric event-colour id for a configured name or id.
pub(crate) fn color_id_for_key(key: &str) -> Option<&'static str> {
    const COLORS: &[(&str, &str)] = &[
        ("lavender", "1"),
        ("sage", "2"),
        ("grape", "3"),
        ("flamingo", "4"),
        ("banana", "5"),
        ("tangerine", "6"),
        ("peacock", "7"),
        ("graphite", "8"),
        ("blueberry", "9"),
        ("basil", "10"),
        ("tomato", "11"),
    ];
    let key = key.trim().to_ascii_lowercase();
    COLORS
        .iter()
        .find(|(name, id)| key == *name || key == *id)
        .map(|(_, id)| *id)
}

pub(crate) fn color_name_for_id(id: &str) -> Option<&'static str> {
    const COLORS: &[(&str, &str)] = &[
        ("lavender", "1"),
        ("sage", "2"),
        ("grape", "3"),
        ("flamingo", "4"),
        ("banana", "5"),
        ("tangerine", "6"),
        ("peacock", "7"),
        ("graphite", "8"),
        ("blueberry", "9"),
        ("basil", "10"),
        ("tomato", "11"),
    ];
    COLORS
        .iter()
        .find(|(_, color_id)| *color_id == id)
        .map(|(name, _)| *name)
}

fn parse_calendars(
    value: Option<serde_yaml::Value>,
    path: &Path,
) -> Result<Option<Vec<GcalCalendarSettings>>, ConfigError> {
    let Some(value) = value else {
        return Ok(None);
    };
    if value.is_null() {
        return Ok(None);
    }
    let serde_yaml::Value::Sequence(items) = value else {
        return Err(ConfigError::Invalid(format!(
            "gcal.calendars in {} must be a sequence; got {}",
            path.display(),
            render_scalar(&value)
        )));
    };
    let mut calendars = Vec::with_capacity(items.len());
    for (index, item) in items.into_iter().enumerate() {
        calendars.push(parse_calendar_item(item, index, path)?);
    }
    Ok(Some(calendars))
}

fn parse_calendar_item(
    item: serde_yaml::Value,
    index: usize,
    path: &Path,
) -> Result<GcalCalendarSettings, ConfigError> {
    match item {
        serde_yaml::Value::String(id) => {
            let id = id.trim();
            if id.is_empty() {
                return Err(ConfigError::Invalid(format!(
                    "gcal.calendars[{index}] in {} must be a nonempty calendar id",
                    path.display()
                )));
            }
            Ok(GcalCalendarSettings {
                id: id.to_string(),
                root: None,
            })
        }
        serde_yaml::Value::Mapping(mapping) => {
            let get = |name: &str| {
                let key = serde_yaml::Value::String(name.to_string());
                mapping.get(&key).cloned()
            };
            let id = match get("id") {
                None | Some(serde_yaml::Value::Null) => {
                    return Err(ConfigError::Invalid(format!(
                        "gcal.calendars[{index}].id in {} is required",
                        path.display()
                    )));
                }
                Some(serde_yaml::Value::String(id)) => {
                    let id = id.trim();
                    if id.is_empty() {
                        return Err(ConfigError::Invalid(format!(
                            "gcal.calendars[{index}].id in {} must be nonempty",
                            path.display()
                        )));
                    }
                    id.to_string()
                }
                Some(other) => {
                    return Err(ConfigError::Invalid(format!(
                        "gcal.calendars[{index}].id in {} must be a string; got {}",
                        path.display(),
                        render_scalar(&other)
                    )));
                }
            };
            let root = match get("root") {
                None | Some(serde_yaml::Value::Null) => None,
                Some(serde_yaml::Value::String(root)) => {
                    let root = root.trim();
                    if root.is_empty() {
                        None
                    } else {
                        Some(root.to_string())
                    }
                }
                Some(other) => {
                    return Err(ConfigError::Invalid(format!(
                        "gcal.calendars[{index}].root in {} must be a string; got {}",
                        path.display(),
                        render_scalar(&other)
                    )));
                }
            };
            Ok(GcalCalendarSettings { id, root })
        }
        other => Err(ConfigError::Invalid(format!(
            "gcal.calendars[{index}] in {} must be a calendar id or a mapping; got {}",
            path.display(),
            render_scalar(&other)
        ))),
    }
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

    fn parse(text: &str) -> Result<GcalSettings, ConfigError> {
        parse_gcal_config(text, Path::new("/config.yml"))
    }

    #[test]
    fn missing_and_null_use_defaults() {
        assert_eq!(parse("").unwrap(), GcalSettings::default());
        assert_eq!(parse("gcal:\n").unwrap(), GcalSettings::default());
        assert_eq!(parse("gcal: null\n").unwrap(), GcalSettings::default());
    }

    #[test]
    fn unknown_keys_are_ignored() {
        let settings = parse("gcal:\n  extra: 1\n  account: you@example.com\n")
            .expect("unknown keys ignored");
        assert_eq!(settings.account.as_deref(), Some("you@example.com"));
    }

    #[test]
    fn parses_full_block() {
        let settings = parse(
            "gcal:\n  account: you@example.com\n  writer: kellys-macbook-pro\n  root: meet\n  calendars:\n    - id: primary\n    - id: work\n      root: job\n  colors:\n    tomato: job\n  tentative: task\n  timeout_secs: 90\n",
        )
        .expect("full block");
        assert_eq!(settings.account.as_deref(), Some("you@example.com"));
        assert_eq!(settings.writer.as_deref(), Some("kellys-macbook-pro"));
        assert_eq!(settings.root.as_deref(), Some("meet"));
        assert_eq!(settings.calendars.as_ref().map(Vec::len), Some(2));
        assert_eq!(
            settings.colors.get("tomato").map(String::as_str),
            Some("job")
        );
        assert_eq!(settings.tentative, Some(GcalTentative::Task));
        assert_eq!(settings.timeout_secs, Some(90));
    }

    #[test]
    fn calendars_accept_string_ids() {
        let settings =
            parse("gcal:\n  calendars: [primary, work]\n").expect("string ids");
        let calendars = settings.calendars.expect("calendars");
        assert_eq!(calendars[0].id, "primary");
        assert_eq!(calendars[1].id, "work");
    }

    #[test]
    fn rejects_wrong_types() {
        for text in [
            "gcal: yes\n",
            "gcal:\n  account: 1\n",
            "gcal:\n  timeout_secs: soon\n",
            "gcal:\n  timeout_secs: 0\n",
            "gcal:\n  tentative: maybe\n",
            "gcal:\n  colors: [tomato]\n",
            "gcal:\n  calendars: primary\n",
            "gcal:\n  calendars:\n    - root: job\n",
        ] {
            let error = parse(text).expect_err(text);
            assert!(
                error.message().contains("gcal"),
                "{text} names gcal: {error}"
            );
        }
    }

    #[test]
    fn validates_google_event_colour_names_and_ids() {
        let settings =
            parse("gcal:\n  colors:\n    tomato: job\n    7: body\n")
                .expect("valid colour names and ids");
        assert_eq!(
            settings.colors.get("tomato").map(String::as_str),
            Some("job")
        );
        assert_eq!(settings.colors.get("7").map(String::as_str), Some("body"));
        assert_eq!(color_id_for_key("tomato"), Some("11"));
        assert_eq!(color_id_for_key("7"), Some("7"));
        assert_eq!(color_name_for_id("11"), Some("tomato"));

        let error = parse("gcal:\n  colors:\n    teal: body\n")
            .expect_err("unsupported colour");
        assert!(error.message().contains("gcal.colors.teal"));
        assert!(error.message().contains("name or id"));
    }
}
