//! Resolved `bob gmail` configuration: the `gmail:` config block.

use std::time::Duration;

use crate::native::{config as bob_config, env as bob_env};

use super::GmailError;

pub(crate) const DEFAULT_FOLDER: &str = "mail";
pub(crate) const DEFAULT_LINK_LABEL: &str = "✉️ {subject} · {who}";
pub(crate) const DEFAULT_PULL_INBOX: &str = "gmail_inbox.md";
pub(crate) const DEFAULT_TIMEOUT_SECS: u64 = 60;
pub(crate) const ASSUME_TTY_ENV: &str = "BOB_GMAIL_ASSUME_TTY";
pub(crate) const COMPOSE_SCOPE: &str =
    "https://www.googleapis.com/auth/gmail.compose";

/// Resolved `bob gmail` configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GmailConfig {
    account: Option<String>,
    writer: Option<String>,
    body: bob_config::GmailBodyMode,
    folder: String,
    link_label: String,
    never_save_from: Vec<String>,
    never_save_labels: Vec<String>,
    pull_inbox: String,
    pull_label: Option<String>,
    signature: bob_config::GmailSignature,
    timeout_secs: u64,
}

impl Default for GmailConfig {
    fn default() -> Self {
        Self::from_settings(bob_config::GmailSettings::default())
            .expect("default gmail config resolves")
    }
}

impl GmailConfig {
    /// Resolve the config file, apply defaults, and validate types.
    ///
    /// Every failure is a setup error (exit 2).
    pub(crate) fn resolve() -> Result<Self, GmailError> {
        Self::resolve_at(&bob_config::config_path())
    }

    fn resolve_at(path: &std::path::Path) -> Result<Self, GmailError> {
        let settings =
            bob_config::load_gmail_config(path).map_err(|error| {
                let message = match error {
                    bob_config::ConfigError::Read(message)
                    | bob_config::ConfigError::Invalid(message) => message,
                };
                GmailError::setup(
                    "config",
                    format!("read the gmail config: {message}"),
                )
                .with_hint(
                    "set BOB_CONFIG_FILE or run 'chezmoi apply \
                         ~/.config/bob/config.yml'",
                )
            })?;
        Self::from_settings(settings)
    }

    pub(crate) fn from_settings(
        settings: bob_config::GmailSettings,
    ) -> Result<Self, GmailError> {
        if let Some(account) = settings.account.as_deref()
            && !account.contains('@')
        {
            return Err(GmailError::setup(
                "config",
                format!("gmail.account {account:?} must contain '@'"),
            ));
        }
        let timeout_secs =
            settings.timeout_secs.unwrap_or(DEFAULT_TIMEOUT_SECS);
        if timeout_secs == 0 {
            return Err(GmailError::setup(
                "config",
                "gmail.timeout_secs must be greater than 0".to_string(),
            ));
        }
        let folder = settings
            .folder
            .unwrap_or_else(|| DEFAULT_FOLDER.to_string());
        if folder.trim().is_empty() {
            return Err(GmailError::setup(
                "config",
                "gmail.folder must not be empty".to_string(),
            ));
        }
        if super::note::path::validate_folder(std::path::Path::new(&folder))
            .is_err()
        {
            return Err(GmailError::setup(
                "config",
                "gmail.folder must be a relative path without parent segments"
                    .to_string(),
            ));
        }
        let link_label = settings
            .link_label
            .unwrap_or_else(|| DEFAULT_LINK_LABEL.to_string());
        if !link_label.contains("{subject}") {
            return Err(GmailError::setup(
                "config",
                "gmail.link_label must contain {subject}".to_string(),
            ));
        }
        let pull = settings.pull.unwrap_or_default();
        let pull_inbox =
            pull.inbox.unwrap_or_else(|| DEFAULT_PULL_INBOX.to_string());
        if pull_inbox.trim().is_empty() {
            return Err(GmailError::setup(
                "config",
                "gmail.pull.inbox must not be empty".to_string(),
            ));
        }
        if super::note::path::validate_folder(std::path::Path::new(&pull_inbox))
            .is_err()
        {
            return Err(GmailError::setup(
                "config",
                "gmail.pull.inbox must be a relative path without parent segments"
                    .to_string(),
            ));
        }
        Ok(Self {
            account: settings.account,
            writer: settings.writer,
            body: settings.body.unwrap_or_default(),
            folder,
            link_label,
            never_save_from: settings.never_save.from,
            never_save_labels: settings.never_save.labels,
            pull_inbox,
            pull_label: pull.label,
            signature: settings.signature.unwrap_or_default(),
            timeout_secs,
        })
    }

