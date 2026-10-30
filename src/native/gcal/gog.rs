//! gog CLI transport for `bob gcal`.
//!
//! Common flags go before the subcommand. Tokens never appear in argv
//! or logs. `BOB_GCAL_GOG` replaces `gog` on PATH.

use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    time::Duration,
};

use serde::Deserialize;

use crate::native::{
    env as bob_env,
    subprocess_runner::{self, RunSpec},
};

use super::{config::GcalConfig, GcalError};

pub(crate) const CLIENT: &str = "bob-gcal";
pub(crate) const MIN_VERSION: (u64, u64, u64) = (0, 44, 0);
pub(crate) const CALENDAR_READONLY_SCOPE: &str =
    "https://www.googleapis.com/auth/calendar.readonly";
pub(crate) const API_CONSOLE_URL: &str =
    "https://console.cloud.google.com/apis/library/calendar-json.googleapis.com";
pub(crate) const REVOKE_URL: &str = "https://myaccount.google.com/permissions";

/// Resolved gog executable plus timeout.
#[derive(Debug, Clone)]
pub(crate) struct Gog {
    program: PathBuf,
    timeout: Duration,
}

/// One stored OAuth client from `gog auth credentials list --json`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub(crate) struct GogClient {
    pub client: String,
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub default: bool,
    #[serde(default)]
    pub client_secret_in_keyring: bool,
}

/// Remote `--step 1` result: the authorization URL and its `state`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AuthStart {
    pub auth_url: String,
    pub state: String,
}

/// One account from `gog auth list --json`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub(crate) struct GogAccount {
    pub email: String,
    #[serde(default)]
    pub client: Option<String>,
    #[serde(default)]
    pub services: Vec<String>,
    #[serde(default)]
    pub scopes: Vec<String>,
}

/// One calendar from `gog calendar calendars --json`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub(crate) struct GogCalendar {
    pub id: String,
    #[serde(default)]
    pub summary: Option<String>,
    #[serde(default)]
    pub primary: bool,
    #[serde(default, rename = "accessRole")]
    pub access_role: Option<String>,
    #[serde(default, rename = "backgroundColor")]
    pub background_color: Option<String>,
    #[serde(default, rename = "colorId")]
    pub color_id: Option<String>,
}

impl GogCalendar {
    pub(crate) fn name(&self) -> &str {
        self.summary
            .as_deref()
            .filter(|name| !name.is_empty())
            .unwrap_or(&self.id)
    }

    pub(crate) fn colour(&self) -> Option<&str> {
        self.background_color
            .as_deref()
            .filter(|value| !value.is_empty())
            .or(self.color_id.as_deref())
    }

    /// `primary` in config matches this calendar's primary flag or id.
    pub(crate) fn matches_configured_id(&self, configured: &str) -> bool {
        self.id == configured
            || (configured == "primary"
                && (self.primary || self.id == "primary"))
    }
}

impl GogAccount {
    pub(crate) fn has_calendar_readonly(&self) -> bool {
        self.services.iter().any(|service| service == "calendar")
            || self.scopes.iter().any(|scope| {
                scope == CALENDAR_READONLY_SCOPE
                    || scope.contains("calendar.readonly")
                    || scope.ends_with("/calendar")
            })
    }
}

impl Gog {
    /// Resolve `BOB_GCAL_GOG`, else `gog` on PATH, to an absolute path.
    pub(crate) fn resolve(timeout: Duration) -> Result<Self, GcalError> {
        let program = match GcalConfig::gog_override() {
            Some(path) => {
                if path.is_absolute() {
                    path
                } else {
                    bob_env::bob_dir()
                        .parent()
                        .unwrap_or_else(|| Path::new("."))
                        .join(&path)
                        .canonicalize()
                        .unwrap_or(path)
                }
            }
            None => bob_env::find_on_path("gog").ok_or_else(|| {
                GcalError::setup(
                    "gog_missing",
                    "gog not found on PATH".to_string(),
                )
                .with_hint(
                    "install gog ≥ v0.44.0 (https://github.com/openclaw/gogcli) \
                     and put it on PATH",
                )
            })?,
        };
        let program = if program.is_absolute() {
            program
        } else {
            program.canonicalize().unwrap_or(program)
        };
        Ok(Self { program, timeout })
    }

