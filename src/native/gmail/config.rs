//! Resolved `bob gmail` configuration: the `gmail:` config block.

use std::time::Duration;

use crate::native::{config as bob_config, env as bob_env};

use super::GmailError;

pub(crate) const DEFAULT_FOLDER: &str = "mail";
pub(crate) const DEFAULT_LINK_LABEL: &str = "✉️ {subject} · {who}";
pub(crate) const DEFAULT_PULL_INBOX: &str = "gmail_inbox.md";
pub(crate) const DEFAULT_SYNC_LABEL: &str = "bob";
pub(crate) const DEFAULT_REPLY_WAIT_DAYS: u32 = 3;
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
    sync_label: Option<String>,
    sync_inbox: String,
    route_prefixes: Vec<String>,
    reply_wait_days: u32,
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
        validate_inbox("gmail.pull.inbox", &pull_inbox)?;
        let sync = settings.sync.unwrap_or_default();
        // `gmail.sync.*` wins; `gmail.pull.*` is the older spelling.
        let sync_label = match sync.label.or(pull.label.clone()) {
            Some(label) if label.is_empty() => None,
            Some(label) => Some(label),
            None => Some(DEFAULT_SYNC_LABEL.to_string()),
        };
        let sync_inbox = match sync.inbox {
            Some(inbox) => {
                validate_inbox("gmail.sync.inbox", &inbox)?;
                inbox
            }
            None => pull_inbox.clone(),
        };
        Ok(Self {
            account: settings.account,
            writer: settings.writer,
            body: settings.body.unwrap_or_default(),
            folder,
            link_label,
            never_save_from: settings.never_save.from,
            never_save_labels: settings.never_save.labels,
            pull_inbox,
            pull_label: pull.label.filter(|label| !label.is_empty()),
            signature: settings.signature.unwrap_or_default(),
            sync_label,
            sync_inbox,
            route_prefixes: sync.route_prefixes.unwrap_or_default(),
            reply_wait_days: sync
                .reply_wait_days
                .unwrap_or(DEFAULT_REPLY_WAIT_DAYS),
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

    pub(crate) fn link_label(&self) -> &str {
        &self.link_label
    }

    pub(crate) fn never_save_from(&self) -> &[String] {
        &self.never_save_from
    }

    pub(crate) fn never_save_labels(&self) -> &[String] {
        &self.never_save_labels
    }

    pub(crate) fn pull_inbox(&self) -> &str {
        &self.pull_inbox
    }

    pub(crate) fn pull_label(&self) -> Option<&str> {
        self.pull_label.as_deref()
    }

    pub(crate) fn signature(&self) -> bob_config::GmailSignature {
        self.signature
    }

    /// The enrollment label: `gmail.sync.label`, then `gmail.pull.label`,
    /// then `bob`. `None` when an explicit empty value disables sync.
    pub(crate) fn sync_label(&self) -> Option<&str> {
        self.sync_label.as_deref()
    }

    /// The file for unrouted threads and route problems:
    /// `gmail.sync.inbox`, then `gmail.pull.inbox`, then `gmail_inbox.md`.
    pub(crate) fn sync_inbox(&self) -> &str {
        &self.sync_inbox
    }

    /// `gmail.sync.route_prefixes`; empty means no-prefix mode.
    pub(crate) fn route_prefixes(&self) -> &[String] {
        &self.route_prefixes
    }

    /// Calendar days from your send to the hold's nudge date.
    pub(crate) fn reply_wait_days(&self) -> u32 {
        self.reply_wait_days
    }

    pub(crate) fn timeout(&self) -> Duration {
        Duration::from_secs(self.timeout_secs)
    }
}

/// An inbox key must be a safe relative path inside the vault.
fn validate_inbox(key: &str, inbox: &str) -> Result<(), GmailError> {
    if inbox.trim().is_empty() {
        return Err(GmailError::setup(
            "config",
            format!("{key} must not be empty"),
        ));
    }
    if super::note::path::validate_folder(std::path::Path::new(inbox)).is_err()
    {
        return Err(GmailError::setup(
            "config",
            format!("{key} must be a relative path without parent segments"),
        ));
    }
    Ok(())
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

    fn sync(
        label: Option<&str>,
        inbox: Option<&str>,
    ) -> Option<bob_config::GmailSyncSettings> {
        Some(bob_config::GmailSyncSettings {
            label: label.map(str::to_string),
            inbox: inbox.map(str::to_string),
            ..Default::default()
        })
    }

    fn pull(
        label: Option<&str>,
        inbox: Option<&str>,
    ) -> Option<bob_config::GmailPullSettings> {
        Some(bob_config::GmailPullSettings {
            label: label.map(str::to_string),
            inbox: inbox.map(str::to_string),
        })
    }

    #[test]
    fn sync_defaults_when_both_blocks_are_missing() {
        let config = GmailConfig::from_settings(settings()).expect("defaults");
        assert_eq!(config.sync_label(), Some("bob"));
        assert_eq!(config.sync_inbox(), "gmail_inbox.md");
        assert!(config.route_prefixes().is_empty(), "no-prefix mode");
        assert_eq!(config.reply_wait_days(), 3);
    }

    #[test]
    fn sync_label_falls_back_to_pull_then_bob() {
        let resolve = |sync, pull| {
            GmailConfig::from_settings(bob_config::GmailSettings {
                sync,
                pull,
                ..settings()
            })
            .expect("resolves")
            .sync_label()
            .map(str::to_string)
        };
        // An explicit sync label wins over pull.
        assert_eq!(
            resolve(sync(Some("watch"), None), pull(Some("old"), None)),
            Some("watch".to_string())
        );
        // An unset sync label uses pull's spelling.
        assert_eq!(
            resolve(sync(None, None), pull(Some("old"), None)),
            Some("old".to_string())
        );
        assert_eq!(resolve(None, pull(Some("old"), None)), Some("old".into()));
        // A pull block without a label still defaults sync to bob.
        assert_eq!(resolve(None, pull(None, None)), Some("bob".to_string()));
        // An explicit empty value disables sync, in either spelling.
        assert_eq!(
            resolve(sync(Some(""), None), pull(Some("old"), None)),
            None
        );
        assert_eq!(resolve(None, pull(Some(""), None)), None);
        // An explicit sync label re-enables what an empty pull label disabled.
        assert_eq!(
            resolve(sync(Some("bob"), None), pull(Some(""), None)),
            Some("bob".to_string())
        );
    }

    #[test]
    fn pull_label_keeps_its_own_resolution() {
        let resolve = |pull| {
            GmailConfig::from_settings(bob_config::GmailSettings {
                pull,
                ..settings()
            })
            .expect("resolves")
            .pull_label()
            .map(str::to_string)
        };
        assert_eq!(resolve(None), Some("bob".to_string()));
        assert_eq!(resolve(pull(Some("old"), None)), Some("old".to_string()));
        assert_eq!(resolve(pull(Some(""), None)), None);
        assert_eq!(resolve(pull(None, None)), None);
    }

    #[test]
    fn sync_inbox_falls_back_to_pull_then_default() {
        let resolve = |sync, pull| {
            GmailConfig::from_settings(bob_config::GmailSettings {
                sync,
                pull,
                ..settings()
            })
            .expect("resolves")
            .sync_inbox()
            .to_string()
        };
        assert_eq!(
            resolve(sync(None, Some("routed.md")), pull(None, Some("old.md"))),
            "routed.md"
        );
        assert_eq!(
            resolve(sync(None, None), pull(None, Some("old.md"))),
            "old.md"
        );
        assert_eq!(
            resolve(None, pull(None, Some("inbox/mail.md"))),
            "inbox/mail.md"
        );
        assert_eq!(resolve(sync(None, None), None), "gmail_inbox.md");
    }

    #[test]
    fn route_prefixes_and_reply_wait_days_resolve() {
        let config = GmailConfig::from_settings(bob_config::GmailSettings {
            sync: Some(bob_config::GmailSyncSettings {
                route_prefixes: Some(vec!["#".to_string(), "+".to_string()]),
                reply_wait_days: Some(7),
                ..Default::default()
            }),
            ..settings()
        })
        .expect("resolves");
        assert_eq!(config.route_prefixes(), ["#", "+"]);
        assert_eq!(config.reply_wait_days(), 7);
    }

    #[test]
    fn unsafe_sync_inbox_is_a_setup_error_naming_the_key() {
        for inbox in ["/gmail_inbox.md", "../outside.md", "a/../../b.md"] {
            let error = GmailConfig::from_settings(bob_config::GmailSettings {
                sync: sync(None, Some(inbox)),
                ..settings()
            })
            .expect_err("unsafe inbox");
            assert_eq!(error.exit_code(), 2);
            assert_eq!(error.kind(), "config");
            assert!(
                error.message().contains("gmail.sync.inbox"),
                "{inbox}: {}",
                error.message()
            );
        }
        let error = GmailConfig::from_settings(bob_config::GmailSettings {
            pull: pull(None, Some("/abs.md")),
            ..settings()
        })
        .expect_err("unsafe pull inbox");
        assert!(error.message().contains("gmail.pull.inbox"));
    }

    #[test]
    fn sync_config_errors_from_the_file_are_setup_errors() {
        let dir = std::env::temp_dir().join(format!(
            "bob-cli-gmail-sync-config-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let path = dir.join("config.yml");
        for (text, key) in [
            (
                "gmail:\n  sync:\n    route_prefixes: [\"#\", \"#\"]\n",
                "gmail.sync.route_prefixes",
            ),
            (
                "gmail:\n  sync:\n    reply_wait_days: 0\n",
                "gmail.sync.reply_wait_days",
            ),
            ("gmail:\n  sync:\n    label: 7\n", "gmail.sync.label"),
        ] {
            std::fs::write(&path, text).expect("write config");
            let error = GmailConfig::resolve_at(&path).expect_err(text);
            assert_eq!(error.exit_code(), 2, "{text}");
            assert!(
                error.message().contains(key),
                "{text}: {}",
                error.message()
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
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
