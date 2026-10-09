use std::path::Path;

use super::ConfigError;

/// Today's plan budget configuration (`plan:` block).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PlanConfig {
    pub(crate) max_themes: u32,
    pub(crate) max_links: u32,
    pub(crate) max_next: u32,
    pub(crate) max_pending: u32,
    pub(crate) max_ready: u32,
    pub(crate) max_ready_per_note: u32,
    pub(crate) strict: bool,
    pub(crate) link_unblocked: bool,
    pub(crate) exempt: Vec<String>,
    pub(crate) inventory_labels: Vec<String>,
}

impl Default for PlanConfig {
    fn default() -> Self {
        Self {
            max_themes: 3,
            max_links: 10,
            max_next: 15,
            max_pending: 10,
            max_ready: 100,
            max_ready_per_note: 5,
            strict: false,
            link_unblocked: true,
            exempt: vec!["GTD".to_string()],
            inventory_labels: vec![
                "LATER".to_string(),
                "MISC".to_string(),
                "NEW FEATURES".to_string(),
                "SASE".to_string(),
            ],
        }
    }
}

impl PlanConfig {
    pub(crate) fn max_themes(&self) -> u32 {
        self.max_themes
    }

    pub(crate) fn max_links(&self) -> u32 {
        self.max_links
    }

    pub(crate) fn max_next(&self) -> u32 {
        self.max_next
    }

    pub(crate) fn max_pending(&self) -> u32 {
        self.max_pending
    }

    pub(crate) fn max_ready(&self) -> u32 {
        self.max_ready
    }

    pub(crate) fn max_ready_per_note(&self) -> u32 {
        self.max_ready_per_note
    }

    pub(crate) fn strict(&self) -> bool {
        self.strict
    }

    pub(crate) fn link_unblocked(&self) -> bool {
        self.link_unblocked
    }

    pub(crate) fn exempt(&self) -> &[String] {
        &self.exempt
    }

    pub(crate) fn inventory_labels(&self) -> &[String] {
        &self.inventory_labels
    }
}

pub(crate) fn load_plan_config(path: &Path) -> Result<PlanConfig, ConfigError> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(PlanConfig::default());
        }
        Err(error) => {
            return Err(ConfigError::Read(format!(
                "read {}: {error}",
                path.display()
            )));
        }
    };
    parse_plan_config(&text, path)
}

fn plan_value(
    text: &str,
    path: &Path,
) -> Result<Option<serde_yaml::Value>, ConfigError> {
    let config: super::RawConfig =
        serde_yaml::from_str(text).map_err(|error| {
            ConfigError::Invalid(format!("parse {}: {error}", path.display()))
        })?;
    Ok(config.plan)
}