    pub(crate) fn program(&self) -> &Path {
        &self.program
    }

    /// `gog version` as a dotted string.
    pub(crate) fn version(&self) -> Result<String, GcalError> {
        let output = self.run(
            self.common_flags(None)
                .into_iter()
                .chain(once_os("version"))
                .collect(),
        )?;
        if let Ok(value) =
            serde_json::from_str::<serde_json::Value>(&output.stdout)
            && let Some(version) = value.get("version").and_then(|v| v.as_str())
        {
            return Ok(version.to_string());
        }
        let first = output
            .stdout
            .split_whitespace()
            .next()
            .unwrap_or(output.stdout.trim());
        if first.is_empty() {
            return Err(GcalError::runtime(
                "bad_json",
                "gog version printed no version".to_string(),
            ));
        }
        Ok(first.to_string())
    }

    /// Require gog ≥ 0.44.0.
    pub(crate) fn require_version(&self) -> Result<String, GcalError> {
        let version = self.version()?;
        match parse_semver(&version) {
            Some(parsed) if parsed >= MIN_VERSION => Ok(version),
            Some(_) => Err(GcalError::setup(
                "gog_too_old",
                format!("gog {version} is older than v0.44.0"),
            )
            .with_hint("upgrade gog to v0.44.0 or later")),
            None if version == "dev" => Ok(version),
            None => Err(GcalError::setup(
                "gog_too_old",
                format!(
                    "gog version {version:?} is not a dotted semver ≥ 0.44.0"
                ),
            )
            .with_hint("upgrade gog to v0.44.0 or later")),
        }
    }

    pub(crate) fn credentials_list(&self) -> Result<Vec<GogClient>, GcalError> {
        let output = self.run(
            self.common_flags(None)
                .into_iter()
                .chain([
                    OsString::from("auth"),
                    OsString::from("credentials"),
                    OsString::from("list"),
                ])
                .collect(),
        )?;
        let value = parse_json(&output.stdout)?;
        let clients = value
            .get("clients")
            .cloned()
            .unwrap_or(serde_json::Value::Array(Vec::new()));
        serde_json::from_value(clients).map_err(|error| {
            GcalError::runtime(
                "bad_json",
                format!("parse gog credentials list: {error}"),
            )
        })
    }

    /// Pipe normalized `{"installed": …}` JSON to
    /// `gog auth credentials set -` over stdin. The secret never appears
    /// in argv. Storing a client is a write: omit `--readonly` and never
    /// pass `--insecure`.
    pub(crate) fn credentials_set_json(
        &self,
        payload: &[u8],
    ) -> Result<(), GcalError> {
        let _ = self.run_with_stdin(
            vec![
                OsString::from("--no-input"),
                OsString::from("--json"),
                OsString::from("--color"),
                OsString::from("never"),
                OsString::from("--client"),
                OsString::from(CLIENT),
                OsString::from("auth"),
                OsString::from("credentials"),
                OsString::from("set"),
                OsString::from("-"),
            ],
            Some(payload.to_vec()),
        )?;
        Ok(())
    }

    /// `gog auth list --json`. Pass `Some(CLIENT)` to filter Bob's bucket.
    pub(crate) fn accounts(
        &self,
        client: Option<&str>,
    ) -> Result<Vec<GogAccount>, GcalError> {
        let mut flags = vec![
            OsString::from("--no-input"),
            OsString::from("--json"),
            OsString::from("--color"),
            OsString::from("never"),
            OsString::from("--readonly"),
        ];
        if let Some(client) = client {
            flags.push(OsString::from("--client"));
            flags.push(OsString::from(client));
        }
        flags.push(OsString::from("auth"));
        flags.push(OsString::from("list"));
        let output = self.run(flags)?;
        let value = parse_json(&output.stdout)?;
        let accounts = value
            .get("accounts")
            .cloned()
            .unwrap_or(serde_json::Value::Array(Vec::new()));
        serde_json::from_value(accounts).map_err(|error| {
            GcalError::runtime(
                "bad_json",
                format!("parse gog auth list: {error}"),
            )
        })
    }

