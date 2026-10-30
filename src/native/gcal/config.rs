//! Resolved `bob gcal` configuration: the `gcal:` config section.

use std::{collections::BTreeMap, path::Path, time::Duration};

use crate::native::{config as bob_config, env as bob_env};

use super::GcalError;

pub(crate) const DEFAULT_ROOT: &str = "meet";
pub(crate) const DEFAULT_TIMEOUT_SECS: u64 = 60;
pub(crate) const GOG_ENV_VAR: &str = "BOB_GCAL_GOG";
pub(crate) const ASSUME_TTY_ENV: &str = "BOB_GCAL_ASSUME_TTY";

/// Resolved `bob gcal` configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GcalConfig {
    account: Option<String>,
    writer: Option<String>,
    root: String,
    calendars: Vec<bob_config::GcalCalendarSettings>,
    #[allow(dead_code)]
    colors: BTreeMap<String, String>,
    #[allow(dead_code)]
    tentative: bob_config::GcalTentative,
    timeout_secs: u64,
}

impl GcalConfig {
    /// Resolve the config file, apply defaults, and validate types.
    ///
    /// Every failure is a setup error (exit 2).
    pub(crate) fn resolve() -> Result<Self, GcalError> {
        Self::resolve_at(&bob_config::config_path())
    }

    fn resolve_at(path: &Path) -> Result<Self, GcalError> {
        let settings = bob_config::load_gcal_config(path).map_err(|error| {
            let message = match error {
                bob_config::ConfigError::Read(message)
                | bob_config::ConfigError::Invalid(message) => message,
            };
            GcalError::setup(
                "config",
                format!("read the gcal config: {message}"),
            )
            .with_hint(
                "set BOB_CONFIG_FILE or run 'chezmoi apply \
                         ~/.config/bob/config.yml'",
            )
        })?;
        Self::from_settings(settings)
    }

    pub(crate) fn from_settings(
        settings: bob_config::GcalSettings,
    ) -> Result<Self, GcalError> {
        if let Some(account) = settings.account.as_deref()
            && !account.contains('@')
        {
            return Err(GcalError::setup(
                "config",
                format!("gcal.account {account:?} must contain '@'"),
            ));
        }
        let timeout_secs =
            settings.timeout_secs.unwrap_or(DEFAULT_TIMEOUT_SECS);
        if timeout_secs == 0 {
            return Err(GcalError::setup(
                "config",
                "gcal.timeout_secs must be greater than 0".to_string(),
            ));
        }
        let calendars = settings.calendars.unwrap_or_else(|| {
            vec![bob_config::GcalCalendarSettings {
                id: "primary".to_string(),
                root: None,
            }]
        });
        if calendars.is_empty() {
            return Err(GcalError::setup(
                "config",
                "gcal.calendars must not be empty".to_string(),
            ));
        }
        Ok(Self {
            account: settings.account,
            writer: settings.writer,
            root: settings.root.unwrap_or_else(|| DEFAULT_ROOT.to_string()),
            calendars,
            colors: settings.colors,
            tentative: settings
                .tentative
                .unwrap_or(bob_config::GcalTentative::Context),
            timeout_secs,
        })
    }

    pub(crate) fn account(&self) -> Option<&str> {
        self.account.as_deref()
    }

    pub(crate) fn writer(&self) -> Option<&str> {
        self.writer.as_deref()
    }

    pub(crate) fn root(&self) -> &str {
        &self.root
    }

    pub(crate) fn calendars(&self) -> &[bob_config::GcalCalendarSettings] {
        &self.calendars
    }

    #[allow(dead_code)]
    pub(crate) fn colors(&self) -> &BTreeMap<String, String> {
        &self.colors
    }

    #[allow(dead_code)]
    pub(crate) fn tentative(&self) -> bob_config::GcalTentative {
        self.tentative
    }

    pub(crate) fn timeout(&self) -> Duration {
        Duration::from_secs(self.timeout_secs)
    }

    /// Calendar root: per-calendar override, else the default root.
    pub(crate) fn root_for(&self, calendar_id: &str) -> &str {
        self.calendars
            .iter()
            .find(|calendar| calendar.id == calendar_id)
            .and_then(|calendar| calendar.root.as_deref())
            .unwrap_or(&self.root)
    }

    /// Configured per-calendar override, without falling through to the default.
    pub(crate) fn calendar_root(&self, calendar_id: &str) -> Option<&str> {
        self.calendars
            .iter()
            .find(|calendar| calendar.id == calendar_id)
            .and_then(|calendar| calendar.root.as_deref())
    }

    /// Root mapped to a Google event colour id, if configured.
    pub(crate) fn color_root(&self, color_id: &str) -> Option<&str> {
        self.colors.iter().find_map(|(key, root)| {
            (bob_config::color_id_for_key(key) == Some(color_id))
                .then_some(root.as_str())
        })
    }

    /// Whether `calendar_id` is in the configured sync set.
    ///
    /// `primary` matches the configured id `primary`.
    pub(crate) fn syncs(&self, calendar_id: &str, primary: bool) -> bool {
        self.calendars.iter().any(|calendar| {
            calendar.id == calendar_id
                || (calendar.id == "primary"
                    && (calendar_id == "primary" || primary))
        })
    }

    /// The test hook: an executable replacing `gog` on PATH.
    pub(crate) fn gog_override() -> Option<std::path::PathBuf> {
        bob_env::var_os(GOG_ENV_VAR)
            .filter(|value| !value.is_empty())
            .map(std::path::PathBuf::from)
    }
}

/// Whether stdin should be treated as a TTY for login prompts.
pub(crate) fn assume_tty() -> bool {
    matches!(
        bob_env::var(ASSUME_TTY_ENV).ok().as_deref(),
        Some("1") | Some("true")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_when_block_is_missing() {
        let config = GcalConfig::from_settings(bob_config::GcalSettings {
            account: None,
            writer: None,
            root: None,
            calendars: None,
            colors: BTreeMap::new(),
            tentative: None,
            timeout_secs: None,
        })
        .expect("defaults");
        assert_eq!(config.root(), "meet");
        assert_eq!(config.calendars().len(), 1);
        assert_eq!(config.calendars()[0].id, "primary");
        assert_eq!(config.timeout(), Duration::from_secs(60));
        assert!(config.syncs("primary", true));
        assert!(config.syncs("you@example.com", true));
        assert!(!config.syncs("work", false));
    }

    #[test]
    fn rejects_account_without_at() {
        let error = GcalConfig::from_settings(bob_config::GcalSettings {
            account: Some("not-an-email".to_string()),
            writer: None,
            root: None,
            calendars: None,
            colors: BTreeMap::new(),
            tentative: None,
            timeout_secs: None,
        })
        .expect_err("bad email");
        assert_eq!(error.exit_code(), 2);
        assert!(error.message().contains("gcal.account"));
    }
}