    pub(crate) fn account(&self) -> Option<&str> {
        self.account.as_deref()
    }

    pub(crate) fn writer(&self) -> Option<&str> {
        self.writer.as_deref()
    }

    pub(crate) fn body(&self) -> bob_config::GmailBodyMode {
        self.body
    }

    pub(crate) fn folder(&self) -> &str {
        &self.folder
    }

    // Consumed by mail-note / save / pull-attach; parsed in full by shell.
    #[allow(dead_code)]
    pub(crate) fn link_label(&self) -> &str {
        &self.link_label
    }

    #[allow(dead_code)]
    pub(crate) fn never_save_from(&self) -> &[String] {
        &self.never_save_from
    }

    #[allow(dead_code)]
    pub(crate) fn never_save_labels(&self) -> &[String] {
        &self.never_save_labels
    }

    #[allow(dead_code)]
    pub(crate) fn pull_inbox(&self) -> &str {
        &self.pull_inbox
    }

    pub(crate) fn pull_label(&self) -> Option<&str> {
        self.pull_label.as_deref()
    }

    pub(crate) fn signature(&self) -> bob_config::GmailSignature {
        self.signature
    }

    pub(crate) fn timeout(&self) -> Duration {
        Duration::from_secs(self.timeout_secs)
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

    fn settings() -> bob_config::GmailSettings {
        bob_config::GmailSettings::default()
    }

    #[test]
    fn defaults_when_block_is_missing() {
        let config = GmailConfig::from_settings(settings()).expect("defaults");
        assert_eq!(config.folder(), "mail");
        assert_eq!(config.link_label(), "✉️ {subject} · {who}");
        assert_eq!(config.body(), bob_config::GmailBodyMode::Full);
        assert_eq!(config.pull_inbox(), "gmail_inbox.md");
        assert_eq!(config.pull_label(), Some("bob"));
        assert_eq!(config.timeout(), Duration::from_secs(60));
    }

    #[test]
    fn rejects_account_without_at() {
        let error = GmailConfig::from_settings(bob_config::GmailSettings {
            account: Some("not-an-email".to_string()),
            ..settings()
        })
        .expect_err("bad email");
        assert_eq!(error.exit_code(), 2);
        assert!(error.message().contains("gmail.account"));
    }

    #[test]
    fn body_and_signature_resolve() {
        let config = GmailConfig::from_settings(bob_config::GmailSettings {
            body: Some(bob_config::GmailBodyMode::Headers),
            signature: Some(bob_config::GmailSignature::None),
            ..settings()
        })
        .expect("explicit modes");
        assert_eq!(config.body(), bob_config::GmailBodyMode::Headers);
        assert_eq!(config.signature(), bob_config::GmailSignature::None);
    }

    #[test]
    fn link_label_needs_subject() {
        let error = GmailConfig::from_settings(bob_config::GmailSettings {
            link_label: Some("no placeholder".to_string()),
            ..settings()
        })
        .expect_err("label without subject");
        assert!(error.message().contains("gmail.link_label"));
    }

    #[test]
    fn folder_must_be_relative_and_stay_inside_the_vault() {
        for folder in ["/mail", "mail/../outside"] {
            let error = GmailConfig::from_settings(bob_config::GmailSettings {
                folder: Some(folder.to_string()),
                ..settings()
            })
            .expect_err("invalid folder");
            assert_eq!(error.exit_code(), 2);
            assert_eq!(error.kind(), "config");
        }
    }
}