    pub(crate) fn calendars(
        &self,
        account: &str,
    ) -> Result<Vec<GogCalendar>, GcalError> {
        let output = self.run(
            self.common_flags(Some(account))
                .into_iter()
                .chain([
                    OsString::from("calendar"),
                    OsString::from("calendars"),
                    OsString::from("--all"),
                ])
                .collect(),
        )?;
        let value = parse_json(&output.stdout)?;
        let calendars = value
            .get("calendars")
            .cloned()
            .unwrap_or(serde_json::Value::Array(Vec::new()));
        serde_json::from_value(calendars).map_err(|error| {
            GcalError::runtime(
                "bad_json",
                format!("parse gog calendar list: {error}"),
            )
        })
    }

    /// Fetch a complete event snapshot for one calendar and local-day window.
    pub(crate) fn events(
        &self,
        calendar_id: &str,
        from: &str,
        to: &str,
    ) -> Result<Vec<serde_json::Value>, GcalError> {
        const FIELDS: &str = "items(id,iCalUID,recurringEventId,originalStartTime,status,summary,description,location,htmlLink,hangoutLink,conferenceData(entryPoints(entryPointType,uri)),start,end,transparency,eventType,colorId,attendees(self,responseStatus),organizer(self)),nextPageToken";
        let output = self.run(
            self.common_flags(None)
                .into_iter()
                .chain([
                    OsString::from("calendar"),
                    OsString::from("events"),
                    OsString::from(calendar_id),
                    OsString::from("--from"),
                    OsString::from(from),
                    OsString::from("--to"),
                    OsString::from(to),
                    OsString::from("--all-pages"),
                    OsString::from("--max"),
                    OsString::from("250"),
                    OsString::from("--fields"),
                    OsString::from(FIELDS),
                ])
                .collect(),
        )?;
        let value = parse_json(&output.stdout)?;
        if value
            .get("nextPageToken")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|token| !token.is_empty())
        {
            return Err(GcalError::runtime(
                "incomplete_snapshot",
                format!("gog left another page for calendar {calendar_id}"),
            ));
        }
        let events = value
            .get("events")
            .or_else(|| value.get("items"))
            .cloned()
            .unwrap_or(serde_json::Value::Null);
        serde_json::from_value(events).map_err(|error| {
            GcalError::runtime(
                "bad_json",
                format!("parse gog events for {calendar_id}: {error}"),
            )
        })
    }

    /// Look up one event for lifecycle reconciliation.
    pub(crate) fn event(
        &self,
        calendar_id: &str,
        event_id: &str,
    ) -> Result<serde_json::Value, GcalError> {
        let output = self.run(
            self.common_flags(None)
                .into_iter()
                .chain([
                    OsString::from("calendar"),
                    OsString::from("event"),
                    OsString::from(calendar_id),
                    OsString::from(event_id),
                ])
                .collect(),
        )?;
        let value = parse_json(&output.stdout)?;
        value.get("event").cloned().ok_or_else(|| {
            GcalError::runtime(
                "bad_json",
                format!("gog returned no event for {calendar_id}/{event_id}"),
            )
        })
    }

    /// Remote `--step 1`: print the authorization URL. gog 0.44.0
    /// accepts `--force-consent` and `--redirect-uri` (verified in a
    /// throwaway `GOG_HOME`).
    pub(crate) fn auth_begin(
        &self,
        account: &str,
        redirect_uri: Option<&str>,
    ) -> Result<AuthStart, GcalError> {
        let mut args = self.auth_add_prefix(account);
        args.extend([
            OsString::from("--force-consent"),
            OsString::from("--remote"),
            OsString::from("--step"),
            OsString::from("1"),
        ]);
        if let Some(uri) = redirect_uri {
            args.push(OsString::from("--redirect-uri"));
            args.push(OsString::from(uri));
        }
        let output = self.run(args)?;
        let value = parse_json(&output.stdout)?;
        let auth_url = value
            .get("auth_url")
            .and_then(|v| v.as_str())
            .filter(|url| !url.is_empty())
            .ok_or_else(|| {
                GcalError::runtime(
                    "bad_json",
                    "gog remote step 1 printed no auth_url".to_string(),
                )
            })?;
        let state = state_from_auth_url(auth_url).ok_or_else(|| {
            GcalError::runtime(
                "bad_json",
                "gog auth_url is missing state".to_string(),
            )
        })?;
        Ok(AuthStart {
            auth_url: auth_url.to_string(),
            state,
        })
    }

    /// Remote `--step 2`: exchange the redirect URL for a refresh token.
    /// The single-use PKCE code is in `--auth-url`; it is useless without
    /// gog's stored verifier, and gog redeems it immediately.
    pub(crate) fn auth_finish(
        &self,
        account: &str,
        redirect_url: &str,
    ) -> Result<(), GcalError> {
        let mut args = self.auth_add_prefix(account);
        args.extend([
            OsString::from("--remote"),
            OsString::from("--step"),
            OsString::from("2"),
            OsString::from("--auth-url"),
            OsString::from(redirect_url),
        ]);
        let _ = self.run(args)?;
        Ok(())
    }

    fn auth_add_prefix(&self, account: &str) -> Vec<OsString> {
        vec![
            OsString::from("--no-input"),
            OsString::from("--json"),
            OsString::from("--color"),
            OsString::from("never"),
            OsString::from("--readonly"),
            OsString::from("--client"),
            OsString::from(CLIENT),
            OsString::from("auth"),
            OsString::from("add"),
            OsString::from(account),
            OsString::from("--services"),
            OsString::from("calendar"),
        ]
    }

    pub(crate) fn auth_remove(&self, account: &str) -> Result<(), GcalError> {
        let _ = self.run(vec![
            OsString::from("--no-input"),
            OsString::from("--json"),
            OsString::from("--color"),
            OsString::from("never"),
            OsString::from("--client"),
            OsString::from(CLIENT),
            OsString::from("--force"),
            OsString::from("auth"),
            OsString::from("remove"),
            OsString::from(account),
        ])?;
        Ok(())
    }

    fn common_flags(&self, account: Option<&str>) -> Vec<OsString> {
        let mut flags = vec![
            OsString::from("--no-input"),
            OsString::from("--json"),
            OsString::from("--color"),
            OsString::from("never"),
            OsString::from("--readonly"),
            OsString::from("--client"),
            OsString::from(CLIENT),
        ];
        if let Some(account) = account {
            flags.push(OsString::from("--account"));
            flags.push(OsString::from(account));
        }
        flags
    }

    fn run(
        &self,
        args: Vec<OsString>,
    ) -> Result<subprocess_runner::RunOutput, GcalError> {
        self.run_with_stdin(args, None)
    }

    fn run_with_stdin(
        &self,
        args: Vec<OsString>,
        stdin: Option<Vec<u8>>,
    ) -> Result<subprocess_runner::RunOutput, GcalError> {
        let output = subprocess_runner::run(&RunSpec {
            program: self.program.clone(),
            args,
            stdin,
            env: Vec::new(),
            timeout: self.timeout,
        })
        .map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                GcalError::setup(
                    "gog_missing",
                    format!("gog not found: {}", self.program.display()),
                )
                .with_hint(
                    "install gog ≥ v0.44.0 and put it on PATH, or set \
                     BOB_GCAL_GOG",
                )
            } else {
                GcalError::runtime("gog_missing", format!("start gog: {error}"))
            }
        })?;
        if output.timed_out {
            return Err(GcalError::runtime(
                "timeout",
                format!("gog timed out after {}s", self.timeout.as_secs()),
            )
            .with_hint("check your network, or raise gcal.timeout_secs"));
        }
        match output.status {
            Some(0) => Ok(output),
            Some(_) | None => {
                Err(classify_gog_failure(&output.stdout, &output.stderr))
            }
        }
    }
}

