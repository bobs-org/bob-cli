use std::path::Path;

use super::ConfigError;

/// Task freshness configuration (`freshness:` block).
///
/// See `docs/freshness.md` for the full contract. The interval is the
/// days before a confirmed Ready task is due for review; the budget is
/// an optional daily goal meter that never hides tasks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FreshnessConfig {
    pub(crate) interval: u16,
    pub(crate) stale_daily_budget: Option<u32>,
    pub(crate) interval_from_config: bool,
}

impl Default for FreshnessConfig {
    fn default() -> Self {
        Self {
            interval: 7,
            stale_daily_budget: None,
            interval_from_config: false,
        }
    }
}

impl FreshnessConfig {
    // Test-only getters: production reads the fields directly.
    #[cfg(test)]
    pub(crate) fn interval(&self) -> u16 {
        self.interval
    }

    #[cfg(test)]
    pub(crate) fn stale_daily_budget(&self) -> Option<u32> {
        self.stale_daily_budget
    }
}

pub(crate) fn load_freshness_config(
    path: &Path,
) -> Result<FreshnessConfig, ConfigError> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(FreshnessConfig::default());
        }
        Err(error) => {
            return Err(ConfigError::Read(format!(
                "read {}: {error}",
                path.display()
            )));
        }
    };
    parse_freshness_config(&text, path)
}

fn freshness_value(
    text: &str,
    path: &Path,
) -> Result<Option<serde_yaml::Value>, ConfigError> {
    let config: super::RawConfig =
        serde_yaml::from_str(text).map_err(|error| {
            ConfigError::Invalid(format!("parse {}: {error}", path.display()))
        })?;
    Ok(config.freshness)
}

fn parse_freshness_config(
    text: &str,
    path: &Path,
) -> Result<FreshnessConfig, ConfigError> {
    let freshness = freshness_value(text, path)?;
    let path_display = path.display();
    let defaults = FreshnessConfig::default();

    let Some(freshness) = freshness else {
        return Ok(defaults);
    };
    if freshness.is_null() {
        return Ok(defaults);
    }
    let serde_yaml::Value::Mapping(ref mapping) = freshness else {
        return Err(ConfigError::Invalid(format!(
            "freshness in {path_display} must be a mapping"
        )));
    };

    let get = |name: &str| {
        let key = serde_yaml::Value::String(name.to_string());
        mapping.get(&key).cloned()
    };

    let mut interval = defaults.interval;
    let mut interval_from_config = false;
    if let Some(value) = get("interval")
        && !value.is_null()
    {
        interval = parse_interval(&value, &path_display.to_string())?;
        interval_from_config = true;
    }

    let mut budget = defaults.stale_daily_budget;
    if let Some(value) = get("stale_daily_budget")
        && !value.is_null()
    {
        budget = Some(parse_budget(&value, &path_display.to_string())?);
    }

    // Unknown keys are ignored, like every other config block.
    Ok(FreshnessConfig {
        interval,
        stale_daily_budget: budget,
        interval_from_config,
    })
}

fn parse_interval(
    value: &serde_yaml::Value,
    path_display: &str,
) -> Result<u16, ConfigError> {
    let number = match value {
        serde_yaml::Value::Number(number) => {
            if let Some(int) = number.as_i64() {
                int
            } else if let Some(uint) = number.as_u64()
                && let Ok(int) = i64::try_from(uint)
            {
                int
            } else {
                return Err(ConfigError::Invalid(format!(
                    "freshness.interval in {path_display} must be an integer 1-365; got {value:?}"
                )));
            }
        }
        _ => {
            return Err(ConfigError::Invalid(format!(
                "freshness.interval in {path_display} must be an integer 1-365; got {}",
                render_scalar(value)
            )));
        }
    };
    if !(1..=365).contains(&number) {
        return Err(ConfigError::Invalid(format!(
            "freshness.interval in {path_display} must be an integer 1-365; got {number}"
        )));
    }
    Ok(number as u16)
}

