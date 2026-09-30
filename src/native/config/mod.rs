use std::{
    env,
    ffi::OsString,
    path::{Path, PathBuf},
};

use serde::Deserialize;

use super::env as bob_env;

pub(crate) mod freshness;
pub(crate) mod plan;

pub(crate) use freshness::{load_freshness_config, FreshnessConfig};
pub(crate) use plan::{load_plan_config, PlanConfig};

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

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct HighlightsConfig {
    pre_scan_hook: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct GkeepSettings {
    pub(crate) email: Option<String>,
    pub(crate) token_command: Option<String>,
    pub(crate) token_store_command: Option<String>,
    pub(crate) device_id: Option<String>,
    pub(crate) target: Option<String>,
    pub(crate) timeout_secs: Option<u64>,
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

    pub(crate) fn levels(&self) -> &[PriorityLevel] {
        &self.levels
    }

    /// Exact match after trimming, against the configured level `value`
    /// (what lands in notes).
    pub(crate) fn level_for_value(
        &self,
        value: &str,
    ) -> Option<&PriorityLevel> {
        let value = value.trim();
        self.levels.iter().find(|level| level.value == value)
    }

    /// ASCII case-insensitive match after trimming, against the configured
    /// level label (what `--level` accepts).
    pub(crate) fn level_by_label(&self, label: &str) -> Option<&PriorityLevel> {
        let label = label.trim();
        self.levels
            .iter()
            .find(|level| level.label.eq_ignore_ascii_case(label))
    }
}

impl HighlightsConfig {
    pub(crate) fn pre_scan_hook(&self) -> Option<&str> {
        self.pre_scan_hook.as_deref()
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
pub(crate) fn mix64(value: u64) -> u64 {
    let mut z = value.wrapping_add(0x9E3779B97F4A7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D049BB133111EB);
    z ^ (z >> 31)
}

/// Mix a base seed with stable per-task identity parts. The parts are
/// hashed with FNV-1a over bytes (never `std::hash::DefaultHasher`, whose
/// output is randomized per process), joined with a NUL separator so
/// `("ab", "c")` and `("a", "bc")` cannot collide. The base seed is
/// combined with xor so a zero base still spreads across the hash.
pub(crate) fn derive_seed(base: u64, parts: &[&str]) -> u64 {
    let mut hash = 0xcbf29ce484222325u64;
    for (index, part) in parts.iter().enumerate() {
        if index > 0 {
            hash = fnv_feed(hash, 0);
        }
        for byte in part.as_bytes() {
            hash = fnv_feed(hash, *byte);
        }
    }
    base ^ hash
}

fn fnv_feed(hash: u64, byte: u8) -> u64 {
    (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3)
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
                "priority levels need {}; run 'chezmoi apply ~/.config/bob/config.yml'",
                path.display()
            ))
        } else {
            ConfigError::Read(format!("read {}: {error}", path.display()))
        }
    })?;
    parse_priority_property(&text, path)
}

/// The deployed four-level property, for unit tests that need a
/// `PriorityProperty` without touching the filesystem.
#[cfg(test)]
pub(crate) fn test_property() -> PriorityProperty {
    parse_test_property(
        r#"
properties:
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
"#,
    )
}

/// Parse a `PriorityProperty` from YAML text, for unit tests that need a
/// non-deployed window (a zero-width window, fixed offsets) without
/// touching the filesystem.
#[cfg(test)]
pub(crate) fn parse_test_property(text: &str) -> PriorityProperty {
    parse_priority_property(text, Path::new("test-config.yml"))
        .expect("test property parses")
}

pub(crate) fn load_highlights_config(
    path: &Path,
) -> Result<HighlightsConfig, ConfigError> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(HighlightsConfig::default());
        }
        Err(error) => {
            return Err(ConfigError::Read(format!(
                "read {}: {error}",
                path.display()
            )));
        }
    };
    parse_highlights_config(&text, path)
}

