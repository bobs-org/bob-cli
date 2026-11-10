#![allow(dead_code)]

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use chrono::NaiveDate;

use super::ConfigError;
use crate::native::stats::range::{parse_iso_date, RangeSpec};

/// Isolated `stats:` block. Metric entries stay raw so one bad
/// definition cannot fail the load.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StatsConfig {
    pub(crate) range: RangeSpec,
    pub(crate) overview: Vec<String>,
    pub(crate) hide: Vec<String>,
    pub(crate) since: Option<NaiveDate>,
    pub(crate) user_metrics: Vec<(String, serde_yaml::Value)>,
    pub(crate) period_strip: PeriodStripConfig,
    pub(crate) path: PathBuf,
}

/// `stats.period_strip` for period-note charts. Invalid `top_roots`
/// never fails the load: the value is clamped or defaulted and a
/// warning is attached for `bob periods show`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PeriodStripConfig {
    pub(crate) top_roots: u8,
    pub(crate) top_roots_warning: Option<String>,
}

pub(crate) const DEFAULT_PERIOD_STRIP_TOP_ROOTS: u8 = 5;
pub(crate) const MIN_PERIOD_STRIP_TOP_ROOTS: u8 = 1;
pub(crate) const MAX_PERIOD_STRIP_TOP_ROOTS: u8 = 8;

impl Default for PeriodStripConfig {
    fn default() -> Self {
        Self {
            top_roots: DEFAULT_PERIOD_STRIP_TOP_ROOTS,
            top_roots_warning: None,
        }
    }
}

impl Default for StatsConfig {
    fn default() -> Self {
        Self {
            range: RangeSpec::four_weeks(),
            overview: default_overview(),
            hide: Vec::new(),
            since: None,
            user_metrics: Vec::new(),
            period_strip: PeriodStripConfig::default(),
            path: PathBuf::new(),
        }
    }
}

pub(crate) fn default_overview() -> Vec<String> {
    [
        "worked",
        "reviewed",
        "done",
        "created",
        "net",
        "review_closed",
    ]
    .into_iter()
    .map(str::to_string)
    .collect()
}

pub(crate) fn load_stats_config(
    path: &Path,
) -> Result<StatsConfig, ConfigError> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(StatsConfig {
                path: path.to_path_buf(),
                ..StatsConfig::default()
            });
        }
        Err(error) => {
            return Err(ConfigError::Read(format!(
                "read {}: {error}",
                path.display()
            )));
        }
    };
    parse_stats_config(&text, path)
}

pub(crate) fn parse_stats_config(
    text: &str,
    path: &Path,
) -> Result<StatsConfig, ConfigError> {
    let config: super::RawConfig =
        serde_yaml::from_str(text).map_err(|error| {
            ConfigError::Invalid(format!("parse {}: {error}", path.display()))
        })?;
    parse_stats_value(config.stats, path)
}

fn parse_stats_value(
    stats: Option<serde_yaml::Value>,
    path: &Path,
) -> Result<StatsConfig, ConfigError> {
    let path_display = path.display();
    let mut parsed = StatsConfig {
        path: path.to_path_buf(),
        ..StatsConfig::default()
    };

    let Some(stats) = stats else {
        return Ok(parsed);
    };
    if stats.is_null() {
        return Ok(parsed);
    }
    let serde_yaml::Value::Mapping(mapping) = stats else {
        return Err(ConfigError::Invalid(format!(
            "stats in {path_display} must be a mapping"
        )));
    };

    parsed.range = parse_range(mapping_get(&mapping, "range"), path)?;
    parsed.overview = parse_id_list(
        mapping_get(&mapping, "overview"),
        "overview",
        default_overview(),
        path,
    )?;
    parsed.hide =
        parse_id_list(mapping_get(&mapping, "hide"), "hide", Vec::new(), path)?;
    parsed.since = parse_since(mapping_get(&mapping, "since"), path)?;
    parsed.user_metrics =
        parse_metrics_container(mapping_get(&mapping, "metrics"), path)?;
    parsed.period_strip =
        parse_period_strip(mapping_get(&mapping, "period_strip"), path);
    Ok(parsed)
}

fn parse_period_strip(
    value: Option<&serde_yaml::Value>,
    path: &Path,
) -> PeriodStripConfig {
    match value {
        None | Some(serde_yaml::Value::Null) => PeriodStripConfig::default(),
        Some(serde_yaml::Value::Mapping(mapping)) => {
            parse_top_roots(mapping_get(mapping, "top_roots"), path)
        }
        Some(other) => PeriodStripConfig {
            top_roots: DEFAULT_PERIOD_STRIP_TOP_ROOTS,
            top_roots_warning: Some(top_roots_invalid_message(
                path,
                &render_scalar(other),
            )),
        },
    }
}

