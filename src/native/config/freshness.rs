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
    pub(crate) rotten_daily_budget: Option<u32>,
    pub(crate) interval_from_config: bool,
    /// The removed `stale_daily_budget` key supplied the budget, so
    /// callers owe one `freshness_stale_daily_budget_deprecated`
    /// diagnostic for this loaded config.
    pub(crate) stale_budget_deprecated: bool,
}

impl Default for FreshnessConfig {
    fn default() -> Self {
        Self {
            interval: 7,
            rotten_daily_budget: None,
            interval_from_config: false,
            stale_budget_deprecated: false,
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
    pub(crate) fn rotten_daily_budget(&self) -> Option<u32> {
        self.rotten_daily_budget
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

    // The canonical `rotten_daily_budget` wins by presence,
    // including an explicit null (budget off). The removed
    // `stale_daily_budget` still supplies the budget for one release
    // with a deprecation diagnostic; when both occur the legacy value
    // is warned about and ignored.
    let canonical = get("rotten_daily_budget");
    let legacy = get("stale_daily_budget");
    let legacy_present = matches!(legacy, Some(ref value) if !value.is_null());
    let mut stale_budget_deprecated = false;
    let mut budget = defaults.rotten_daily_budget;
    if let Some(value) = canonical {
        if !value.is_null() {
            budget = Some(parse_budget(
                &value,
                "freshness.rotten_daily_budget",
                &path_display.to_string(),
            )?);
        }
        stale_budget_deprecated = legacy_present;
    } else if let Some(value) = legacy
        && !value.is_null()
    {
        budget = Some(parse_budget(
            &value,
            "freshness.stale_daily_budget",
            &path_display.to_string(),
        )?);
        stale_budget_deprecated = true;
    }

    // Unknown keys are ignored, like every other config block.
    Ok(FreshnessConfig {
        interval,
        rotten_daily_budget: budget,
        interval_from_config,
        stale_budget_deprecated,
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
    key: &str,
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
                    "{key} in {path_display} must be an integer >= 1; got {value:?}"
                )));
            }
        }
        _ => {
            return Err(ConfigError::Invalid(format!(
                "{key} in {path_display} must be an integer >= 1; got {}",
                render_scalar(value)
            )));
        }
    };
    if number < 1 {
        return Err(ConfigError::Invalid(format!(
            "{key} in {path_display} must be an integer >= 1; got {number}"
        )));
    }
    u32::try_from(number).map_err(|_| {
        ConfigError::Invalid(format!(
            "{key} in {path_display} must be an integer >= 1; got {number}"
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
        assert_eq!(config.rotten_daily_budget(), None);
        assert!(!config.stale_budget_deprecated);
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
            \x20 rotten_daily_budget: 15\n\
            \x20 unknown_key: ignored\n",
            Path::new("/config.yml"),
        )
        .expect("valid freshness block");
        assert_eq!(config.interval(), 10);
        assert_eq!(config.rotten_daily_budget(), Some(15));
        assert!(config.interval_from_config);
        assert!(!config.stale_budget_deprecated);
    }

    #[test]
    fn null_values_fall_back_to_defaults() {
        let config = parse_freshness_config(
            "freshness:\n  interval:\n  rotten_daily_budget:\n",
            Path::new("/config.yml"),
        )
        .expect("null values give defaults");
        assert_eq!(config, FreshnessConfig::default());
    }

    #[test]
    fn legacy_budget_key_supplies_budget_with_deprecation_flag() {
        let config = parse_freshness_config(
            "freshness:\n  stale_daily_budget: 15\n",
            Path::new("/config.yml"),
        )
        .expect("legacy budget key works for one release");
        assert_eq!(config.rotten_daily_budget(), Some(15));
        assert!(config.stale_budget_deprecated);
    }

    #[test]
    fn canonical_budget_key_wins_over_legacy() {
        // Both present: the canonical value wins and the legacy value
        // is ignored (still flagged for the deprecation diagnostic).
        let config = parse_freshness_config(
            "freshness:\n  rotten_daily_budget: 20\n  stale_daily_budget: 15\n",
            Path::new("/config.yml"),
        )
        .expect("both budget keys");
        assert_eq!(config.rotten_daily_budget(), Some(20));
        assert!(config.stale_budget_deprecated);

        // Presence wins, including an explicit null (budget off): the
        // legacy value is ignored.
        let config = parse_freshness_config(
            "freshness:\n  rotten_daily_budget:\n  stale_daily_budget: 15\n",
            Path::new("/config.yml"),
        )
        .expect("canonical null wins");
        assert_eq!(config.rotten_daily_budget(), None);
        assert!(config.stale_budget_deprecated);

        // Equal values still flag the legacy key's presence.
        let config = parse_freshness_config(
            "freshness:\n  rotten_daily_budget: 15\n  stale_daily_budget: 15\n",
            Path::new("/config.yml"),
        )
        .expect("equal budget keys");
        assert_eq!(config.rotten_daily_budget(), Some(15));
        assert!(config.stale_budget_deprecated);
    }

    #[test]
    fn legacy_budget_key_recovers_and_rejects_like_canonical() {
        // An invalid legacy value is rejected while it is selected.
        let error = parse_freshness_config(
            "freshness:\n  stale_daily_budget: soon\n",
            Path::new("/config.yml"),
        )
        .expect_err("invalid legacy budget must fail");
        assert!(
            matches!(error, ConfigError::Invalid(_)),
            "expected invalid config, got {error:?}"
        );
        // Recovery: fixing the legacy value loads again.
        let config = parse_freshness_config(
            "freshness:\n  stale_daily_budget: 9\n",
            Path::new("/config.yml"),
        )
        .expect("fixed legacy budget recovers");
        assert_eq!(config.rotten_daily_budget(), Some(9));
        // An invalid canonical value is rejected even when a valid
        // legacy value is also present: the selected key validates.
        parse_freshness_config(
            "freshness:\n  rotten_daily_budget: soon\n  stale_daily_budget: 9\n",
            Path::new("/config.yml"),
        )
        .expect_err("invalid canonical budget must fail");
    }

    #[test]
    fn rejects_invalid_freshness_values() {
        for text in [
            "freshness:\n  interval: 0\n",
            "freshness:\n  interval: 366\n",
            "freshness:\n  interval: -3\n",
            "freshness:\n  interval: soon\n",
            "freshness:\n  interval: 7.5\n",
            "freshness:\n  rotten_daily_budget: 0\n",
            "freshness:\n  rotten_daily_budget: soon\n",
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
            "properties:\n  - name: priority\n    values: priority\n    schedules: scheduled\n    levels:\n      - label: P1\n        value: high\n        min_days: 1\n        max_days: 1\nfreshness:\n  rotten_daily_budget: soon\n",
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