fn once_os(value: &str) -> std::iter::Once<OsString> {
    std::iter::once(OsString::from(value))
}

pub(crate) fn state_from_auth_url(auth_url: &str) -> Option<String> {
    let url = url::Url::parse(auth_url).ok()?;
    url.query_pairs().find_map(|(key, value)| {
        (key == "state" && !value.is_empty()).then(|| value.into_owned())
    })
}

fn parse_json(stdout: &str) -> Result<serde_json::Value, GcalError> {
    serde_json::from_str(stdout.trim()).map_err(|error| {
        GcalError::runtime(
            "bad_json",
            format!("gog returned invalid JSON: {error}"),
        )
    })
}

pub(crate) fn parse_semver(version: &str) -> Option<(u64, u64, u64)> {
    let trimmed = version.trim().trim_start_matches('v');
    let mut parts = trimmed
        .split(|ch: char| !ch.is_ascii_digit())
        .filter(|part| !part.is_empty());
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    let patch = parts.next().and_then(|part| part.parse().ok()).unwrap_or(0);
    Some((major, minor, patch))
}

/// Project-specific Calendar API console URL from Google's message,
/// else `None` for the generic library URL.
fn project_console_url(stdout: &str, stderr: &str) -> Option<String> {
    for text in [stdout, stderr] {
        let lower = text.to_ascii_lowercase();
        let mut search = lower.as_str();
        while let Some(index) = search.find("project") {
            let after = &search[index + "project".len()..];
            let digits: String = after
                .chars()
                .skip_while(|ch| !ch.is_ascii_digit())
                .take_while(|ch| ch.is_ascii_digit())
                .collect();
            if !digits.is_empty() {
                return Some(format!(
                    "https://console.developers.google.com/apis/api/\
                     calendar-json.googleapis.com/overview?project={digits}"
                ));
            }
            search = after;
        }
    }
    None
}