fn parse_budget(
    value: &serde_yaml::Value,
    path_display: &str,
) -> Result<u32, ConfigError> {
    let number = match value {
        serde_yaml::Value::Number(number) => {
            if let Some(int) = number.as_i64() {
                int
            } else if let Some(uint) = number.as_u64()
                && let Ok(int) = i64::try_from(uint)
            {
                int
            } else {
                return Err(ConfigError::Invalid(format!(
                    "freshness.stale_daily_budget in {path_display} must be an integer >= 1; got {value:?}"
                )));
            }
        }
        _ => {
            return Err(ConfigError::Invalid(format!(
                "freshness.stale_daily_budget in {path_display} must be an integer >= 1; got {}",
                render_scalar(value)
            )));
        }
    };
    if number < 1 {
        return Err(ConfigError::Invalid(format!(
            "freshness.stale_daily_budget in {path_display} must be an integer >= 1; got {number}"
        )));
    }
    u32::try_from(number).map_err(|_| {
        ConfigError::Invalid(format!(
            "freshness.stale_daily_budget in {path_display} must be an integer >= 1; got {number}"
        ))
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

    #[test]
    fn missing_freshness_file_loads_defaults() {
        let config =
            load_freshness_config(Path::new("/definitely/missing/config.yml"))
                .expect("missing file gives defaults");
        assert_eq!(config, FreshnessConfig::default());
        assert_eq!(config.interval(), 7);
        assert_eq!(config.stale_daily_budget(), None);
    }

    #[test]
    fn absent_freshness_block_loads_defaults() {
        let config = parse_freshness_config(
            "properties: []\nunknown_top_level: ignored\n",
            Path::new("/config.yml"),
        )
        .expect("missing block gives defaults");
        assert_eq!(config, FreshnessConfig::default());
    }

    #[test]
    fn null_freshness_block_loads_defaults() {
        let config =
            parse_freshness_config("freshness:\n", Path::new("/config.yml"))
                .expect("null block gives defaults");
        assert_eq!(config, FreshnessConfig::default());
    }

    #[test]
    fn parses_freshness_overrides_and_ignores_unknown_keys() {
        let config = parse_freshness_config(
            "unknown_top_level: ignored\n\
            freshness:\n\
            \x20 interval: 10\n\
            \x20 stale_daily_budget: 15\n\
            \x20 unknown_key: ignored\n",
            Path::new("/config.yml"),
        )
        .expect("valid freshness block");
        assert_eq!(config.interval(), 10);
        assert_eq!(config.stale_daily_budget(), Some(15));
        assert!(config.interval_from_config);
    }

    #[test]
    fn null_values_fall_back_to_defaults() {
        let config = parse_freshness_config(
            "freshness:\n  interval:\n  stale_daily_budget:\n",
            Path::new("/config.yml"),
        )
        .expect("null values give defaults");
        assert_eq!(config, FreshnessConfig::default());
    }

    #[test]
    fn rejects_invalid_freshness_values() {
        for text in [
            "freshness:\n  interval: 0\n",
            "freshness:\n  interval: 366\n",
            "freshness:\n  interval: -3\n",
            "freshness:\n  interval: soon\n",
            "freshness:\n  interval: 7.5\n",
            "freshness:\n  stale_daily_budget: 0\n",
            "freshness:\n  stale_daily_budget: soon\n",
            "freshness: [1, 2]\n",
        ] {
            let error = parse_freshness_config(text, Path::new("/config.yml"))
                .expect_err("invalid freshness value must fail");
            assert!(
                matches!(error, ConfigError::Invalid(_)),
                "expected invalid config for {text:?}, got {error:?}"
            );
        }
    }

    #[test]
    fn mistyped_freshness_block_leaves_other_loaders_working() {
        for text in [
            "properties:\n  - name: priority\n    values: priority\n    schedules: scheduled\n    levels:\n      - label: P1\n        value: high\n        min_days: 1\n        max_days: 1\nfreshness:\n  interval: soon\n",
            "properties:\n  - name: priority\n    values: priority\n    schedules: scheduled\n    levels:\n      - label: P1\n        value: high\n        min_days: 1\n        max_days: 1\nfreshness:\n  stale_daily_budget: soon\n",
            "properties:\n  - name: priority\n    values: priority\n    schedules: scheduled\n    levels:\n      - label: P1\n        value: high\n        min_days: 1\n        max_days: 1\nfreshness: [1, 2]\n",
        ] {
            let path = Path::new("/config.yml");
            super::super::parse_priority_property(text, path)
                .expect("priority loader must ignore a mistyped freshness block");
            super::super::parse_highlights_config(text, path)
                .expect("highlights loader must ignore a mistyped freshness block");
            super::super::parse_gkeep_config(text, path)
                .expect("gkeep loader must ignore a mistyped freshness block");
            let error = parse_freshness_config(text, path)
                .expect_err("freshness loader must still reject the mistyped block");
            assert!(matches!(error, ConfigError::Invalid(_)));
        }
    }
}