#[derive(Debug, Deserialize)]
pub(crate) struct RawConfig {
    #[serde(default)]
    properties: Vec<RawProperty>,
    #[serde(default)]
    highlights: Option<RawHighlights>,
    #[serde(default)]
    gkeep: Option<RawGkeep>,
    #[serde(default)]
    pub(crate) plan: Option<serde_yaml::Value>,
    #[serde(default)]
    pub(crate) freshness: Option<serde_yaml::Value>,
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

#[derive(Debug, Deserialize)]
struct RawHighlights {
    #[serde(default)]
    pre_scan_hook: Option<String>,
    #[serde(default)]
    pre_scan_command: Option<serde_yaml::Value>,
}

#[derive(Debug, Deserialize)]
struct RawGkeep {
    #[serde(default)]
    email: Option<String>,
    #[serde(default)]
    token_command: Option<String>,
    #[serde(default)]
    token_store_command: Option<String>,
    #[serde(default)]
    device_id: Option<String>,
    #[serde(default)]
    target: Option<String>,
    #[serde(default)]
    timeout_secs: Option<u64>,
}

pub(crate) fn parse_priority_property(
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

    let mut seen_values = std::collections::HashSet::new();
    for level in &levels {
        if !seen_values.insert(level.value.clone()) {
            return Err(ConfigError::Invalid(format!(
                "priority property \"{name}\" in {path_display} configures duplicate value {:?}",
                level.value
            )));
        }
    }
    let mut seen_labels = std::collections::HashSet::new();
    for level in &levels {
        if !seen_labels.insert(level.label.to_ascii_lowercase()) {
            return Err(ConfigError::Invalid(format!(
                "priority property \"{name}\" in {path_display} configures duplicate label {:?}",
                level.label
            )));
        }
    }

    Ok(PriorityProperty { name, levels })
}

pub(crate) fn load_gkeep_config(
    path: &Path,
) -> Result<GkeepSettings, ConfigError> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(GkeepSettings::default());
        }
        Err(error) => {
            return Err(ConfigError::Read(format!(
                "read {}: {error}",
                path.display()
            )));
        }
    };
    parse_gkeep_config(&text, path)
}

pub(crate) fn parse_gkeep_config(
    text: &str,
    path: &Path,
) -> Result<GkeepSettings, ConfigError> {
    let config: RawConfig = serde_yaml::from_str(text).map_err(|error| {
        ConfigError::Invalid(format!("parse {}: {error}", path.display()))
    })?;

    Ok(config
        .gkeep
        .map(|raw| GkeepSettings {
            email: raw.email,
            token_command: raw.token_command,
            token_store_command: raw.token_store_command,
            device_id: raw.device_id,
            target: raw.target,
            timeout_secs: raw.timeout_secs,
        })
        .unwrap_or_default())
}