pub(crate) fn classify_gog_failure(stdout: &str, stderr: &str) -> GcalError {
    let blob = format!("{stdout}\n{stderr}").to_ascii_lowercase();
    if blob.contains("accessnotconfigured")
        || blob.contains("access not configured")
        || blob.contains("has not been used in project")
        || blob.contains("is disabled")
    {
        let url = project_console_url(stdout, stderr)
            .unwrap_or_else(|| API_CONSOLE_URL.to_string());
        return GcalError::runtime(
            "api_disabled",
            "the Google Calendar API is not enabled for this Cloud project"
                .to_string(),
        )
        .with_hint(&format!("enable it at {url}"));
    }
    if blob.contains("store secret") || blob.contains("keyring") {
        return GcalError::runtime(
            "keyring",
            "gog could not save the client or token in the system keyring"
                .to_string(),
        )
        .with_hint(
            "run `gog auth status` and `gog auth keyring file` to inspect the \
             keyring, then rerun `bob gcal login`",
        );
    }
    if blob.contains("invalid_client")
        || blob.contains("oauth client was not found")
    {
        return GcalError::setup(
            "invalid_client",
            "Google does not know this OAuth client (deleted or from \
             another project)"
                .to_string(),
        )
        .with_hint(
            "download a Desktop app client JSON and run \
             `bob gcal login -c PATH`",
        );
    }
    if blob.contains("redirect_uri_mismatch") {
        return GcalError::setup(
            "client_invalid",
            "Google rejected the redirect: the OAuth client is not a \
             Desktop app"
                .to_string(),
        )
        .with_hint(
            "download a Desktop app client JSON and run \
             `bob gcal login -c PATH`",
        );
    }
    if blob.contains("manual auth state missing")
        || blob.contains("manual auth state mismatch")
    {
        return GcalError::runtime(
            "stale_link",
            "that sign-in link is out of date".to_string(),
        )
        .with_hint("Bob starts a fresh sign-in link and you try once more");
    }
    if blob.contains("no code found in url")
        || blob.contains("missing state in redirect url")
    {
        return GcalError::runtime(
            "no_code",
            "the redirect address is missing the authorization code"
                .to_string(),
        )
        .with_hint(
            "after you click Allow, copy the whole address of the page \
             that starts with http://127.0.0.1:",
        );
    }
    if blob.contains("invalid_grant") {
        return GcalError::runtime(
            "invalid_grant",
            "the Calendar grant was rejected or expired".to_string(),
        )
        .with_hint(
            "publish the consent screen (Testing-mode tokens expire in 7 days), \
             then run `bob gcal login`",
        );
    }
    if blob.contains("insufficient") && blob.contains("scope")
        || blob.contains("access_token_scope_insufficient")
        || blob.contains("insufficientpermissions")
        || blob.contains("insufficient authentication scopes")
    {
        return GcalError::setup(
            "insufficient_scope",
            "the stored token lacks Calendar read-only scope".to_string(),
        )
        .with_hint("run `bob gcal login` to grant calendar.readonly");
    }
    if blob.contains("no stored")
        || blob.contains("not logged in")
        || blob.contains("no refresh token")
        || blob.contains("no token")
        || blob.contains("token not found")
    {
        return GcalError::setup(
            "not_logged_in",
            "no bob-gcal Calendar grant is stored".to_string(),
        )
        .with_hint("run `bob gcal login`");
    }
    if blob.contains("credentials")
        && (blob.contains("not found") || blob.contains("no oauth client"))
        || blob.contains("no client")
    {
        return GcalError::setup(
            "no_client",
            "no bob-gcal OAuth client is stored".to_string(),
        )
        .with_hint(
            "run `bob gcal login -c PATH` with a Desktop-app client JSON",
        );
    }
    if blob.contains("connection refused")
        || blob.contains("network is unreachable")
        || blob.contains("temporary failure in name resolution")
        || blob.contains("no such host")
        || blob.contains("i/o timeout")
        || blob.contains("tls handshake")
        || blob.contains("dial tcp")
    {
        return GcalError::runtime(
            "network",
            "gog could not reach Google".to_string(),
        )
        .with_hint("check your network connection");
    }
    if blob.contains("notfound")
        || blob.contains("not found")
        || blob.contains("\"code\":404")
        || blob.contains("\"code\": 404")
        || (blob.contains("410")
            && (blob.contains("gone") || blob.contains("deleted")))
    {
        return GcalError::runtime(
            "not_found",
            "Google has no such event (deleted or moved away)".to_string(),
        );
    }
    let detail = stderr
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .or_else(|| stdout.lines().map(str::trim).find(|line| !line.is_empty()))
        .unwrap_or("gog failed");
    GcalError::runtime("gog", detail.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn semver_parse_and_compare() {
        assert_eq!(parse_semver("0.44.0"), Some((0, 44, 0)));
        assert_eq!(parse_semver("v0.44.1"), Some((0, 44, 1)));
        assert!(parse_semver("0.44.0").unwrap() >= MIN_VERSION);
        assert!(parse_semver("0.43.9").unwrap() < MIN_VERSION);
    }

    #[test]
    fn maps_api_disabled_and_invalid_grant() {
        let error = classify_gog_failure("", "accessNotConfigured");
        assert_eq!(error.kind(), "api_disabled");
        assert!(error.hint().unwrap_or_default().contains("calendar-json"));

        let error = classify_gog_failure(
            "",
            "accessNotConfigured: Calendar API has not been used in project \
             123456789012",
        );
        assert_eq!(error.kind(), "api_disabled");
        assert!(error
            .hint()
            .unwrap_or_default()
            .contains("project=123456789012"));

        let error =
            classify_gog_failure("", "invalid_grant: Token has been expired");
        assert_eq!(error.kind(), "invalid_grant");
        assert!(error.hint().unwrap_or_default().contains("7 days"));
    }

    #[test]
    fn maps_keyring_invalid_client_and_redirect_mismatch() {
        let error = classify_gog_failure("", "cannot store secret in keyring");
        assert_eq!(error.kind(), "keyring");
        assert!(error
            .hint()
            .unwrap_or_default()
            .contains("gog auth keyring"));

        let error = classify_gog_failure("", "invalid_client: unknown");
        assert_eq!(error.kind(), "invalid_client");
        assert_eq!(error.exit_code(), 2);

        let error =
            classify_gog_failure("", "Error 400: redirect_uri_mismatch");
        assert_eq!(error.kind(), "client_invalid");
        assert_eq!(error.exit_code(), 2);
    }

    #[test]
    fn maps_remote_step_failures() {
        let error = classify_gog_failure(
            "",
            "manual auth state missing; start a new manual flow",
        );
        assert_eq!(error.kind(), "stale_link");

        let error = classify_gog_failure("", "no code found in URL");
        assert_eq!(error.kind(), "no_code");
    }

    #[test]
    fn state_from_auth_url_reads_query() {
        let state = state_from_auth_url(
            "https://accounts.google.com/o/oauth2/auth?state=abc&code=x",
        )
        .expect("state");
        assert_eq!(state, "abc");
    }

    #[test]
    fn calendar_primary_matches_configured_id() {
        let calendar = GogCalendar {
            id: "you@example.com".to_string(),
            summary: Some("Personal".to_string()),
            primary: true,
            access_role: Some("owner".to_string()),
            background_color: None,
            color_id: None,
        };
        assert!(calendar.matches_configured_id("primary"));
        assert!(calendar.matches_configured_id("you@example.com"));
        assert!(!calendar.matches_configured_id("work"));
    }
}
