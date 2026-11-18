use super::catalog::STEPS;
use crate::native::config::{config_path, RawConfig};
use serde_yaml::Value;
use std::{collections::BTreeMap, fs, io};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Cadence {
    Interval(i64),
    Manual,
    Off,
}

impl std::fmt::Display for Cadence {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Manual => write!(f, "manual"),
            Self::Off => write!(f, "off"),
            Self::Interval(n) if n % 1440 == 0 => write!(f, "{}d", n / 1440),
            Self::Interval(n) if n % 60 == 0 => write!(f, "{}h", n / 60),
            Self::Interval(n) => write!(f, "{n}m"),
        }
    }
}

pub(super) struct Config {
    pub writer: Option<String>,
    pub steps: BTreeMap<String, Cadence>,
}
impl Config {
    pub fn load() -> Result<Self, String> {
        let path = config_path();
        let text = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(e) if e.kind() == io::ErrorKind::NotFound => String::new(),
            Err(e) => return Err(format!("read {}: {e}", path.display())),
        };
        Self::parse(&text)
    }
    pub fn parse(text: &str) -> Result<Self, String> {
        let raw: RawConfig =
            serde_yaml::from_str(text).map_err(|e| e.to_string())?;
        let mut config = Self {
            writer: None,
            steps: STEPS.iter().map(|s| (s.name.into(), s.default)).collect(),
        };
        if let Some(value) = raw.tick {
            let map = value.as_mapping().ok_or("tick: expected a mapping")?;
            for (key, value) in map {
                match key.as_str() {
                    Some("writer") => {
                        config.writer = Some(writer(value, "tick.writer")?)
                    }
                    Some("steps") => {
                        let steps = value
                            .as_mapping()
                            .ok_or("tick.steps: expected a mapping")?;
                        for (key, value) in steps {
                            let name = key
                                .as_str()
                                .ok_or("tick.steps: expected a step name")?;
                            if !config.steps.contains_key(name) {
                                return Err(format!(
                                    "tick.steps.{name}: unknown step"
                                ));
                            }
                            config.steps.insert(
                                name.into(),
                                cadence(value).map_err(|e| {
                                    format!("tick.steps.{name}: {e}")
                                })?,
                            );
                        }
                    }
                    Some(key) => {
                        return Err(format!("tick.{key}: unknown key"))
                    }
                    None => return Err("tick: expected a string key".into()),
                }
            }
        }
        if config.writer.is_none()
            && let Some(value) = raw.gcal.as_ref().and_then(|v| v.get("writer"))
            && !value.is_null()
        {
            config.writer = Some(writer(value, "gcal.writer")?);
        }
        Ok(config)
    }
}
fn writer(value: &Value, key: &str) -> Result<String, String> {
    value
        .as_str()
        .filter(|s| !s.trim().is_empty())
        .map(|s| s.trim().into())
        .ok_or_else(|| format!("{key}: expected a nonempty host label"))
}
fn cadence(value: &Value) -> Result<Cadence, String> {
    if value == &Value::Bool(false) {
        return Ok(Cadence::Off);
    }
    let invalid = || {
        format!(
            "{value:?} is not an interval; use 1h, manual, or off (5m to 7d)"
        )
    };
    let text = value.as_str().ok_or_else(invalid)?;
    match text {
        "manual" => return Ok(Cadence::Manual),
        "off" => return Ok(Cadence::Off),
        _ => {}
    }
    let multiplier = match text.as_bytes().last() {
        Some(b'm') => 1,
        Some(b'h') => 60,
        Some(b'd') => 1440,
        _ => return Err(invalid()),
    };
    let digits = &text[..text.len() - 1];
    if digits.is_empty() || !digits.bytes().all(|c| c.is_ascii_digit()) {
        return Err(invalid());
    }
    let minutes = digits
        .parse::<i64>()
        .ok()
        .and_then(|n| n.checked_mul(multiplier))
        .filter(|n| (5..=10080).contains(n))
        .ok_or_else(invalid)?;
    Ok(Cadence::Interval(minutes))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn defaults_and_partial_overrides() {
        for text in ["", "tick: {}", "tick: {steps: {}}"] {
            let c = Config::parse(text).unwrap();
            assert_eq!(c.steps["gcal"], Cadence::Interval(60));
            assert_eq!(c.steps["gkeep"], Cadence::Manual);
        }
        let c = Config::parse(
            "gcal: {writer: mac}\ntick: {steps: {tasks: false, refs: 7d}}",
        )
        .unwrap();
        assert_eq!(c.writer.as_deref(), Some("mac"));
        assert_eq!(c.steps["tasks"], Cadence::Off);
        assert_eq!(c.steps["refs"], Cadence::Interval(10080));
        assert_eq!(
            Config::parse("gcal: {writer: mac}\ntick: {writer: athena}")
                .unwrap()
                .writer
                .as_deref(),
            Some("athena")
        );
    }
    #[test]
    fn strict_keys_and_grammar() {
        for value in [
            "1hr",
            "0m",
            "8d",
            "4m",
            "true",
            "15",
            "[]",
            "{}",
            "null",
            "1.5h",
            "-5m",
            "+5m",
            "999999999999999999d",
        ] {
            let text = format!("tick: {{steps: {{gcal: {value}}}}}");
            assert!(
                Config::parse(&text)
                    .err()
                    .unwrap()
                    .contains("tick.steps.gcal"),
                "{value}"
            );
        }
        for (text, key) in [
            ("tick: {order: []}", "tick.order"),
            ("tick: {steps: {typo: 1h}}", "tick.steps.typo"),
            ("tick: []", "tick"),
            ("tick: {writer: false}", "tick.writer"),
        ] {
            assert!(Config::parse(text).err().unwrap().contains(key));
        }
        for value in [
            "5m", "60m", "1h", "24h", "1d", "7d", "manual", "off", "false",
        ] {
            assert!(Config::parse(&format!(
                "tick: {{steps: {{gcal: {value}}}}}"
            ))
            .is_ok());
        }
    }
}
