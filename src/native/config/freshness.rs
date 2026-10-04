use std::path::Path;

use super::ConfigError;

/// Normalized `freshness.decay` configuration (`docs/freshness.md`).
///
/// `decay` absent, null, `true`, or `{}` means enabled with 3 keeps.
/// `false` keeps counting and display but never asks or skips.
/// `keeps` accepts an integer 0–999 (0 asks on every due Ready
/// re-confirmation, never NEW); `enter` is an optional nonempty
/// configured priority label (absent/null uses interval-aware entry).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DecayConfig {
    pub(crate) enabled: bool,
    pub(crate) keeps: u16,
    pub(crate) enter: Option<String>,
}

impl Default for DecayConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            keeps: 3,
            enter: None,
        }
    }
}

/// Task freshness configuration (`freshness:` block).
///
/// See `docs/freshness.md` for the full contract. The interval is the
/// days before a confirmed Ready task is due for review; the budget is
/// an optional daily goal meter that never hides tasks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FreshnessConfig {
    pub(crate) interval: u16,
    /// Daily review cadence for the `[/]` lane. `None` means the
    /// lane is not walked (`pending_interval: false`).
    pub(crate) pending_interval: Option<u16>,
    /// Daily review cadence for the `[*]` lane. `None` means the
    /// lane is not walked (`next_interval: false`).
    pub(crate) next_interval: Option<u16>,
    /// Explicit `^prj` review cadence. `None` means inherit the
    /// existing Ready chain.
    pub(crate) project_interval: Option<u16>,
    /// Explicit `^ref` review cadence. `None` means inherit the
    /// existing Ready chain (Ready) or lane interval (Pending/Next).
    pub(crate) reference_interval: Option<u16>,
    pub(crate) rotten_daily_budget: Option<u32>,
    pub(crate) interval_from_config: bool,
    /// The removed `stale_daily_budget` key supplied the budget, so
    /// callers owe one `freshness_stale_daily_budget_deprecated`
    /// diagnostic for this loaded config.
    pub(crate) stale_budget_deprecated: bool,
    pub(crate) decay: DecayConfig,
}