pub(crate) fn parse_highlights_config(
    text: &str,
    path: &Path,
) -> Result<HighlightsConfig, ConfigError> {
    let config: RawConfig = serde_yaml::from_str(text).map_err(|error| {
        ConfigError::Invalid(format!("parse {}: {error}", path.display()))
    })?;

    if config
        .highlights
        .as_ref()
        .is_some_and(|highlights| highlights.pre_scan_command.is_some())
    {
        return Err(ConfigError::Invalid(format!(
            "highlights.pre_scan_command in {} was renamed; use highlights.pre_scan_hook",
            path.display()
        )));
    }

    let pre_scan_hook = config
        .highlights
        .and_then(|highlights| highlights.pre_scan_hook)
        .map(|command| command.trim().to_string())
        .filter(|command| !command.is_empty());

    Ok(HighlightsConfig { pre_scan_hook })
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

    #[test]
    fn parses_highlights_pre_scan_hook() {
        let config = parse_highlights_config(
            r#"
unused_top_level: ignored
properties:
  - name: priority
    values: priority
highlights:
  pre_scan_hook: " bob_xlib_pull "
"#,
            Path::new("/config.yml"),
        )
        .expect("valid highlights config");

        assert_eq!(config.pre_scan_hook(), Some("bob_xlib_pull"));
    }

    #[test]
    fn parses_absent_highlights_config_as_none() {
        let config = parse_highlights_config(
            r#"
properties:
  - name: priority
    values: priority
"#,
            Path::new("/config.yml"),
        )
        .expect("valid config without highlights block");

        assert_eq!(config.pre_scan_hook(), None);
    }

    #[test]
    fn missing_gkeep_file_loads_defaults() {
        let settings =
            load_gkeep_config(Path::new("/definitely/missing/config.yml"))
                .expect("missing file gives defaults");
        assert_eq!(settings, GkeepSettings::default());
    }

    #[test]
    fn parses_full_gkeep_section_and_ignores_unknown_keys() {
        let settings = parse_gkeep_config(
            r#"
unknown_top_level: ignored
gkeep:
  email: bryanbugyi34@gmail.com
  token_command: "pass show gkeep/master_token"
  token_store_command: "pass insert -m -f gkeep/master_token"
  device_id: 3f9c0a1b2c3d4e5f
  target: gkeep_inbox.md
  timeout_secs: 300
  future_key: ignored
"#,
            Path::new("/config.yml"),
        )
        .expect("valid gkeep config");

        assert_eq!(
            settings,
            GkeepSettings {
                email: Some("bryanbugyi34@gmail.com".to_string()),
                token_command: Some("pass show gkeep/master_token".to_string()),
                token_store_command: Some(
                    "pass insert -m -f gkeep/master_token".to_string()
                ),
                device_id: Some("3f9c0a1b2c3d4e5f".to_string()),
                target: Some("gkeep_inbox.md".to_string()),
                timeout_secs: Some(300),
            }
        );
    }

    #[test]
    fn parses_absent_gkeep_section_as_defaults() {
        let settings = parse_gkeep_config(
            r#"
properties:
  - name: priority
    values: priority
"#,
            Path::new("/config.yml"),
        )
        .expect("valid config without gkeep block");

        assert_eq!(settings, GkeepSettings::default());
    }

    #[test]
    fn rejects_invalid_gkeep_yaml() {
        let error = parse_gkeep_config(
            "gkeep:\n\ttimeout_secs: [unclosed",
            Path::new("/config.yml"),
        )
        .expect_err("invalid YAML must be rejected");

        assert!(
            error.message().contains("parse /config.yml"),
            "invalid YAML must name the file: {}",
            error.message()
        );
    }

    #[test]
    fn rejects_non_numeric_gkeep_timeout() {
        let error = parse_gkeep_config(
            "gkeep:\n  timeout_secs: soon\n",
            Path::new("/config.yml"),
        )
        .expect_err("non-numeric timeout must be rejected");

        assert!(
            error.message().contains("parse /config.yml"),
            "bad value must name the file: {}",
            error.message()
        );
    }

    #[test]
    fn blank_highlights_pre_scan_hook_disables_file_hook() {
        let config = parse_highlights_config(
            r#"
highlights:
  pre_scan_hook: "   "
"#,
            Path::new("/config.yml"),
        )
        .expect("valid blank highlights config");

        assert_eq!(config.pre_scan_hook(), None);
    }

    #[test]
    fn rejects_legacy_highlights_pre_scan_command() {
        let error = parse_highlights_config(
            r#"
highlights:
  pre_scan_command: "bob_xlib_pull"
"#,
            Path::new("/config.yml"),
        )
        .expect_err("legacy pre_scan_command must be rejected");

        assert!(
            error.message().contains("pre_scan_hook"),
            "legacy rejection must name the new spelling: {}",
            error.message()
        );
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

    fn deployed_property() -> PriorityProperty {
        parse(DEPLOYED_CONFIG).expect("deployed config parses")
    }

    #[test]
    fn level_for_value_matches_exact_value_after_trim() {
        let property = deployed_property();
        assert_eq!(
            property
                .level_for_value("medium")
                .map(|level| level.label()),
            Some("P2")
        );
        assert_eq!(
            property
                .level_for_value("  medium  ")
                .map(|level| level.label()),
            Some("P2")
        );
        assert_eq!(property.level_for_value("Medium"), None);
        assert_eq!(property.level_for_value("highest"), None);
        assert_eq!(property.level_for_value(""), None);
    }

    #[test]
    fn level_by_label_matches_ascii_case_insensitively() {
        let property = deployed_property();
        assert_eq!(
            property.level_by_label("p2").map(|level| level.value()),
            Some("medium")
        );
        assert_eq!(
            property.level_by_label("P2").map(|level| level.value()),
            Some("medium")
        );
        assert_eq!(
            property.level_by_label("  p3 ").map(|level| level.value()),
            Some("low")
        );
        assert_eq!(property.level_by_label("P5"), None);
    }

    #[test]
    fn levels_exposes_every_configured_level() {
        let property = deployed_property();
        assert_eq!(
            property
                .levels()
                .iter()
                .map(|level| level.label())
                .collect::<Vec<_>>(),
            vec!["P1", "P2", "P3", "P4"]
        );
    }

    #[test]
    fn rejects_duplicate_level_values() {
        let error = parse(
            r#"
properties:
  - name: priority
    values: priority
    schedules: scheduled
    levels:
      - label: P1
        value: high
        min_days: 2
        max_days: 7
      - label: P1b
        value: high
        min_days: 2
        max_days: 7
"#,
        )
        .expect_err("duplicate values");
        assert!(error.message().contains("duplicate value"));
        assert!(error.message().contains("high"));
    }

    #[test]
    fn rejects_duplicate_level_labels() {
        let error = parse(
            r#"
properties:
  - name: priority
    values: priority
    schedules: scheduled
    levels:
      - label: P1
        value: high
        min_days: 2
        max_days: 7
      - label: P1
        value: urgent
        min_days: 1
        max_days: 1
"#,
        )
        .expect_err("duplicate labels");
        assert!(error.message().contains("duplicate label"));
    }

    #[test]
    fn rejects_labels_that_differ_only_by_case() {
        let error = parse(
            r#"
properties:
  - name: priority
    values: priority
    schedules: scheduled
    levels:
      - label: P1
        value: high
        min_days: 2
        max_days: 7
      - label: p1
        value: urgent
        min_days: 1
        max_days: 1
"#,
        )
        .expect_err("case-only label difference");
        assert!(error.message().contains("duplicate label"));
    }

    #[test]
    fn missing_file_message_is_command_neutral() {
        let error = load_priority_property(Path::new("/does/not/exist.yml"))
            .expect_err("missing file");
        assert_eq!(
            error,
            ConfigError::Read(
                "priority levels need /does/not/exist.yml; run 'chezmoi apply ~/.config/bob/config.yml'".to_string()
            )
        );
        assert!(
            !error.message().contains("p:<N>"),
            "message must not name a single command: {}",
            error.message()
        );
    }

    #[test]
    fn derive_seed_is_deterministic_and_sensitive_to_every_part() {
        let first = derive_seed(42, &["notes/sase.md", "1a2b3c4d", "0"]);
        assert_eq!(first, derive_seed(42, &["notes/sase.md", "1a2b3c4d", "0"]));
        assert_ne!(first, derive_seed(43, &["notes/sase.md", "1a2b3c4d", "0"]));
        assert_ne!(
            first,
            derive_seed(42, &["notes/other.md", "1a2b3c4d", "0"])
        );
        assert_ne!(first, derive_seed(42, &["notes/sase.md", "9f8e7d6c", "0"]));
        assert_ne!(first, derive_seed(42, &["notes/sase.md", "1a2b3c4d", "1"]));
    }

    #[test]
    fn derive_seed_separator_prevents_part_boundary_collisions() {
        assert_ne!(derive_seed(7, &["ab", "c"]), derive_seed(7, &["a", "bc"]));
    }

    #[test]
    fn mix64_spreads_sequential_inputs() {
        let mut seen = std::collections::HashSet::new();
        for value in 0..1000u64 {
            seen.insert(mix64(value));
        }
        assert_eq!(seen.len(), 1000);
    }

    #[test]
    fn mistyped_plan_block_leaves_priority_loader_working() {
        let text = "properties:\n  - name: priority\n    values: priority\n    schedules: scheduled\n    levels:\n      - label: P1\n        value: high\n        min_days: 1\n        max_days: 1\nplan:\n  max_themes: many\n";
        parse_priority_property(text, Path::new("/config.yml"))
            .expect("priority loader must ignore a mistyped plan block");
    }

    #[test]
    fn mistyped_plan_block_leaves_highlights_loader_working() {
        let text =
            "highlights:\n  pre_scan_hook: hook\nplan:\n  strict: \"yes\"\n";
        let config = parse_highlights_config(text, Path::new("/config.yml"))
            .expect("highlights loader must ignore a mistyped plan block");
        assert_eq!(config.pre_scan_hook(), Some("hook"));
    }

    #[test]
    fn mistyped_plan_block_leaves_gkeep_loader_working() {
        let text = "gkeep:\n  email: a@b.c\nplan:\n  exempt: GTD\n";
        let settings = parse_gkeep_config(text, Path::new("/config.yml"))
            .expect("gkeep loader must ignore a mistyped plan block");
        assert_eq!(settings.email.as_deref(), Some("a@b.c"));
    }
}
