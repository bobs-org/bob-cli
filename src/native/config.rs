use std::{
    env,
    ffi::OsString,
    path::{Path, PathBuf},
};

use serde::Deserialize;

use super::env as bob_env;

const CONFIG_RELATIVE_PATH: &str = "bob/config.yml";

pub(crate) fn config_path() -> PathBuf {
    resolve_config_path(
        env::var_os("BOB_CONFIG_FILE"),
        env::var_os("XDG_CONFIG_HOME"),
        bob_env::home_dir(),
    )
}

fn resolve_config_path(
    config_file: Option<OsString>,
    xdg_config_home: Option<OsString>,
    home: PathBuf,
) -> PathBuf {
    if let Some(config_file) = non_empty_os_string(config_file) {
        return expand_tilde_with_home(&PathBuf::from(config_file), &home);
    }
    if let Some(xdg_config_home) = non_empty_os_string(xdg_config_home) {
        return expand_tilde_with_home(&PathBuf::from(xdg_config_home), &home)
            .join(CONFIG_RELATIVE_PATH);
    }
    home.join(".config").join(CONFIG_RELATIVE_PATH)
}

fn non_empty_os_string(value: Option<OsString>) -> Option<OsString> {
    value.filter(|value| !value.is_empty())
}

/// Expand a leading `~` against `home` without touching process env, so
/// `resolve_config_path` stays pure and unit-testable.
fn expand_tilde_with_home(path: &Path, home: &Path) -> PathBuf {
    let Some(path_text) = path.to_str() else {
        return path.to_path_buf();
    };

    if path_text == "~" {
        return home.to_path_buf();
    }

    if let Some(suffix) = path_text.strip_prefix("~/") {
        return home.join(suffix);
    }

    path.to_path_buf()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PriorityProperty {
    name: String,
    levels: Vec<PriorityLevel>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PriorityLevel {
    label: String,
    value: String,
    min_days: u64,
    max_days: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ConfigError {
    Read(String),
    Invalid(String),
}

impl ConfigError {
    #[cfg(test)]
    fn message(&self) -> &str {
        match self {
            Self::Read(message) | Self::Invalid(message) => message,
        }
    }
}

impl PriorityProperty {
    pub(crate) fn name(&self) -> &str {
        &self.name
    }

    pub(crate) fn level(&self, number: u64) -> Option<&PriorityLevel> {
        let index = usize::try_from(number).ok()?.checked_sub(1)?;
        self.levels.get(index)
    }

    pub(crate) fn level_count(&self) -> usize {
        self.levels.len()
    }

    pub(crate) fn labels(&self) -> String {
        self.levels
            .iter()
            .map(|level| level.label.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    }
}

impl PriorityLevel {
    pub(crate) fn label(&self) -> &str {
        &self.label
    }

    pub(crate) fn value(&self) -> &str {
        &self.value
    }

    pub(crate) fn min_days(&self) -> u64 {
        self.min_days
    }

    pub(crate) fn max_days(&self) -> u64 {
        self.max_days
    }

    /// Roll a day offset inclusively within `[min_days, max_days]` from `seed`.
    pub(crate) fn roll_offset(&self, seed: u64) -> u64 {
        let span = self.max_days - self.min_days + 1;
        self.min_days + mix64(seed) % span
    }
}

/// The splitmix64 finalizer, used only to spread a seed across a small span;
/// not intended to be cryptographically secure.
fn mix64(value: u64) -> u64 {
    let mut z = value.wrapping_add(0x9E3779B97F4A7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D049BB133111EB);
    z ^ (z >> 31)
}

pub(crate) fn roll_seed() -> u64 {
    if let Some(seed) = env::var("BOB_PRIORITY_ROLL_SEED")
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
    {
        return seed;
    }

    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos() as u64)
        .unwrap_or(0);
    nanos ^ mix64(u64::from(std::process::id()))
}

pub(crate) fn load_priority_property(
    path: &Path,
) -> Result<PriorityProperty, ConfigError> {
    let text = std::fs::read_to_string(path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            ConfigError::Read(format!(
                "p:<N> needs {}; run 'chezmoi apply ~/.config/bob/config.yml'",
                path.display()
            ))
        } else {
            ConfigError::Read(format!("read {}: {error}", path.display()))
        }
    })?;
    parse_priority_property(&text, path)
}

#[derive(Debug, Deserialize)]
struct RawConfig {
    #[serde(default)]
    properties: Vec<RawProperty>,
}

#[derive(Debug, Deserialize)]
struct RawProperty {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    values: Option<serde_yaml::Value>,
    #[serde(default)]
    schedules: Option<String>,
    #[serde(default)]
    levels: Option<Vec<RawLevel>>,
}

#[derive(Debug, Deserialize)]
struct RawLevel {
    #[serde(default)]
    label: Option<String>,
    #[serde(default)]
    value: Option<serde_yaml::Value>,
    #[serde(default)]
    min_days: Option<i64>,
    #[serde(default)]
    max_days: Option<i64>,
}

fn parse_priority_property(
    text: &str,
    path: &Path,
) -> Result<PriorityProperty, ConfigError> {
    let config: RawConfig = serde_yaml::from_str(text).map_err(|error| {
        ConfigError::Invalid(format!("parse {}: {error}", path.display()))
    })?;

    let path_display = path.display();
    let raw_property = config
        .properties
        .into_iter()
        .find(|property| {
            property.values.as_ref().and_then(|values| values.as_str())
                == Some("priority")
        })
        .ok_or_else(|| {
            ConfigError::Invalid(format!(
                "no priority property is configured in {path_display}"
            ))
        })?;

    let name = raw_property.name.unwrap_or_default();

    match raw_property.schedules.as_deref() {
        Some("scheduled") => {}
        Some(other) => {
            return Err(ConfigError::Invalid(format!(
                "priority property \"{name}\" in {path_display} must schedule \"scheduled\"; it schedules \"{other}\""
            )));
        }
        None => {
            return Err(ConfigError::Invalid(format!(
                "priority property \"{name}\" in {path_display} must schedule \"scheduled\""
            )));
        }
    }

    let raw_levels = raw_property.levels.unwrap_or_default();
    if raw_levels.is_empty() {
        return Err(ConfigError::Invalid(format!(
            "priority property \"{name}\" in {path_display} configures no levels"
        )));
    }

    let levels = raw_levels
        .into_iter()
        .enumerate()
        .map(|(index, level)| {
            parse_priority_level(level, index, &name, &path_display.to_string())
        })
        .collect::<Result<Vec<_>, _>>()?;

    Ok(PriorityProperty { name, levels })
}

fn parse_priority_level(
    level: RawLevel,
    index: usize,
    property_name: &str,
    path_display: &str,
) -> Result<PriorityLevel, ConfigError> {
    let ordinal = index + 1;
    let label = level
        .label
        .as_deref()
        .map(str::trim)
        .filter(|label| !label.is_empty())
        .ok_or_else(|| {
            ConfigError::Invalid(format!(
                "priority property \"{property_name}\" in {path_display} level #{ordinal} must define a non-empty label"
            ))
        })?
        .to_string();

    let level_ref = format!("\"{label}\"");

    let missing_value_error = || {
        ConfigError::Invalid(format!(
            "priority property \"{property_name}\" in {path_display} level {level_ref} must define a non-empty value"
        ))
    };
    let raw_value = level
        .value
        .as_ref()
        .and_then(scalar_to_string)
        .ok_or_else(missing_value_error)?;
    let value = raw_value.trim();
    if value.is_empty() {
        return Err(missing_value_error());
    }
    if value.contains('[')
        || value.contains(']')
        || value.contains("::")
        || value.contains('\n')
    {
        return Err(ConfigError::Invalid(format!(
            "priority property \"{property_name}\" in {path_display} level {level_ref} value cannot contain \"[\", \"]\", \"::\", or a newline"
        )));
    }
    let value = value.to_string();

    let min_days = level.min_days.ok_or_else(|| {
        ConfigError::Invalid(format!(
            "priority property \"{property_name}\" in {path_display} level {level_ref} must define a non-negative min_days"
        ))
    })?;
    if min_days < 0 {
        return Err(ConfigError::Invalid(format!(
            "priority property \"{property_name}\" in {path_display} level {level_ref} min_days must be non-negative"
        )));
    }
    let max_days = level.max_days.ok_or_else(|| {
        ConfigError::Invalid(format!(
            "priority property \"{property_name}\" in {path_display} level {level_ref} must define a non-negative max_days"
        ))
    })?;
    if max_days < 0 {
        return Err(ConfigError::Invalid(format!(
            "priority property \"{property_name}\" in {path_display} level {level_ref} max_days must be non-negative"
        )));
    }
    if min_days > max_days {
        return Err(ConfigError::Invalid(format!(
            "priority property \"{property_name}\" in {path_display} level {level_ref} min_days cannot exceed max_days"
        )));
    }

    Ok(PriorityLevel {
        label,
        value,
        min_days: min_days as u64,
        max_days: max_days as u64,
    })
}

fn scalar_to_string(value: &serde_yaml::Value) -> Option<String> {
    match value {
        serde_yaml::Value::String(value) => Some(value.clone()),
        serde_yaml::Value::Number(value) => Some(value.to_string()),
        serde_yaml::Value::Bool(value) => Some(value.to_string()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DEPLOYED_CONFIG: &str = r#"
properties:
  - name: scheduled
    values: date
  - name: dependsOn
    values: local_task_id
  - name: priority
    values: priority
    schedules: scheduled
    levels:
      - label: P1
        value: high
        min_days: 2
        max_days: 7
      - label: P2
        value: medium
        min_days: 8
        max_days: 30
      - label: P3
        value: low
        min_days: 31
        max_days: 90
      - label: P4
        value: lowest
        min_days: 91
        max_days: 365
"#;

    #[test]
    fn parses_deployed_config() {
        let property =
            parse_priority_property(DEPLOYED_CONFIG, Path::new("/config.yml"))
                .expect("valid config");
        assert_eq!(property.name(), "priority");
        assert_eq!(property.level_count(), 4);
        assert_eq!(property.labels(), "P1, P2, P3, P4");

        let expected = [
            ("P1", "high", 2, 7),
            ("P2", "medium", 8, 30),
            ("P3", "low", 31, 90),
            ("P4", "lowest", 91, 365),
        ];
        for (number, (label, value, min_days, max_days)) in
            (1u64..).zip(expected)
        {
            let level = property.level(number).expect("configured level");
            assert_eq!(level.label(), label);
            assert_eq!(level.value(), value);
            assert_eq!(level.min_days, min_days);
            assert_eq!(level.max_days, max_days);
        }
        assert!(property.level(5).is_none());
        assert!(property.level(0).is_none());
    }

    fn parse(text: &str) -> Result<PriorityProperty, ConfigError> {
        parse_priority_property(text, Path::new("/config.yml"))
    }

    #[test]
    fn rejects_missing_priority_property() {
        let error = parse(
            r#"
properties:
  - name: scheduled
    values: date
"#,
        )
        .expect_err("no priority property");
        assert_eq!(
            error,
            ConfigError::Invalid(
                "no priority property is configured in /config.yml".to_string()
            )
        );
    }

    #[test]
    fn rejects_wrong_schedules_target() {
        let error = parse(
            r#"
properties:
  - name: priority
    values: priority
    schedules: due
    levels:
      - label: P1
        value: high
        min_days: 1
        max_days: 1
"#,
        )
        .expect_err("wrong schedules");
        assert_eq!(
            error,
            ConfigError::Invalid(
                "priority property \"priority\" in /config.yml must schedule \"scheduled\"; it schedules \"due\"".to_string()
            )
        );
    }

    #[test]
    fn rejects_missing_schedules() {
        let error = parse(
            r#"
properties:
  - name: priority
    values: priority
    levels:
      - label: P1
        value: high
        min_days: 1
        max_days: 1
"#,
        )
        .expect_err("missing schedules");
        assert_eq!(
            error,
            ConfigError::Invalid(
                "priority property \"priority\" in /config.yml must schedule \"scheduled\"".to_string()
            )
        );
    }

    #[test]
    fn rejects_empty_levels() {
        let error = parse(
            r#"
properties:
  - name: priority
    values: priority
    schedules: scheduled
    levels: []
"#,
        )
        .expect_err("empty levels");
        assert_eq!(
            error,
            ConfigError::Invalid(
                "priority property \"priority\" in /config.yml configures no levels".to_string()
            )
        );
    }

    #[test]
    fn rejects_blank_label() {
        let error = parse(
            r#"
properties:
  - name: priority
    values: priority
    schedules: scheduled
    levels:
      - label: "  "
        value: high
        min_days: 1
        max_days: 1
"#,
        )
        .expect_err("blank label");
        assert!(error.message().contains("must define a non-empty label"));
    }

    #[test]
    fn rejects_missing_value() {
        let error = parse(
            r#"
properties:
  - name: priority
    values: priority
    schedules: scheduled
    levels:
      - label: P1
        min_days: 1
        max_days: 1
"#,
        )
        .expect_err("missing value");
        assert!(error.message().contains("must define a non-empty value"));
    }

    #[test]
    fn rejects_blank_value() {
        let error = parse(
            r#"
properties:
  - name: priority
    values: priority
    schedules: scheduled
    levels:
      - label: P1
        value: "  "
        min_days: 1
        max_days: 1
"#,
        )
        .expect_err("blank value");
        assert!(error.message().contains("must define a non-empty value"));
    }

    #[test]
    fn rejects_value_containing_field_syntax() {
        let error = parse(
            r#"
properties:
  - name: priority
    values: priority
    schedules: scheduled
    levels:
      - label: P1
        value: "high::extra"
        min_days: 1
        max_days: 1
"#,
        )
        .expect_err("value with field syntax");
        assert!(error
            .message()
            .contains("cannot contain \"[\", \"]\", \"::\""));
    }

    #[test]
    fn rejects_negative_min_days() {
        let error = parse(
            r#"
properties:
  - name: priority
    values: priority
    schedules: scheduled
    levels:
      - label: P1
        value: high
        min_days: -1
        max_days: 1
"#,
        )
        .expect_err("negative min_days");
        assert!(error.message().contains("min_days must be non-negative"));
    }

    #[test]
    fn rejects_min_greater_than_max() {
        let error = parse(
            r#"
properties:
  - name: priority
    values: priority
    schedules: scheduled
    levels:
      - label: P1
        value: high
        min_days: 5
        max_days: 1
"#,
        )
        .expect_err("min greater than max");
        assert!(error.message().contains("min_days cannot exceed max_days"));
    }

    #[test]
    fn rejects_non_integer_min_days() {
        let error = parse(
            r#"
properties:
  - name: priority
    values: priority
    schedules: scheduled
    levels:
      - label: P1
        value: high
        min_days: "soon"
        max_days: 1
"#,
        )
        .expect_err("non-integer min_days");
        assert!(matches!(error, ConfigError::Invalid(_)));
    }

    #[test]
    fn tolerates_unusual_sibling_properties() {
        let property = parse(
            r#"
properties:
  - name: tags
    values:
      - work
      - home
  - values: date
  - name: priority
    values: priority
    schedules: scheduled
    levels:
      - label: P1
        value: high
        min_days: 1
        max_days: 1
"#,
        )
        .expect("resolves priority property despite unusual siblings");
        assert_eq!(property.name(), "priority");
    }

    #[test]
    fn resolve_config_path_prefers_bob_config_file() {
        let path = resolve_config_path(
            Some(OsString::from("/explicit/config.yml")),
            Some(OsString::from("/xdg")),
            PathBuf::from("/home/user"),
        );
        assert_eq!(path, PathBuf::from("/explicit/config.yml"));
    }

    #[test]
    fn resolve_config_path_falls_back_to_xdg_config_home() {
        let path = resolve_config_path(
            None,
            Some(OsString::from("/xdg")),
            PathBuf::from("/home/user"),
        );
        assert_eq!(path, PathBuf::from("/xdg/bob/config.yml"));
    }

    #[test]
    fn resolve_config_path_expands_tilde_in_xdg_config_home() {
        let path = resolve_config_path(
            None,
            Some(OsString::from("~/xdg")),
            PathBuf::from("/home/user"),
        );
        assert_eq!(path, PathBuf::from("/home/user/xdg/bob/config.yml"));
    }

    #[test]
    fn resolve_config_path_falls_back_to_home_dot_config() {
        let path = resolve_config_path(None, None, PathBuf::from("/home/user"));
        assert_eq!(path, PathBuf::from("/home/user/.config/bob/config.yml"));
    }

    #[test]
    fn resolve_config_path_ignores_empty_env_values() {
        let path = resolve_config_path(
            Some(OsString::new()),
            Some(OsString::new()),
            PathBuf::from("/home/user"),
        );
        assert_eq!(path, PathBuf::from("/home/user/.config/bob/config.yml"));
    }

    #[test]
    fn roll_offset_stays_within_bounds_for_many_seeds() {
        let levels = [(2u64, 7u64), (8, 30), (31, 90), (91, 365)];
        for (min_days, max_days) in levels {
            let level = PriorityLevel {
                label: "P".to_string(),
                value: "v".to_string(),
                min_days,
                max_days,
            };
            let mut seen = std::collections::HashSet::new();
            for seed in 0..10_000u64 {
                let offset = level.roll_offset(seed);
                assert!(
                    offset >= min_days && offset <= max_days,
                    "offset {offset} out of [{min_days}, {max_days}] for seed {seed}"
                );
                seen.insert(offset);
            }
            if max_days - min_days < 6 {
                assert_eq!(
                    seen.len() as u64,
                    max_days - min_days + 1,
                    "expected every offset in a small span to appear"
                );
            } else {
                assert!(seen.contains(&min_days) || seen.contains(&max_days));
            }
        }
    }

    #[test]
    fn roll_offset_returns_fixed_value_when_min_equals_max() {
        let level = PriorityLevel {
            label: "P".to_string(),
            value: "v".to_string(),
            min_days: 42,
            max_days: 42,
        };
        for seed in 0..1000u64 {
            assert_eq!(level.roll_offset(seed), 42);
        }
    }

    #[test]
    fn roll_offset_p4_window_hits_both_extremes() {
        let level = PriorityLevel {
            label: "P4".to_string(),
            value: "lowest".to_string(),
            min_days: 91,
            max_days: 365,
        };
        let mut hit_min = false;
        let mut hit_max = false;
        for seed in 0..50_000u64 {
            let offset = level.roll_offset(seed);
            if offset == 91 {
                hit_min = true;
            }
            if offset == 365 {
                hit_max = true;
            }
            if hit_min && hit_max {
                break;
            }
        }
        assert!(hit_min, "never rolled the minimum offset");
        assert!(hit_max, "never rolled the maximum offset");
    }
}