impl Default for FreshnessConfig {
    fn default() -> Self {
        Self {
            interval: 7,
            pending_interval: Some(1),
            next_interval: Some(1),
            project_interval: None,
            reference_interval: None,
            rotten_daily_budget: None,
            interval_from_config: false,
            stale_budget_deprecated: false,
            decay: DecayConfig::default(),
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

    let mut pending_interval = defaults.pending_interval;
    if let Some(value) = get("pending_interval") {
        pending_interval = parse_lane_interval(
            &value,
            "freshness.pending_interval",
            &path_display.to_string(),
        )?;
    }
    let mut next_interval = defaults.next_interval;
    if let Some(value) = get("next_interval") {
        next_interval = parse_lane_interval(
            &value,
            "freshness.next_interval",
            &path_display.to_string(),
        )?;
    }

    let mut project_interval = defaults.project_interval;
    if let Some(value) = get("project_interval") {
        project_interval = parse_tracker_interval(
            &value,
            "freshness.project_interval",
            &path_display.to_string(),
        )?;
    }
    let mut reference_interval = defaults.reference_interval;
    if let Some(value) = get("reference_interval") {
        reference_interval = parse_tracker_interval(
            &value,
            "freshness.reference_interval",
            &path_display.to_string(),
        )?;
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

    let decay =
        parse_decay_config(get("decay").as_ref(), &path_display.to_string())?;

    // Unknown keys are ignored, like every other config block.
    Ok(FreshnessConfig {
        interval,
        pending_interval,
        next_interval,
        project_interval,
        reference_interval,
        rotten_daily_budget: budget,
        interval_from_config,
        stale_budget_deprecated,
        decay,
    })
}

/// Normalize the `freshness.decay` value: absent, null, `true`, or an
/// empty mapping means enabled with 3 keeps; `false` disables asking
/// while keeping counting and display; a mapping may set `keeps`
/// (integer 0–999) and `enter` (a nonempty priority label).
/// Anything else is a config error under the current freshness
/// failure contract.
fn parse_decay_config(
    value: Option<&serde_yaml::Value>,
    path_display: &str,
) -> Result<DecayConfig, ConfigError> {
    let key = "freshness.decay";
    let Some(value) = value else {
        return Ok(DecayConfig::default());
    };
    if value.is_null() {
        return Ok(DecayConfig::default());
    }
    if let serde_yaml::Value::Bool(flag) = value {
        if *flag {
            return Ok(DecayConfig::default());
        }
        return Ok(DecayConfig {
            enabled: false,
            ..DecayConfig::default()
        });
    }
    let serde_yaml::Value::Mapping(mapping) = value else {
        return Err(ConfigError::Invalid(format!(
            "{key} in {path_display} must be a mapping, true, false, or null; got {}",
            render_scalar(value)
        )));
    };
    let get = |name: &str| {
        let lookup = serde_yaml::Value::String(name.to_string());
        mapping.get(&lookup).cloned()
    };
    let mut keeps = DecayConfig::default().keeps;
    if let Some(raw) = get("keeps")
        && !raw.is_null()
    {
        keeps = parse_decay_keeps(&raw, path_display)?;
    }
    let mut enter = None;
    if let Some(raw) = get("enter")
        && !raw.is_null()
    {
        let serde_yaml::Value::String(label) = raw else {
            return Err(ConfigError::Invalid(format!(
                "{key}.enter in {path_display} must be a nonempty priority label or null; got {}",
                render_scalar(&raw)
            )));
        };
        if label.trim().is_empty() {
            return Err(ConfigError::Invalid(format!(
                "{key}.enter in {path_display} must be a nonempty priority label or null; got empty"
            )));
        }
        enter = Some(label.trim().to_string());
    }
    // Unknown keys are ignored, like every other config block.
    Ok(DecayConfig {
        enabled: true,
        keeps,
        enter,
    })
}

/// Parse `freshness.decay.keeps`: an integer 0–999. Fractional,
/// negative, out-of-range, and non-numeric values are config errors.
fn parse_decay_keeps(
    value: &serde_yaml::Value,
    path_display: &str,
) -> Result<u16, ConfigError> {
    let key = "freshness.decay.keeps";
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
                    "{key} in {path_display} must be an integer 0-999; got {value:?}"
                )));
            }
        }
        _ => {
            return Err(ConfigError::Invalid(format!(
                "{key} in {path_display} must be an integer 0-999; got {}",
                render_scalar(value)
            )));
        }
    };
    if !(0..=999).contains(&number) {
        return Err(ConfigError::Invalid(format!(
            "{key} in {path_display} must be an integer 0-999; got {number}"
        )));
    }
    Ok(number as u16)
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

/// Parse a lane interval: absent or null means the default 1,
/// `false` turns that lane's walk off, and an integer 1–365 sets it.
/// Anything else (including `true`, 0, 366, strings, floats) is a
/// config error shaped like `interval`'s.
fn parse_lane_interval(
    value: &serde_yaml::Value,
    key: &str,
    path_display: &str,
) -> Result<Option<u16>, ConfigError> {
    if value.is_null() {
        return Ok(Some(1));
    }
    if let serde_yaml::Value::Bool(flag) = value {
        if !flag {
            return Ok(None);
        }
        return Err(ConfigError::Invalid(format!(
            "{key} in {path_display} must be an integer 1-365 or false; got {flag}"
        )));
    }
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
                    "{key} in {path_display} must be an integer 1-365 or false; got {value:?}"
                )));
            }
        }
        _ => {
            return Err(ConfigError::Invalid(format!(
                "{key} in {path_display} must be an integer 1-365 or false; got {}",
                render_scalar(value)
            )));
        }
    };
    if !(1..=365).contains(&number) {
        return Err(ConfigError::Invalid(format!(
            "{key} in {path_display} must be an integer 1-365 or false; got {number}"
        )));
    }
    Ok(Some(number as u16))
}

