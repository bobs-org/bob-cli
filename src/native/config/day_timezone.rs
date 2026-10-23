use std::path::Path;

use chrono_tz::Tz;
use serde::Deserialize;

use super::ConfigError;

#[derive(Debug, Deserialize)]
struct RawDayTimezone {
    #[serde(default)]
    timezone: Option<serde_yaml::Value>,
}

pub(crate) fn load_day_timezone(
    path: &Path,
) -> Result<Option<Tz>, ConfigError> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(None);
        }
        Err(error) => {
            return Err(ConfigError::Read(format!(
                "read {}: {error}",
                path.display()
            )));
        }
    };
    parse_day_timezone(&text, path)
}

fn parse_day_timezone(
    text: &str,
    path: &Path,
) -> Result<Option<Tz>, ConfigError> {
    // The timezone is a shared, optional setting, but several commands
    // intentionally tolerate unrelated broken config sections (for example,
    // capture's URL routing fallback and gkeep's vault-only source). If the
    // document cannot be parsed at all, there is no readable timezone value
    // to validate; preserve those command-specific config behaviors. A
    // syntactically valid but invalid `timezone:` value still returns a clear
    // config error below.
    let Ok(config) = serde_yaml::from_str::<RawDayTimezone>(text) else {
        return Ok(None);
    };

    let Some(value) = config.timezone else {
        return Ok(None);
    };
    let Some(value) = value.as_str() else {
        return Err(ConfigError::Invalid(format!(
            "timezone in {} must be an IANA timezone name",
            path.display()
        )));
    };
    let value = value.trim();
    if value.is_empty() {
        return Err(ConfigError::Invalid(format!(
            "timezone in {} must be a nonempty IANA timezone name",
            path.display()
        )));
    }

    value.parse::<Tz>().map(Some).map_err(|_| {
        ConfigError::Invalid(format!(
            "timezone in {} is not a valid IANA timezone: {value:?}",
            path.display()
        ))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> Result<Option<Tz>, ConfigError> {
        parse_day_timezone(text, Path::new("/config.yml"))
    }

    #[test]
    fn timezone_is_optional_and_null_means_unset() {
        assert_eq!(parse("plan: {}\n").unwrap(), None);
        assert_eq!(parse("timezone: null\n").unwrap(), None);
    }

    #[test]
    fn parses_valid_iana_timezone_names() {
        assert_eq!(
            parse("timezone: America/New_York\n").unwrap(),
            Some(chrono_tz::America::New_York)
        );
    }

    #[test]
    fn invalid_timezone_has_a_clear_config_error() {
        let error = parse("timezone: Mars/Olympus\n").unwrap_err();
        let message = error.to_string();
        assert!(message.contains("timezone"));
        assert!(message.contains("Mars/Olympus"));
        assert!(message.contains("/config.yml"));
    }

    #[test]
    fn rejects_non_string_and_empty_timezone_values() {
        assert!(parse("timezone: 42\n")
            .unwrap_err()
            .to_string()
            .contains("IANA timezone name"));
        assert!(parse("timezone: ''\n")
            .unwrap_err()
            .to_string()
            .contains("nonempty"));
    }
}