fn parse_plan_config(
    text: &str,
    path: &Path,
) -> Result<PlanConfig, ConfigError> {
    let plan = plan_value(text, path)?;
    let path_display = path.display();
    let defaults = PlanConfig::default();

    let Some(plan) = plan else {
        return Ok(defaults);
    };
    if plan.is_null() {
        return Ok(defaults);
    }
    let serde_yaml::Value::Mapping(ref mapping) = plan else {
        return Err(ConfigError::Invalid(format!(
            "plan in {path_display} must be a mapping"
        )));
    };

    let get = |name: &str| {
        let key = serde_yaml::Value::String(name.to_string());
        mapping.get(&key).cloned()
    };

    let cap = |name: &str, value: Option<serde_yaml::Value>, fallback: u32| {
        let Some(value) = value else {
            return Ok(fallback);
        };
        if value.is_null() {
            return Ok(fallback);
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
                    return Err(ConfigError::Invalid(format!(
                        "plan.{name} in {path_display} must be an integer >= 1; got {value:?}"
                    )));
                }
            }
            _ => {
                return Err(ConfigError::Invalid(format!(
                    "plan.{name} in {path_display} must be an integer >= 1; got {}",
                    render_scalar(&value)
                )));
            }
        };
        if number < 1 {
            return Err(ConfigError::Invalid(format!(
                "plan.{name} in {path_display} must be an integer >= 1; got {number}"
            )));
        }
        u32::try_from(number).map_err(|_| {
            ConfigError::Invalid(format!(
                "plan.{name} in {path_display} must be an integer >= 1; got {number}"
            ))
        })
    };

    let capped_cap = |name: &str,
                      value: Option<serde_yaml::Value>,
                      fallback: u32| {
        let Some(value) = value else {
            return Ok(fallback);
        };
        if value.is_null() {
            return Ok(fallback);
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
                    return Err(ConfigError::Invalid(format!(
                            "plan.{name} in {path_display} must be an integer from 1 to 999; got {value:?}"
                        )));
                }
            }
            _ => {
                return Err(ConfigError::Invalid(format!(
                        "plan.{name} in {path_display} must be an integer from 1 to 999; got {}",
                        render_scalar(&value)
                    )));
            }
        };
        if !(1..=999).contains(&number) {
            return Err(ConfigError::Invalid(format!(
                    "plan.{name} in {path_display} must be an integer from 1 to 999; got {number}"
                )));
        }
        u32::try_from(number).map_err(|_| {
                ConfigError::Invalid(format!(
                    "plan.{name} in {path_display} must be an integer from 1 to 999; got {number}"
                ))
            })
    };

    let strict_value = match get("strict") {
        None | Some(serde_yaml::Value::Null) => defaults.strict,
        Some(serde_yaml::Value::Bool(flag)) => flag,
        Some(other) => {
            return Err(ConfigError::Invalid(format!(
                "plan.strict in {path_display} must be a boolean; got {}",
                render_scalar(&other)
            )));
        }
    };

    // Successor linking kill switch (`docs/task-dependencies.md` §12.8):
    // boolean only; an invalid value fails the load and the caller falls
    // back to `true` through the existing warning paths.
    let link_unblocked_value = match get("link_unblocked") {
        None | Some(serde_yaml::Value::Null) => defaults.link_unblocked,
        Some(serde_yaml::Value::Bool(flag)) => flag,
        Some(other) => {
            return Err(ConfigError::Invalid(format!(
                "plan.link_unblocked in {path_display} must be a boolean; got {}",
                render_scalar(&other)
            )));
        }
    };

    let strings = |name: &str, value: Option<serde_yaml::Value>| {
        let Some(value) = value else {
            return Ok(match name {
                "exempt" => defaults.exempt.clone(),
                _ => defaults.inventory_labels.clone(),
            });
        };
        if value.is_null() {
            return Ok(match name {
                "exempt" => defaults.exempt.clone(),
                _ => defaults.inventory_labels.clone(),
            });
        }
        let serde_yaml::Value::Sequence(items) = value else {
            return Err(ConfigError::Invalid(format!(
                "plan.{name} in {path_display} must hold non-empty strings"
            )));
        };
        items
            .into_iter()
            .map(|value| {
                value
                    .as_str()
                    .map(str::trim)
                    .filter(|text| !text.is_empty())
                    .map(str::to_string)
                    .ok_or_else(|| {
                        ConfigError::Invalid(format!(
                            "plan.{name} in {path_display} must hold non-empty strings"
                        ))
                    })
            })
            .collect::<Result<Vec<_>, _>>()
    };

    // A stale `max_now` stays ignored like any other unknown key.
    Ok(PlanConfig {
        max_themes: cap("max_themes", get("max_themes"), defaults.max_themes)?,
        max_links: cap("max_links", get("max_links"), defaults.max_links)?,
        max_next: cap("max_next", get("max_next"), defaults.max_next)?,
        max_pending: cap(
            "max_pending",
            get("max_pending"),
            defaults.max_pending,
        )?,
        max_ready: cap("max_ready", get("max_ready"), defaults.max_ready)?,
        max_ready_per_note: capped_cap(
            "max_ready_per_note",
            get("max_ready_per_note"),
            defaults.max_ready_per_note,
        )?,
        strict: strict_value,
        link_unblocked: link_unblocked_value,
        exempt: strings("exempt", get("exempt"))?,
        inventory_labels: strings("inventory_labels", get("inventory_labels"))?,
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
    fn missing_plan_file_loads_defaults() {
        let config =
            load_plan_config(Path::new("/definitely/missing/config.yml"))
                .expect("missing file gives defaults");
        assert_eq!(config, PlanConfig::default());
        assert_eq!(config.max_themes(), 3);
        assert_eq!(config.max_links(), 10);
        assert_eq!(config.max_next(), 15);
        assert_eq!(config.max_pending(), 10);
        assert_eq!(config.max_ready(), 100);
        assert_eq!(config.max_ready_per_note(), 5);
        assert!(!config.strict());
        assert_eq!(config.exempt(), ["GTD"]);
        assert_eq!(
            config.inventory_labels(),
            ["LATER", "MISC", "NEW FEATURES", "SASE"]
        );
    }

    #[test]
    fn absent_plan_block_loads_defaults() {
        let config = parse_plan_config(
            "properties: []\nunknown_top_level: ignored\n",
            Path::new("/config.yml"),
        )
        .expect("missing block gives defaults");
        assert_eq!(config, PlanConfig::default());
    }

    #[test]
    fn null_plan_block_loads_defaults() {
        let config = parse_plan_config("plan:\n", Path::new("/config.yml"))
            .expect("null block gives defaults");
        assert_eq!(config, PlanConfig::default());
    }

    #[test]
    fn parses_plan_overrides_and_ignores_unknown_keys() {
        let config = parse_plan_config(
            "unknown_top_level: ignored\n\
            plan:\n\
            \x20 max_themes: 5\n\
            \x20 max_links: 12\n\
            \x20 max_next: 20\n\
            \x20 max_pending: 7\n\
            \x20 max_ready: 42\n\
            \x20 max_ready_per_note: 3\n\
            \x20 max_now: 99\n\
            \x20 strict: true\n\
            \x20 exempt: [GTD, ADMIN]\n\
            \x20 inventory_labels: [LATER]\n\
            \x20 unknown_key: ignored\n",
            Path::new("/config.yml"),
        )
        .expect("valid plan block");
        assert_eq!(config.max_themes(), 5);
        assert_eq!(config.max_links(), 12);
        assert_eq!(config.max_next(), 20);
        assert_eq!(config.max_pending(), 7);
        assert_eq!(config.max_ready(), 42);
        assert_eq!(config.max_ready_per_note(), 3);
        assert!(config.strict());
        assert!(config.link_unblocked());
        assert_eq!(config.exempt(), ["GTD", "ADMIN"]);
        assert_eq!(config.inventory_labels(), ["LATER"]);
    }

    #[test]
    fn rejects_invalid_plan_values() {
        for text in [
            "plan:\n  max_themes: 0\n",
            "plan:\n  max_links: -2\n",
            "plan:\n  max_next: 0\n",
            "plan:\n  max_pending: 0\n",
            "plan:\n  max_ready: 0\n",
            "plan:\n  max_ready: -1\n",
            "plan:\n  max_ready: 1.5\n",
            "plan:\n  max_ready: many\n",
            "plan:\n  max_ready: 4294967296\n",
            "plan:\n  max_ready_per_note: 0\n",
            "plan:\n  max_ready_per_note: 1000\n",
            "plan:\n  max_ready_per_note: -1\n",
            "plan:\n  max_ready_per_note: 1.5\n",
            "plan:\n  max_ready_per_note: many\n",
            "plan:\n  max_ready_per_note: 4294967296\n",
            "plan:\n  max_themes: many\n",
            "plan:\n  strict: \"yes\"\n",
            "plan:\n  link_unblocked: \"yes\"\n",
            "plan:\n  link_unblocked: 1\n",
            "plan:\n  exempt: GTD\n",
            "plan:\n  exempt: [GTD, '']\n",
            "plan:\n  exempt: [GTD, 7]\n",
            "plan:\n  inventory_labels: ['  ']\n",
            "plan:\n  inventory_labels: MISC\n",
            "plan: [1, 2]\n",
        ] {
            let error = parse_plan_config(text, Path::new("/config.yml"))
                .expect_err("invalid plan value must fail");
            assert!(
                matches!(error, ConfigError::Invalid(_)),
                "expected invalid config for {text:?}, got {error:?}"
            );
        }
    }

    #[test]
    fn link_unblocked_defaults_true_and_parses_false() {
        let defaulted = parse_plan_config(
            "plan:\n  max_links: 12\n",
            Path::new("/config.yml"),
        )
        .expect("absent link_unblocked");
        assert!(defaulted.link_unblocked());
        let nulled = parse_plan_config(
            "plan:\n  link_unblocked: null\n",
            Path::new("/config.yml"),
        )
        .expect("null link_unblocked");
        assert!(nulled.link_unblocked());
        let disabled = parse_plan_config(
            "plan:\n  link_unblocked: false\n",
            Path::new("/config.yml"),
        )
        .expect("explicit false");
        assert!(!disabled.link_unblocked());
    }

    #[test]
    fn max_ready_per_note_bounds_and_message() {
        for (text, expected) in [
            ("plan:\n  max_ready_per_note: 1\n", 1),
            ("plan:\n  max_ready_per_note: 5\n", 5),
            ("plan:\n  max_ready_per_note: 999\n", 999),
        ] {
            let config = parse_plan_config(text, Path::new("/config.yml"))
                .expect("boundary cap parses");
            assert_eq!(config.max_ready_per_note(), expected);
        }
        let error = parse_plan_config(
            "plan:\n  max_ready_per_note: 0\n",
            Path::new("/cfg.yml"),
        )
        .expect_err("0 is invalid");
        assert_eq!(
            error.message(),
            "plan.max_ready_per_note in /cfg.yml must be an integer from 1 to 999; got 0"
        );
    }

    #[test]
    fn absent_max_ready_per_note_falls_back_to_default() {
        let config = parse_plan_config(
            "plan:\n  max_ready: 42\n",
            Path::new("/config.yml"),
        )
        .expect("absent per-note cap");
        assert_eq!(config.max_ready_per_note(), 5);
        let nulled = parse_plan_config(
            "plan:\n  max_ready_per_note: null\n",
            Path::new("/config.yml"),
        )
        .expect("null per-note cap");
        assert_eq!(nulled.max_ready_per_note(), 5);
    }

    #[test]
    fn mistyped_plan_block_leaves_other_loaders_working() {
        for text in [
            "properties:\n  - name: priority\n    values: priority\n    schedules: scheduled\n    levels:\n      - label: P1\n        value: high\n        min_days: 1\n        max_days: 1\nplan:\n  max_themes: many\n",
            "properties:\n  - name: priority\n    values: priority\n    schedules: scheduled\n    levels:\n      - label: P1\n        value: high\n        min_days: 1\n        max_days: 1\nplan:\n  strict: \"yes\"\n",
            "properties:\n  - name: priority\n    values: priority\n    schedules: scheduled\n    levels:\n      - label: P1\n        value: high\n        min_days: 1\n        max_days: 1\nplan:\n  exempt: GTD\n",
        ] {
            let path = Path::new("/config.yml");
            super::super::parse_priority_property(text, path)
                .expect("priority loader must ignore a mistyped plan block");
            super::super::parse_highlights_config(text, path)
                .expect("highlights loader must ignore a mistyped plan block");
            super::super::parse_gkeep_config(text, path)
                .expect("gkeep loader must ignore a mistyped plan block");
            let error = parse_plan_config(text, path)
                .expect_err("plan loader must still reject the mistyped block");
            assert!(matches!(error, ConfigError::Invalid(_)));
        }
    }
}