/// Parse a tracker interval: absent or null means inherit the
/// existing cadence (`None`). An integer 1–365 sets the explicit
/// type cadence. Booleans (including `false`), zero, negatives,
/// values above 365, fractional numbers, strings, and containers
/// are config errors.
fn parse_tracker_interval(
    value: &serde_yaml::Value,
    key: &str,
    path_display: &str,
) -> Result<Option<u16>, ConfigError> {
    if value.is_null() {
        return Ok(None);
    }
    if let serde_yaml::Value::Bool(flag) = value {
        return Err(ConfigError::Invalid(format!(
            "{key} in {path_display} must be an integer 1-365 or null; got {flag}"
        )));
    }
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
                    "{key} in {path_display} must be an integer 1-365 or null; got {value:?}"
                )));
            }
        }
        _ => {
            return Err(ConfigError::Invalid(format!(
                "{key} in {path_display} must be an integer 1-365 or null; got {}",
                render_scalar(value)
            )));
        }
    };
    if !(1..=365).contains(&number) {
        return Err(ConfigError::Invalid(format!(
            "{key} in {path_display} must be an integer 1-365 or null; got {number}"
        )));
    }
    Ok(Some(number as u16))
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
    fn decay_defaults_when_absent_null_true_or_empty() {
        for text in [
            "properties: []\n",
            "freshness:\n",
            "freshness:\n  decay:\n",
            "freshness:\n  decay: true\n",
            "freshness:\n  decay: {}\n",
            "freshness:\n  decay:\n    keeps:\n    enter:\n",
            "freshness:\n  interval: 7\n  decay:\n    unknown_key: ignored\n",
        ] {
            let config = parse_freshness_config(text, Path::new("/config.yml"))
                .expect("decay defaults");
            assert_eq!(config.decay, DecayConfig::default(), "for {text:?}");
            assert!(config.decay.enabled);
            assert_eq!(config.decay.keeps, 3);
            assert_eq!(config.decay.enter, None);
        }
    }

    #[test]
    fn decay_false_counts_but_never_asks() {
        let config = parse_freshness_config(
            "freshness:\n  decay: false\n",
            Path::new("/config.yml"),
        )
        .expect("decay false parses");
        assert!(!config.decay.enabled);
        assert_eq!(config.decay.keeps, 3);
        assert_eq!(config.decay.enter, None);
    }

    #[test]
    fn decay_parses_zero_and_fixed_entry() {
        let config = parse_freshness_config(
            "freshness:\n  decay:\n    keeps: 0\n",
            Path::new("/config.yml"),
        )
        .expect("keeps 0 parses");
        assert!(config.decay.enabled);
        assert_eq!(config.decay.keeps, 0);
        let config = parse_freshness_config(
            "freshness:\n  decay:\n    keeps: 2\n    enter: P1\n",
            Path::new("/config.yml"),
        )
        .expect("fixed entry parses");
        assert_eq!(config.decay.keeps, 2);
        assert_eq!(config.decay.enter, Some("P1".to_string()));
    }

    #[test]
    fn rejects_invalid_decay_values() {
        for text in [
            "freshness:\n  decay:\n    keeps: -1\n",
            "freshness:\n  decay:\n    keeps: 1000\n",
            "freshness:\n  decay:\n    keeps: 2.5\n",
            "freshness:\n  decay:\n    keeps: soon\n",
            "freshness:\n  decay: soon\n",
            "freshness:\n  decay: 3\n",
            "freshness:\n  decay: [1, 2]\n",
            "freshness:\n  decay:\n    enter: ''\n",
            "freshness:\n  decay:\n    enter: '   '\n",
            "freshness:\n  decay:\n    enter: 2\n",
        ] {
            let error = parse_freshness_config(text, Path::new("/config.yml"))
                .expect_err("invalid decay must fail");
            assert!(
                matches!(error, ConfigError::Invalid(_)),
                "expected invalid config for {text:?}, got {error:?}"
            );
        }
    }

    #[test]
    fn lane_intervals_default_absent_and_null() {
        for text in [
            "freshness:\n  interval: 7\n",
            "freshness:\n  pending_interval:\n  next_interval:\n",
        ] {
            let config = parse_freshness_config(text, Path::new("/config.yml"))
                .expect("absent or null lanes give defaults");
            assert_eq!(config.pending_interval, Some(1));
            assert_eq!(config.next_interval, Some(1));
        }
    }

    #[test]
    fn lane_intervals_parse_false_and_integers() {
        let config = parse_freshness_config(
            "freshness:\n  pending_interval: false\n  next_interval: 3\n",
            Path::new("/config.yml"),
        )
        .expect("false and integer lanes");
        assert_eq!(config.pending_interval, None);
        assert_eq!(config.next_interval, Some(3));
    }

    #[test]
    fn rejects_invalid_lane_intervals() {
        for text in [
            "freshness:\n  pending_interval: true\n",
            "freshness:\n  next_interval: true\n",
            "freshness:\n  pending_interval: 0\n",
            "freshness:\n  next_interval: 366\n",
            "freshness:\n  pending_interval: soon\n",
            "freshness:\n  next_interval: 7.5\n",
        ] {
            let error = parse_freshness_config(text, Path::new("/config.yml"))
                .expect_err("invalid lane interval must fail");
            assert!(
                matches!(error, ConfigError::Invalid(_)),
                "expected invalid config for {text:?}, got {error:?}"
            );
        }
    }

    #[test]
    fn tracker_intervals_default_absent_and_null() {
        for text in [
            "freshness:\n  interval: 7\n",
            "freshness:\n  project_interval:\n  reference_interval:\n",
        ] {
            let config = parse_freshness_config(text, Path::new("/config.yml"))
                .expect("absent or null tracker keys inherit");
            assert_eq!(config.project_interval, None);
            assert_eq!(config.reference_interval, None);
        }
    }

    #[test]
    fn tracker_intervals_parse_integers() {
        let config = parse_freshness_config(
            "freshness:\n  project_interval: 1\n  reference_interval: 3\n",
            Path::new("/config.yml"),
        )
        .expect("tracker intervals parse");
        assert_eq!(config.project_interval, Some(1));
        assert_eq!(config.reference_interval, Some(3));
        let config = parse_freshness_config(
            "freshness:\n  project_interval: 365\n  reference_interval: 365\n",
            Path::new("/config.yml"),
        )
        .expect("tracker upper bound parses");
        assert_eq!(config.project_interval, Some(365));
        assert_eq!(config.reference_interval, Some(365));
    }

    #[test]
    fn rejects_invalid_tracker_intervals() {
        for text in [
            "freshness:\n  project_interval: false\n",
            "freshness:\n  reference_interval: false\n",
            "freshness:\n  project_interval: true\n",
            "freshness:\n  reference_interval: true\n",
            "freshness:\n  project_interval: 0\n",
            "freshness:\n  reference_interval: 366\n",
            "freshness:\n  project_interval: soon\n",
            "freshness:\n  reference_interval: 7.5\n",
            "freshness:\n  project_interval: [1]\n",
            "freshness:\n  reference_interval:\n    days: 3\n",
        ] {
            let error = parse_freshness_config(text, Path::new("/config.yml"))
                .expect_err("invalid tracker interval must fail");
            assert!(
                matches!(error, ConfigError::Invalid(_)),
                "expected invalid config for {text:?}, got {error:?}"
            );
        }
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
            "freshness:\n  pending_interval: true\n",
            "freshness:\n  next_interval: 0\n",
            "freshness:\n  pending_interval: 366\n",
            "freshness:\n  next_interval: soon\n",
            "freshness:\n  project_interval: false\n",
            "freshness:\n  reference_interval: 0\n",
            "freshness:\n  project_interval: 366\n",
            "freshness:\n  reference_interval: soon\n",
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
            "properties:\n  - name: priority\n    values: priority\n    schedules: scheduled\n    levels:\n      - label: P1\n        value: high\n        min_days: 1\n        max_days: 1\nfreshness:\n  pending_interval: soon\n",
            "properties:\n  - name: priority\n    values: priority\n    schedules: scheduled\n    levels:\n      - label: P1\n        value: high\n        min_days: 1\n        max_days: 1\nfreshness:\n  next_interval: soon\n",
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
