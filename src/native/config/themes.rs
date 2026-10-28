use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

use super::{ConfigError, RawConfig};
use crate::native::{
    plan_budget::{normalize_component, split_components},
    pomodoro_homes::Hue,
};

/// One entry of the user-owned `themes:` vocabulary block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ConfiguredTheme {
    pub(crate) key: String,
    pub(crate) name: String,
    pub(crate) color: Option<Hue>,
}

/// The parsed `themes:` block in config order. `None` means no configured
/// themes (missing, null, or empty), which is exactly today's behavior.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ThemesConfig {
    pub(crate) entries: Vec<ConfiguredTheme>,
}

impl ThemesConfig {
    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.entries.len()
    }

    pub(crate) fn contains(&self, key: &str) -> bool {
        self.entries.iter().any(|entry| entry.key == key)
    }

    pub(crate) fn color(&self, key: &str) -> Option<Hue> {
        self.entries
            .iter()
            .find(|entry| entry.key == key)
            .and_then(|entry| entry.color)
    }
}

/// Everything the theme catalog needs from the config file, read once per
/// catalog load so `plan.exempt` and `themes:` share one file read.
#[derive(Debug, Clone)]
pub(crate) struct ThemeSources {
    pub(crate) exempt: BTreeSet<String>,
    pub(crate) inventory: BTreeSet<String>,
    pub(crate) themes: Option<ThemesConfig>,
    pub(crate) warning: Option<String>,
}

impl ThemeSources {
    fn defaults() -> Self {
        Self {
            exempt: super::PlanConfig::default()
                .exempt()
                .iter()
                .map(|value| normalize_component(value))
                .collect(),
            inventory: super::PlanConfig::default()
                .inventory_labels()
                .iter()
                .map(|value| normalize_component(value))
                .collect(),
            themes: None,
            warning: None,
        }
    }
}

/// Read the config file once and return the exempt set, the inventory set,
/// the optional themes vocabulary, and a warning when the `themes:` block
/// is present but invalid. An invalid block degrades to no configured
/// themes; it never fails the load.
pub(crate) fn load_theme_sources() -> ThemeSources {
    load_theme_sources_from(&super::config_path())
}

fn load_theme_sources_from(path: &Path) -> ThemeSources {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return ThemeSources::defaults();
        }
        Err(error) => {
            let mut sources = ThemeSources::defaults();
            sources.warning = Some(format!("read {}: {error}", path.display()));
            return sources;
        }
    };
    theme_sources_from_text(&text, path)
}

fn theme_sources_from_text(text: &str, path: &Path) -> ThemeSources {
    let mut sources = ThemeSources::defaults();
    match super::parse_plan_config(text, path) {
        Ok(plan) => {
            sources.exempt = plan
                .exempt()
                .iter()
                .map(|value| normalize_component(value))
                .collect();
            sources.inventory = plan
                .inventory_labels()
                .iter()
                .map(|value| normalize_component(value))
                .collect();
        }
        Err(error) => {
            // An unreadable `plan:` block keeps defaults; the plan surface
            // reports the error itself. Only keep a warning here when the
            // whole file fails to parse, since that also hides `themes:`.
            if text.trim().is_empty() {
                let _ = error;
            } else if serde_yaml::from_str::<serde_yaml::Value>(text).is_err() {
                sources.warning = Some(error.to_string());
                return sources;
            }
        }
    }
    match parse_themes_config(text, path) {
        Ok(themes) => {
            sources.themes = themes;
        }
        Err(error) => {
            sources.themes = None;
            sources.warning = Some(error.to_string());
        }
    }
    sources
}

/// Parse the `themes:` block. `Ok(None)` means no configured themes:
/// missing, null, or an empty mapping.
pub(crate) fn parse_themes_config(
    text: &str,
    path: &Path,
) -> Result<Option<ThemesConfig>, ConfigError> {
    let config: RawConfig = serde_yaml::from_str(text).map_err(|error| {
        ConfigError::Invalid(format!("parse {}: {error}", path.display()))
    })?;
    let Some(themes) = config.themes else {
        return Ok(None);
    };
    if themes.is_null() {
        return Ok(None);
    };
    let serde_yaml::Value::Mapping(mapping) = &themes else {
        return Err(ConfigError::Invalid(format!(
            "themes in {} must be a mapping",
            path.display()
        )));
    };
    if mapping.is_empty() {
        return Ok(None);
    }
    let path_display = path.display().to_string();
    let mut entries = Vec::new();
    let mut seen: BTreeMap<String, String> = BTreeMap::new();
    for (key_value, color_value) in mapping {
        let name = match key_value {
            serde_yaml::Value::String(name) => name.clone(),
            other => {
                return Err(ConfigError::Invalid(format!(
                    "themes.{} in {path_display} must be a string; quote theme names YAML reads as numbers or booleans",
                    render_key(other)
                )));
            }
        };
        if name.contains('+')
            || split_components(&name).len() != 1
            || normalize_component(&name).is_empty()
        {
            return Err(ConfigError::Invalid(format!(
                "themes.{name} in {path_display} must be a valid single theme (a theme never contains +)"
            )));
        }
        let key = normalize_component(&name);
        if let Some(first) = seen.get(&key) {
            return Err(ConfigError::Invalid(format!(
                "themes.{first} and themes.{name} in {path_display} normalize to the same theme {key:?}"
            )));
        }
        seen.insert(key.clone(), name.clone());
        let color = match color_value {
            serde_yaml::Value::Null => None,
            serde_yaml::Value::String(raw) => {
                let trimmed = raw.trim();
                if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("auto") {
                    None
                } else {
                    Some(Hue::parse(trimmed).ok_or_else(|| {
                        ConfigError::Invalid(format!(
                            "themes.{name} in {path_display} must be blue, orange, aqua, yellow, magenta, green, violet, red, auto, or empty; got {raw:?}"
                        ))
                    })?)
                }
            }
            other => {
                return Err(ConfigError::Invalid(format!(
                    "themes.{name} in {path_display} must be blue, orange, aqua, yellow, magenta, green, violet, red, auto, or empty; got {}",
                    render_scalar(other)
                )));
            }
        };
        entries.push(ConfiguredTheme { key, name, color });
    }
    Ok(Some(ThemesConfig { entries }))
}