fn parse_top_roots(
    value: Option<&serde_yaml::Value>,
    path: &Path,
) -> PeriodStripConfig {
    match value {
        None | Some(serde_yaml::Value::Null) => PeriodStripConfig::default(),
        Some(serde_yaml::Value::Number(number)) => match number.as_i64() {
            Some(raw) => clamp_top_roots(raw, path),
            None => PeriodStripConfig {
                top_roots: DEFAULT_PERIOD_STRIP_TOP_ROOTS,
                top_roots_warning: Some(top_roots_invalid_message(
                    path,
                    &number.to_string(),
                )),
            },
        },
        Some(other) => PeriodStripConfig {
            top_roots: DEFAULT_PERIOD_STRIP_TOP_ROOTS,
            top_roots_warning: Some(top_roots_invalid_message(
                path,
                &render_scalar(other),
            )),
        },
    }
}

fn clamp_top_roots(raw: i64, path: &Path) -> PeriodStripConfig {
    let min = i64::from(MIN_PERIOD_STRIP_TOP_ROOTS);
    let max = i64::from(MAX_PERIOD_STRIP_TOP_ROOTS);
    if (min..=max).contains(&raw) {
        return PeriodStripConfig {
            top_roots: raw as u8,
            top_roots_warning: None,
        };
    }
    let clamped = raw.clamp(min, max) as u8;
    PeriodStripConfig {
        top_roots: clamped,
        top_roots_warning: Some(format!(
            "stats.period_strip.top_roots in {} is {raw}; using {clamped}",
            path.display()
        )),
    }
}

fn top_roots_invalid_message(path: &Path, got: &str) -> String {
    format!(
        "stats.period_strip.top_roots in {} must be an integer from {} to {}; got {got}",
        path.display(),
        MIN_PERIOD_STRIP_TOP_ROOTS,
        MAX_PERIOD_STRIP_TOP_ROOTS
    )
}

fn parse_range(
    value: Option<&serde_yaml::Value>,
    path: &Path,
) -> Result<RangeSpec, ConfigError> {
    match value {
        None | Some(serde_yaml::Value::Null) => Ok(RangeSpec::four_weeks()),
        Some(serde_yaml::Value::String(text)) => RangeSpec::parse(text)
            .map_err(|error| {
                ConfigError::Invalid(format!(
                    "stats.range in {} {error}",
                    path.display()
                ))
            }),
        Some(other) => Err(ConfigError::Invalid(format!(
            "stats.range in {} must be a string; got {}",
            path.display(),
            render_scalar(other)
        ))),
    }
}

fn parse_since(
    value: Option<&serde_yaml::Value>,
    path: &Path,
) -> Result<Option<NaiveDate>, ConfigError> {
    match value {
        None | Some(serde_yaml::Value::Null) => Ok(None),
        Some(serde_yaml::Value::String(text)) => {
            parse_iso_date(text).map(Some).ok_or_else(|| {
                ConfigError::Invalid(format!(
                    "stats.since in {} must be YYYY-MM-DD; got {text:?}",
                    path.display()
                ))
            })
        }
        Some(other) => Err(ConfigError::Invalid(format!(
            "stats.since in {} must be YYYY-MM-DD; got {}",
            path.display(),
            render_scalar(other)
        ))),
    }
}

fn parse_id_list(
    value: Option<&serde_yaml::Value>,
    key: &str,
    fallback: Vec<String>,
    path: &Path,
) -> Result<Vec<String>, ConfigError> {
    let Some(value) = value else {
        return Ok(fallback);
    };
    if value.is_null() {
        return Ok(fallback);
    }
    let serde_yaml::Value::Sequence(items) = value else {
        return Err(ConfigError::Invalid(format!(
            "stats.{key} in {} must hold non-empty strings",
            path.display()
        )));
    };
    let ids = items
        .iter()
        .map(|item| {
            item.as_str()
                .map(str::trim)
                .filter(|text| !text.is_empty())
                .map(str::to_string)
                .ok_or_else(|| {
                    ConfigError::Invalid(format!(
                        "stats.{key} in {} must hold non-empty strings",
                        path.display()
                    ))
                })
        })
        .collect::<Result<Vec<_>, _>>()?;
    let mut seen = HashSet::new();
    Ok(ids
        .into_iter()
        .filter(|id| seen.insert(id.clone()))
        .collect())
}

