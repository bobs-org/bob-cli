use crate::native::config::ConfigError;

use super::report::{ErrorBody, StatsErrorEnvelope, SCHEMA_VERSION};

/// Engine failures. Vault I/O is future CLI exit 1; configuration,
/// request/unknown-metric, and non-Dataview format failures are
/// future CLI exit 2.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum StatsError {
    Io { message: String },
    Config { message: String },
    Request { code: &'static str, message: String },
    Format { message: String },
}

impl StatsError {
    pub(crate) fn io(message: impl Into<String>) -> Self {
        Self::Io {
            message: message.into(),
        }
    }

    pub(crate) fn config(message: impl Into<String>) -> Self {
        Self::Config {
            message: message.into(),
        }
    }

    pub(crate) fn unknown_metric(id: &str) -> Self {
        Self::Request {
            code: "unknown_metric",
            message: format!("unknown metric {id:?}"),
        }
    }

    pub(crate) fn usage(message: impl Into<String>) -> Self {
        Self::Request {
            code: "usage",
            message: message.into(),
        }
    }

    pub(crate) fn non_dataview() -> Self {
        Self::Format {
            message: "bob stats needs the vault's Tasks task format to be \
                Dataview (docs/freshness.md rule 1); refusing to run"
                .to_string(),
        }
    }

    pub(crate) fn code(&self) -> &'static str {
        match self {
            Self::Io { .. } => "io",
            Self::Config { .. } => "config",
            Self::Request { code, .. } => code,
            Self::Format { .. } => "format",
        }
    }

    pub(crate) fn message(&self) -> &str {
        match self {
            Self::Io { message }
            | Self::Config { message }
            | Self::Request { message, .. }
            | Self::Format { message } => message,
        }
    }

    pub(crate) fn exit_code(&self) -> i32 {
        match self {
            Self::Io { .. } => 1,
            Self::Config { .. }
            | Self::Request { .. }
            | Self::Format { .. } => 2,
        }
    }

    pub(crate) fn envelope(&self) -> StatsErrorEnvelope {
        StatsErrorEnvelope {
            ok: false,
            schema_version: SCHEMA_VERSION,
            error: ErrorBody {
                code: self.code().to_string(),
                message: self.message().to_string(),
            },
        }
    }
}

impl From<ConfigError> for StatsError {
    fn from(error: ConfigError) -> Self {
        match error {
            ConfigError::Read(message) | ConfigError::Invalid(message) => {
                Self::config(message)
            }
        }
    }
}

impl std::fmt::Display for StatsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn envelope_is_schema_one() {
        let error = StatsError::unknown_metric("nope");
        let json = serde_json::to_value(error.envelope()).unwrap();
        assert_eq!(json["ok"], false);
        assert_eq!(json["schema_version"], 1);
        assert_eq!(json["error"]["code"], "unknown_metric");
        assert_eq!(json["error"]["message"], "unknown metric \"nope\"");
        assert_eq!(error.exit_code(), 2);
    }

    #[test]
    fn io_is_exit_one_config_is_exit_two() {
        assert_eq!(StatsError::io("read vault").exit_code(), 1);
        assert_eq!(StatsError::config("stats.range").exit_code(), 2);
        assert_eq!(StatsError::non_dataview().exit_code(), 2);
        assert_eq!(StatsError::non_dataview().code(), "format");
        let usage = StatsError::usage("stats.range must be a preset");
        assert_eq!(usage.code(), "usage");
        assert_eq!(usage.exit_code(), 2);
    }
}
