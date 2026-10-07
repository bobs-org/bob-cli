//! Routing policy: per-entry-point toggles and host exclusions.
//!
//! Missing keys take the defaults. A configured `exclude_hosts` list
//! replaces the defaults; an empty list excludes nothing.

use crate::native::config::{self, ConfigError};

use super::UrlIntent;

/// Where a URL was seen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RoutingEntry {
    Capture,
    Gkeep,
}

/// Default excluded hosts: reminders, tools, or login-walled pages.
pub(crate) const DEFAULT_EXCLUDE_HOSTS: &[&str] = &[
    "google.com",
    "googleplex.com",
    "youtube.com",
    "youtu.be",
    "github.com",
    "x.com",
    "twitter.com",
];

/// Effective routing policy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct UrlRoutingPolicy {
    pub(crate) capture: bool,
    pub(crate) gkeep: bool,
    pub(crate) exclude_hosts: Vec<String>,
}

impl Default for UrlRoutingPolicy {
    fn default() -> Self {
        Self {
            capture: true,
            gkeep: true,
            exclude_hosts: DEFAULT_EXCLUDE_HOSTS
                .iter()
                .map(|host| host.to_string())
                .collect(),
        }
    }
}

impl UrlRoutingPolicy {
    /// Load the policy from the config file at `config_path()`.
    /// A missing file gives the defaults; an unreadable or invalid
    /// file is an error, and callers turn routing off with a warning.
    pub(crate) fn load() -> Result<Self, ConfigError> {
        let highlights =
            config::load_highlights_config(&config::config_path())?;
        Ok(Self::from_highlights(&highlights))
    }

    /// Build the policy from loaded highlight config values.
    pub(crate) fn from_highlights(
        highlights: &config::HighlightsConfig,
    ) -> Self {
        Self {
            capture: highlights.url_routing_capture().unwrap_or(true),
            gkeep: highlights.url_routing_gkeep().unwrap_or(true),
            exclude_hosts: highlights.url_routing_exclude_hosts().map_or_else(
                || {
                    DEFAULT_EXCLUDE_HOSTS
                        .iter()
                        .map(|host| host.to_string())
                        .collect()
                },
                |hosts| hosts.to_vec(),
            ),
        }
    }

    /// Whether `intent` is admitted for `entry`: the entry toggle is
    /// on and the host matches no exclusion (each entry matches the
    /// host and every subdomain).
    pub(crate) fn admits(
        &self,
        intent: &UrlIntent,
        entry: RoutingEntry,
    ) -> bool {
        let enabled = match entry {
            RoutingEntry::Capture => self.capture,
            RoutingEntry::Gkeep => self.gkeep,
        };
        if !enabled {
            return false;
        }
        !self.is_excluded(&intent.host)
    }

    fn is_excluded(&self, host: &str) -> bool {
        let host = host.to_lowercase();
        self.exclude_hosts
            .iter()
            .any(|entry| host == *entry || host.ends_with(&format!(".{entry}")))
    }
}

/// Normalize one `exclude_hosts` entry: lowercased, with any scheme,
/// leading `www.`, or trailing `.` or `/` stripped. Returns `None`
/// when nothing remains.
pub(crate) fn normalize_exclude_host(raw: &str) -> Option<String> {
    let mut text = raw.trim().to_lowercase();
    if text.is_empty() {
        return None;
    }
    if let Some((_, after)) = text.split_once("://") {
        text = after.to_string();
    }
    // Keep only the host part when a path was included.
    if let Some((host, _)) = text.split_once('/') {
        text = host.to_string();
    }
    // Strip a port when present (a bare `host:port` entry).
    if let Some((host, _)) = text.split_once(':') {
        text = host.to_string();
    }
    if let Some(stripped) = text.strip_prefix("www.") {
        text = stripped.to_string();
    }
    text = text.trim_end_matches('.').to_string();
    (!text.is_empty()).then_some(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn intent_for(host: &str) -> UrlIntent {
        UrlIntent {
            original: format!("https://{host}/a"),
            cleaned: format!("https://{host}/a"),
            dedupe_key: format!("https://{host}/a"),
            host: host.to_string(),
            display: format!("{host}/a"),
            route_hint: super::super::RouteHint::Article,
        }
    }

    #[test]
    fn defaults_cover_the_plan_list() {
        let policy = UrlRoutingPolicy::default();
        assert!(policy.capture);
        assert!(policy.gkeep);
        assert_eq!(
            policy.exclude_hosts,
            DEFAULT_EXCLUDE_HOSTS
                .iter()
                .map(|host| host.to_string())
                .collect::<Vec<_>>(),
        );
    }

    #[test]
    fn subdomain_matching() {
        let policy = UrlRoutingPolicy::default();
        assert!(policy.is_excluded("youtube.com"));
        assert!(policy.is_excluded("m.youtube.com"));
        assert!(policy.is_excluded("www.youtube.com"));
        assert!(!policy.is_excluded("notyoutube.com"));
        assert!(!policy.is_excluded("example.com"));
    }

    #[test]
    fn empty_list_excludes_nothing() {
        let policy = UrlRoutingPolicy {
            capture: true,
            gkeep: true,
            exclude_hosts: Vec::new(),
        };
        assert!(
            policy.admits(&intent_for("youtube.com"), RoutingEntry::Capture)
        );
    }

    #[test]
    fn toggles_gate_entries_independently() {
        let policy = UrlRoutingPolicy {
            capture: false,
            gkeep: true,
            exclude_hosts: Vec::new(),
        };
        assert!(
            !policy.admits(&intent_for("example.com"), RoutingEntry::Capture)
        );
        assert!(policy.admits(&intent_for("example.com"), RoutingEntry::Gkeep));
    }

    #[test]
    fn normalization_strips_scheme_www_and_dot() {
        assert_eq!(
            normalize_exclude_host("HTTPS://WWW.Example.COM./"),
            Some("example.com".to_string()),
        );
        assert_eq!(normalize_exclude_host("  "), None);
        assert_eq!(normalize_exclude_host("www."), None);
    }

    #[test]
    fn invalid_yaml_is_an_error() {
        let error = config::parse_highlights_config(
            "highlights:\n  url_routing:\n    capture: \"yes\"\n",
            std::path::Path::new("/config.yml"),
        )
        .expect_err("mistyped capture is invalid");
        assert!(error_message(&error).contains("/config.yml"));
    }

    #[test]
    fn overrides_and_empty_list() {
        let config = config::parse_highlights_config(
            "highlights:\n  url_routing:\n    capture: false\n    exclude_hosts: []\n",
            std::path::Path::new("/config.yml"),
        )
        .expect("overrides parse");
        let policy = UrlRoutingPolicy::from_highlights(&config);
        assert!(!policy.capture);
        assert!(policy.gkeep);
        assert!(policy.exclude_hosts.is_empty());
    }

    fn error_message(error: &ConfigError) -> String {
        match error {
            ConfigError::Read(message) | ConfigError::Invalid(message) => {
                message.clone()
            }
        }
    }
}