fn parse_metrics_container(
    value: Option<&serde_yaml::Value>,
    path: &Path,
) -> Result<Vec<(String, serde_yaml::Value)>, ConfigError> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    if value.is_null() {
        return Ok(Vec::new());
    }
    let serde_yaml::Value::Mapping(mapping) = value else {
        return Err(ConfigError::Invalid(format!(
            "stats.metrics in {} must be a mapping",
            path.display()
        )));
    };
    Ok(mapping
        .iter()
        .map(|(key, entry)| {
            let id = key
                .as_str()
                .map(str::to_string)
                .unwrap_or_else(|| render_scalar(key));
            (id, entry.clone())
        })
        .collect())
}

fn mapping_get<'a>(
    mapping: &'a serde_yaml::Mapping,
    key: &str,
) -> Option<&'a serde_yaml::Value> {
    mapping.iter().find_map(|(candidate, value)| {
        candidate
            .as_str()
            .filter(|name| *name == key)
            .map(|_| value)
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
    use crate::native::config::{
        load_freshness_config, load_plan_config, parse_gkeep_config,
        parse_highlights_config, parse_priority_property,
    };

    fn parse(text: &str) -> StatsConfig {
        parse_stats_config(text, Path::new("/config.yml"))
            .expect("valid stats config")
    }

    fn parse_err(text: &str) -> String {
        match parse_stats_config(text, Path::new("/config.yml")) {
            Err(ConfigError::Invalid(message) | ConfigError::Read(message)) => {
                message
            }
            Ok(_) => panic!("expected invalid stats config for {text:?}"),
        }
    }

    #[test]
    fn missing_file_and_absent_block_use_defaults() {
        let missing =
            load_stats_config(Path::new("/definitely/missing/config.yml"))
                .expect("missing file");
        assert_eq!(missing.range, RangeSpec::four_weeks());
        assert_eq!(
            missing.overview,
            [
                "worked",
                "reviewed",
                "done",
                "created",
                "net",
                "review_closed"
            ]
        );
        assert!(missing.hide.is_empty());
        assert!(missing.since.is_none());
        assert!(missing.user_metrics.is_empty());
        assert_eq!(
            missing.period_strip.top_roots,
            DEFAULT_PERIOD_STRIP_TOP_ROOTS
        );
        assert!(missing.period_strip.top_roots_warning.is_none());

        let absent = parse("properties: []\nunknown_top_level: ignored\n");
        assert_eq!(absent.range, RangeSpec::four_weeks());
        let nulled = parse("stats:\n");
        assert_eq!(nulled.range, RangeSpec::four_weeks());
    }

    #[test]
    fn parses_overrides_and_ignores_unknown_keys() {
        let config = parse(
            "
unknown_top_level: ignored
stats:
  range: 7d
  overview: [worked, done]
  hide: [sessions]
  since: 2026-06-01
  bob_path: ~/.cargo/bin/bob
  future_key: ignored
  metrics:
    sase_time:
      label: SASE time
      kind: pomodoro
      themes: [SASE]
",
        );
        assert_eq!(config.range, RangeSpec::Days(7));
        assert_eq!(config.overview, ["worked", "done"]);
        assert_eq!(config.hide, ["sessions"]);
        assert_eq!(
            config.since,
            Some(NaiveDate::from_ymd_opt(2026, 6, 1).unwrap())
        );
        assert_eq!(config.user_metrics.len(), 1);
        assert_eq!(config.user_metrics[0].0, "sase_time");
        assert_eq!(
            config.period_strip.top_roots,
            DEFAULT_PERIOD_STRIP_TOP_ROOTS
        );
        assert!(config.period_strip.top_roots_warning.is_none());
    }

    #[test]
    fn period_strip_top_roots_defaults_clamps_and_warns_without_failing() {
        let absent = parse("stats:\n  range: 7d\n");
        assert_eq!(
            absent.period_strip.top_roots,
            DEFAULT_PERIOD_STRIP_TOP_ROOTS
        );
        assert!(absent.period_strip.top_roots_warning.is_none());

        let nulled = parse("stats:\n  period_strip:\n    top_roots:\n");
        assert_eq!(
            nulled.period_strip.top_roots,
            DEFAULT_PERIOD_STRIP_TOP_ROOTS
        );
        assert!(nulled.period_strip.top_roots_warning.is_none());

        let five = parse("stats:\n  period_strip:\n    top_roots: 5\n");
        assert_eq!(five.period_strip.top_roots, 5);
        assert!(five.period_strip.top_roots_warning.is_none());

        let one = parse("stats:\n  period_strip:\n    top_roots: 1\n");
        assert_eq!(one.period_strip.top_roots, 1);

        let eight = parse("stats:\n  period_strip:\n    top_roots: 8\n");
        assert_eq!(eight.period_strip.top_roots, 8);

        let zero = parse("stats:\n  period_strip:\n    top_roots: 0\n");
        assert_eq!(zero.period_strip.top_roots, MIN_PERIOD_STRIP_TOP_ROOTS);
        let zero_warning =
            zero.period_strip.top_roots_warning.as_deref().unwrap();
        assert!(zero_warning.contains("stats.period_strip.top_roots"));
        assert!(zero_warning.contains("/config.yml"));
        assert!(zero_warning.contains("0"));

        let twelve = parse("stats:\n  period_strip:\n    top_roots: 12\n");
        assert_eq!(twelve.period_strip.top_roots, MAX_PERIOD_STRIP_TOP_ROOTS);
        let twelve_warning =
            twelve.period_strip.top_roots_warning.as_deref().unwrap();
        assert!(twelve_warning.contains("12"));
        assert!(twelve_warning.contains("/config.yml"));

        let five_word = parse("stats:\n  period_strip:\n    top_roots: five\n");
        assert_eq!(
            five_word.period_strip.top_roots,
            DEFAULT_PERIOD_STRIP_TOP_ROOTS
        );
        let word_warning =
            five_word.period_strip.top_roots_warning.as_deref().unwrap();
        assert!(word_warning.contains("five"));
        assert!(word_warning.contains("/config.yml"));

        let not_a_map = parse("stats:\n  period_strip: 5\n");
        assert_eq!(
            not_a_map.period_strip.top_roots,
            DEFAULT_PERIOD_STRIP_TOP_ROOTS
        );
        assert!(not_a_map.period_strip.top_roots_warning.is_some());
    }

    #[test]
    fn rejects_invalid_top_level_values() {
        for text in [
            "stats: [1, 2]\n",
            "stats:\n  range: 0d\n",
            "stats:\n  range: 4\n",
            "stats:\n  range: yesterday\n",
            "stats:\n  overview: worked\n",
            "stats:\n  overview: [worked, '']\n",
            "stats:\n  hide: sessions\n",
            "stats:\n  hide: [sessions, 7]\n",
            "stats:\n  since: June\n",
            "stats:\n  since: 2026-13-01\n",
            "stats:\n  metrics: []\n",
            "stats:\n  metrics: sase_time\n",
        ] {
            let message = parse_err(text);
            assert!(
                message.contains("stats"),
                "error must name stats: {message}"
            );
            assert!(
                message.contains("/config.yml"),
                "error must name the path: {message}"
            );
        }
    }

    #[test]
    fn bad_stats_block_leaves_other_loaders_working() {
        let text = "
properties:
  - name: priority
    values: priority
    schedules: scheduled
    levels:
      - label: P1
        value: high
        min_days: 1
        max_days: 1
plan:
  max_links: 5
freshness:
  interval: 7
highlights:
  listen_command: echo
gkeep:
  target: inbox.md
stats:
  range: 0d
";
        let path = Path::new("/config.yml");
        parse_priority_property(text, path)
            .expect("priority loader ignores stats");
        parse_highlights_config(text, path)
            .expect("highlights loader ignores stats");
        parse_gkeep_config(text, path).expect("gkeep loader ignores stats");

        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("config.yml");
        std::fs::write(&file, text).unwrap();
        let plan = load_plan_config(&file).expect("plan loader ignores stats");
        assert_eq!(plan.max_links(), 5);
        let freshness =
            load_freshness_config(&file).expect("freshness ignores stats");
        assert_eq!(freshness.interval(), 7);

        let error = parse_stats_config(text, path)
            .expect_err("stats loader still rejects the block");
        assert!(matches!(error, ConfigError::Invalid(_)));
    }

    #[test]
    fn malformed_metric_entries_do_not_fail_the_load() {
        let config = parse(
            "
stats:
  metrics:
    bad: not-a-mapping
    also_bad: 7
    sase_time: { label: SASE time, kind: pomodoro, themes: [SASE] }
",
        );
        assert_eq!(config.user_metrics.len(), 3);
    }

    #[test]
    fn duplicate_overview_and_hide_names_are_deduped() {
        let config = parse(
            "
stats:
  overview: [worked, done, worked]
  hide: [sessions, sessions]
",
        );
        assert_eq!(config.overview, ["worked", "done"]);
        assert_eq!(config.hide, ["sessions"]);
    }

    #[test]
    fn non_string_metric_key_is_stringified() {
        let config = parse(
            "
stats:
  metrics:
    2026: { label: Year, kind: formula, expr: 1 }
",
        );
        assert_eq!(config.user_metrics.len(), 1);
        assert_eq!(config.user_metrics[0].0, "2026");
    }
}