fn render_key(value: &serde_yaml::Value) -> String {
    match value {
        serde_yaml::Value::String(text) => text.clone(),
        serde_yaml::Value::Number(number) => number.to_string(),
        serde_yaml::Value::Bool(flag) => flag.to_string(),
        serde_yaml::Value::Null => "null".to_string(),
        serde_yaml::Value::Sequence(_) => "sequence".to_string(),
        serde_yaml::Value::Mapping(_) => "mapping".to_string(),
        serde_yaml::Value::Tagged(tagged) => format!("tagged {}", tagged.tag),
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
        serde_yaml::Value::Tagged(tagged) => format!("tagged {}", tagged.tag),
    }
}

/// Display the config path with `~` when it sits under home, respecting
/// `BOB_CONFIG_FILE`/`XDG_CONFIG_HOME` through [`super::config_path`].
pub(crate) fn display_config_path() -> String {
    let path = super::config_path();
    let Some(text) = path.to_str() else {
        return path.display().to_string();
    };
    let home = super::super::env::home_dir();
    let Some(home_text) = home.to_str() else {
        return text.to_string();
    };
    if text == home_text {
        return "~".to_string();
    }
    if let Some(suffix) = text.strip_prefix(&format!("{home_text}/")) {
        return format!("~/{suffix}");
    }
    text.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn path() -> PathBuf {
        PathBuf::from("/home/user/.config/bob/config.yml")
    }

    #[test]
    fn missing_block_is_none() {
        let parsed = parse_themes_config("plan:\n  exempt: [PLAN]\n", &path())
            .expect("missing block");
        assert_eq!(parsed, None);
    }

    #[test]
    fn null_block_is_none() {
        let parsed =
            parse_themes_config("themes:\n", &path()).expect("null block");
        assert_eq!(parsed, None);
    }

    #[test]
    fn empty_mapping_is_none() {
        let parsed = parse_themes_config("themes: {}\n", &path())
            .expect("empty mapping");
        assert_eq!(parsed, None);
    }

    #[test]
    fn parses_slots_auto_and_null() {
        let parsed = parse_themes_config(
            "themes:\n  PLAN: blue\n  BUILD: Yellow\n  READ: auto\n  FIX:\n",
            &path(),
        )
        .expect("valid block")
        .expect("configured");
        assert_eq!(parsed.len(), 4);
        assert_eq!(parsed.color("plan"), Some(Hue::Blue));
        assert_eq!(parsed.color("build"), Some(Hue::Yellow));
        assert!(parsed.contains("read"));
        assert_eq!(parsed.color("read"), None);
        assert!(parsed.contains("fix"));
        assert_eq!(parsed.entries[0].name, "PLAN");
    }

    #[test]
    fn rejects_non_mapping() {
        let error = parse_themes_config("themes: [PLAN]\n", &path())
            .expect_err("sequence fails");
        assert!(
            error.message().contains(
                "themes in /home/user/.config/bob/config.yml must be a mapping"
            ),
            "unexpected: {}",
            error.message()
        );
    }

    #[test]
    fn rejects_non_string_key() {
        let error = parse_themes_config("themes:\n  7: blue\n", &path())
            .expect_err("number key fails");
        assert!(
            error.message().contains(
                "quote theme names YAML reads as numbers or booleans"
            ),
            "unexpected: {}",
            error.message()
        );
        assert!(
            error.message().contains("themes.7"),
            "unexpected: {}",
            error.message()
        );
    }

    #[test]
    fn rejects_blank_and_plus_names() {
        let blank = parse_themes_config("themes:\n  '   ': blue\n", &path())
            .expect_err("blank fails");
        assert!(
            blank.message().contains("themes.    in "),
            "unexpected: {}",
            blank.message()
        );
        let plus = parse_themes_config("themes:\n  'A+B': blue\n", &path())
            .expect_err("plus fails");
        assert!(
            plus.message().contains("a theme never contains +"),
            "unexpected: {}",
            plus.message()
        );
    }

    #[test]
    fn rejects_duplicate_keys() {
        let error = parse_themes_config(
            "themes:\n  BUILD: blue\n  build: red\n",
            &path(),
        )
        .expect_err("duplicate fails");
        assert!(
            error.message().contains("themes.BUILD and themes.build"),
            "unexpected: {}",
            error.message()
        );
    }

    #[test]
    fn rejects_bad_color() {
        let error = parse_themes_config("themes:\n  BUILD: teal\n", &path())
            .expect_err("bad color fails");
        assert!(
            error.message().contains("themes.BUILD"),
            "unexpected: {}",
            error.message()
        );
        assert!(
            error.message().contains("blue, orange, aqua"),
            "unexpected: {}",
            error.message()
        );
    }

    #[test]
    fn missing_file_has_no_warning() {
        let sources = load_theme_sources_from(Path::new(
            "/definitely/missing/bob-cli-test-config.yml",
        ));
        assert!(sources.themes.is_none());
        assert!(sources.warning.is_none());
        assert!(sources.exempt.contains("plan"));
    }
}
