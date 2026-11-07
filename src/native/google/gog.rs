//! Shared gog CLI transport for Bob's Google integrations.
//!
//! Common flags go before the subcommand. Tokens never appear in argv
//! or logs. Each service profile can override the `gog` executable.

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

use super::{GoogleError, GoogleService};

pub(crate) const MIN_VERSION: (u64, u64, u64) = (0, 44, 0);

/// Resolved gog executable plus timeout.
#[derive(Debug, Clone)]
pub(crate) struct Gog {
    program: PathBuf,
    timeout: Duration,
    service: GoogleService,
    extra_scopes: Vec<String>,
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

impl GogAccount {
    pub(crate) fn has_service_scope(&self, service: &GoogleService) -> bool {
        service.account_has_scope(&self.services, &self.scopes)
    }
}

impl Gog {
    /// Resolve the service-specific gog override, else `gog` on PATH.
    pub(crate) fn resolve(
        service: GoogleService,
        timeout: Duration,
    ) -> Result<Self, GoogleError> {
        let override_path = bob_env::var_os(service.gog_override_env)
            .filter(|value| !value.is_empty())
            .map(PathBuf::from);
        let program = match override_path {
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
                GoogleError::setup(
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
        Ok(Self {
            program,
            timeout,
            service,
            extra_scopes: Vec::new(),
        })
    }

    /// Extra OAuth scopes (e.g. Gmail compose) appended at sign-in.
    pub(crate) fn with_extra_scopes(mut self, scopes: Vec<String>) -> Self {
        self.extra_scopes = scopes;
        self
    }

    pub(crate) fn extra_scopes(&self) -> &[String] {
        &self.extra_scopes
    }

    pub(crate) fn service(&self) -> GoogleService {
        self.service
    }

    pub(crate) fn program(&self) -> &Path {
        &self.program
    }

    /// `gog version` as a dotted string.
    pub(crate) fn version(&self) -> Result<String, GoogleError> {
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
            return Err(GoogleError::runtime(
                "bad_json",
                "gog version printed no version".to_string(),
            ));
        }
        Ok(first.to_string())
    }

    /// Require gog ≥ 0.44.0.
    pub(crate) fn require_version(&self) -> Result<String, GoogleError> {
        let version = self.version()?;
        match parse_semver(&version) {
            Some(parsed) if parsed >= MIN_VERSION => Ok(version),
            Some(_) => Err(GoogleError::setup(
                "gog_too_old",
                format!("gog {version} is older than v0.44.0"),
            )
            .with_hint("upgrade gog to v0.44.0 or later")),
            None if version == "dev" => Ok(version),
            None => Err(GoogleError::setup(
                "gog_too_old",
                format!(
                    "gog version {version:?} is not a dotted semver ≥ 0.44.0"
                ),
            )
            .with_hint("upgrade gog to v0.44.0 or later")),
        }
    }

    pub(crate) fn credentials_list(
        &self,
    ) -> Result<Vec<GogClient>, GoogleError> {
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
            GoogleError::runtime(
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
    ) -> Result<(), GoogleError> {
        let _ = self.run_with_stdin(
            vec![
                OsString::from("--no-input"),
                OsString::from("--json"),
                OsString::from("--color"),
                OsString::from("never"),
                OsString::from("--client"),
                OsString::from(self.service.client_bucket),
                OsString::from("auth"),
                OsString::from("credentials"),
                OsString::from("set"),
                OsString::from("-"),
            ],
            Some(payload.to_vec()),
        )?;
        Ok(())
    }

    /// `gog auth list --json`. Pass the service bucket to filter Bob's grants.
    pub(crate) fn accounts(
        &self,
        client: Option<&str>,
    ) -> Result<Vec<GogAccount>, GoogleError> {
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
            GoogleError::runtime(
                "bad_json",
                format!("parse gog auth list: {error}"),
            )
        })
    }

    /// Remote `--step 1`: print the authorization URL. gog 0.44.0
    /// accepts `--force-consent` and `--redirect-uri` (verified in a
    /// throwaway `GOG_HOME`). Begin and finish share
    /// [`Self::auth_add_prefix`] so force-consent always agrees.
    pub(crate) fn auth_begin(
        &self,
        account: &str,
        redirect_uri: &str,
    ) -> Result<AuthStart, GoogleError> {
        let mut args = self.auth_add_prefix(account);
        args.extend([
            OsString::from("--remote"),
            OsString::from("--step"),
            OsString::from("1"),
            OsString::from("--redirect-uri"),
            OsString::from(redirect_uri),
        ]);
        let output = self.run(args)?;
        let value = parse_json(&output.stdout)?;
        let auth_url = value
            .get("auth_url")
            .and_then(|v| v.as_str())
            .filter(|url| !url.is_empty())
            .ok_or_else(|| {
                GoogleError::runtime(
                    "bad_json",
                    "gog remote step 1 printed no auth_url".to_string(),
                )
            })?;
        let state = state_from_auth_url(auth_url).ok_or_else(|| {
            GoogleError::runtime(
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
        redirect_uri: &str,
    ) -> Result<(), GoogleError> {
        let mut args = self.auth_add_prefix(account);
        args.extend([
            OsString::from("--remote"),
            OsString::from("--step"),
            OsString::from("2"),
            OsString::from("--auth-url"),
            OsString::from(redirect_url),
            OsString::from("--redirect-uri"),
            OsString::from(redirect_uri),
        ]);
        let _ = self.run(args)?;
        Ok(())
    }

    fn auth_add_prefix(&self, account: &str) -> Vec<OsString> {
        let mut args = vec![
            OsString::from("--no-input"),
            OsString::from("--json"),
            OsString::from("--color"),
            OsString::from("never"),
            OsString::from("--readonly"),
            OsString::from("--client"),
            OsString::from(self.service.client_bucket),
            OsString::from("auth"),
            OsString::from("add"),
            OsString::from(account),
            OsString::from("--services"),
            OsString::from(self.service.gog_service),
            OsString::from("--force-consent"),
        ];
        if self.service.gog_service == "gmail" {
            args.push(OsString::from("--gmail-scope"));
            args.push(OsString::from("readonly"));
        }
        if !self.extra_scopes.is_empty() {
            args.push(OsString::from("--extra-scopes"));
            args.push(OsString::from(self.extra_scopes.join(",")));
        }
        args
    }

    pub(crate) fn auth_remove(&self, account: &str) -> Result<(), GoogleError> {
        let _ = self.run(vec![
            OsString::from("--no-input"),
            OsString::from("--json"),
            OsString::from("--color"),
            OsString::from("never"),
            OsString::from("--client"),
            OsString::from(self.service.client_bucket),
            OsString::from("--force"),
            OsString::from("auth"),
            OsString::from("remove"),
            OsString::from(account),
        ])?;
        Ok(())
    }

    pub(crate) fn common_flags(&self, account: Option<&str>) -> Vec<OsString> {
        let mut flags = vec![
            OsString::from("--no-input"),
            OsString::from("--json"),
            OsString::from("--color"),
            OsString::from("never"),
            OsString::from("--readonly"),
            OsString::from("--client"),
            OsString::from(self.service.client_bucket),
        ];
        if let Some(account) = account {
            flags.push(OsString::from("--account"));
            flags.push(OsString::from(account));
        }
        flags
    }

    pub(crate) fn run(
        &self,
        args: Vec<OsString>,
    ) -> Result<subprocess_runner::RunOutput, GoogleError> {
        self.run_with_stdin(args, None)
    }

    pub(crate) fn run_with_stdin(
        &self,
        args: Vec<OsString>,
        stdin: Option<Vec<u8>>,
    ) -> Result<subprocess_runner::RunOutput, GoogleError> {
        let output = subprocess_runner::run(&RunSpec {
            program: self.program.clone(),
            args,
            stdin,
            env: Vec::new(),
            env_remove: Vec::new(),
            timeout: self.timeout,
        })
        .map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                GoogleError::setup(
                    "gog_missing",
                    format!("gog not found: {}", self.program.display()),
                )
                .with_hint(&format!(
                    "install gog ≥ v0.44.0 and put it on PATH, or set {}",
                    self.service.gog_override_env
                ))
            } else {
                GoogleError::runtime(
                    "gog_missing",
                    format!("start gog: {error}"),
                )
            }
        })?;
        if output.timed_out {
            return Err(GoogleError::runtime(
                "timeout",
                format!("gog timed out after {}s", self.timeout.as_secs()),
            )
            .with_hint(&format!(
                "check your network, or raise {}",
                self.service.timeout_config_key
            )));
        }
        match output.status {
            Some(0) => Ok(output),
            Some(_) | None => Err(classify_gog_failure(
                self.service,
                &output.stdout,
                &output.stderr,
            )),
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

pub(crate) fn parse_json(
    stdout: &str,
) -> Result<serde_json::Value, GoogleError> {
    serde_json::from_str(stdout.trim()).map_err(|error| {
        GoogleError::runtime(
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
fn project_console_url(
    service: GoogleService,
    stdout: &str,
    stderr: &str,
) -> Option<String> {
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
                     {}/overview?project={digits}",
                    service.api_service_id
                ));
            }
            search = after;
        }
    }
    None
}

pub(crate) fn classify_gog_failure(
    service: GoogleService,
    stdout: &str,
    stderr: &str,
) -> GoogleError {
    let blob = format!("{stdout}\n{stderr}").to_ascii_lowercase();
    if blob.contains("accessnotconfigured")
        || blob.contains("access not configured")
        || blob.contains("has not been used in project")
        || blob.contains("is disabled")
    {
        let url = project_console_url(service, stdout, stderr)
            .unwrap_or_else(|| service.api_console_url.to_string());
        return GoogleError::runtime(
            "api_disabled",
            format!(
                "the {} API is not enabled for this Cloud project",
                service.api_name
            ),
        )
        .with_hint(&format!("enable it at {url}"));
    }
    if blob.contains("store secret") || blob.contains("keyring") {
        return GoogleError::runtime(
            "keyring",
            "gog could not save the client or token in the system keyring"
                .to_string(),
        )
        .with_hint(&format!(
            "run `gog auth status` and `gog auth keyring file` to inspect the \
             keyring, then rerun `{} login`",
            service.command
        ));
    }
    if blob.contains("invalid_client")
        || blob.contains("oauth client was not found")
    {
        return GoogleError::setup(
            "invalid_client",
            "Google does not know this OAuth client (deleted or from \
             another project)"
                .to_string(),
        )
        .with_hint(&format!(
            "download a Desktop app client JSON and run \
             `{} login -c PATH`",
            service.command
        ));
    }
    if blob.contains("redirect_uri_mismatch") {
        return GoogleError::setup(
            "client_invalid",
            "Google rejected the redirect: the OAuth client is not a \
             Desktop app"
                .to_string(),
        )
        .with_hint(&format!(
            "download a Desktop app client JSON and run \
             `{} login -c PATH`",
            service.command
        ));
    }
    if blob.contains("manual auth state missing")
        || blob.contains("manual auth state mismatch")
    {
        return GoogleError::runtime(
            "stale_link",
            "that sign-in link is out of date".to_string(),
        )
        .with_hint("Bob starts a fresh sign-in link and you try once more");
    }
    if blob.contains("no code found in url")
        || blob.contains("missing state in redirect url")
    {
        return GoogleError::runtime(
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
        return GoogleError::runtime(
            "invalid_grant",
            format!("the {} grant was rejected or expired", service.service_name),
        )
        .with_hint(&format!(
            "publish the consent screen (Testing-mode tokens expire in 7 days), \
             then run `{} login`",
            service.command
        ));
    }
    if blob.contains("insufficient") && blob.contains("scope")
        || blob.contains("access_token_scope_insufficient")
        || blob.contains("insufficientpermissions")
        || blob.contains("insufficient authentication scopes")
    {
        return GoogleError::setup(
            "insufficient_scope",
            format!(
                "the stored token lacks {} read-only scope",
                service.service_name
            ),
        )
        .with_hint(&format!(
            "run `{} login` to grant {}",
            service.command, service.scope_description
        ));
    }
    if blob.contains("no stored")
        || blob.contains("not logged in")
        || blob.contains("no refresh token")
        || blob.contains("no token")
        || blob.contains("token not found")
    {
        return GoogleError::setup(
            "not_logged_in",
            format!(
                "no {} {} grant is stored",
                service.client_bucket, service.service_name
            ),
        )
        .with_hint(&format!("run `{} login`", service.command));
    }
    if blob.contains("credentials")
        && (blob.contains("not found")
            || blob.contains("no oauth client")
            || blob.contains("credentials missing"))
        || blob.contains("no client")
    {
        return GoogleError::setup(
            "no_client",
            format!("no {} OAuth client is stored", service.client_bucket),
        )
        .with_hint(&format!(
            "run `{} login -c PATH` with a Desktop-app client JSON",
            service.command
        ));
    }
    if blob.contains("connection refused")
        || blob.contains("network is unreachable")
        || blob.contains("temporary failure in name resolution")
        || blob.contains("no such host")
        || blob.contains("i/o timeout")
        || blob.contains("tls handshake")
        || blob.contains("dial tcp")
    {
        return GoogleError::runtime(
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
        return GoogleError::runtime(
            "not_found",
            format!(
                "Google has no such {} (deleted or moved away)",
                service.service_name.to_lowercase()
            ),
        );
    }
    let detail = stderr
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .or_else(|| stdout.lines().map(str::trim).find(|line| !line.is_empty()))
        .unwrap_or("gog failed");
    GoogleError::runtime("gog", detail.to_string())
}

#[cfg(test)]
mod tests {
    use super::super::profile::{GCAL, GMAIL};
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
        let error = classify_gog_failure(GCAL, "", "accessNotConfigured");
        assert_eq!(error.kind(), "api_disabled");
        assert!(error.hint().unwrap_or_default().contains("calendar-json"));

        let error = classify_gog_failure(
            GCAL,
            "",
            "accessNotConfigured: Calendar API has not been used in project \
             123456789012",
        );
        assert_eq!(error.kind(), "api_disabled");
        assert!(error
            .hint()
            .unwrap_or_default()
            .contains("project=123456789012"));

        let error = classify_gog_failure(
            GCAL,
            "",
            "invalid_grant: Token has been expired",
        );
        assert_eq!(error.kind(), "invalid_grant");
        assert!(error.hint().unwrap_or_default().contains("7 days"));
    }

    #[test]
    fn maps_keyring_invalid_client_and_redirect_mismatch() {
        let error =
            classify_gog_failure(GCAL, "", "cannot store secret in keyring");
        assert_eq!(error.kind(), "keyring");
        assert!(error
            .hint()
            .unwrap_or_default()
            .contains("gog auth keyring"));

        let error = classify_gog_failure(GCAL, "", "invalid_client: unknown");
        assert_eq!(error.kind(), "invalid_client");
        assert_eq!(error.exit_code(), 2);

        let error =
            classify_gog_failure(GCAL, "", "Error 400: redirect_uri_mismatch");
        assert_eq!(error.kind(), "client_invalid");
        assert_eq!(error.exit_code(), 2);
    }

    #[test]
    fn maps_missing_client_credentials_to_no_client() {
        // gog v0.44.0 wording when the client bucket has no OAuth client.
        let error = classify_gog_failure(
            GMAIL,
            "",
            "OAuth client credentials missing (OAuth client ID JSON).\n\
             Then run: gog auth credentials <credentials.json>",
        );
        assert_eq!(error.kind(), "no_client");
        assert_eq!(error.exit_code(), 2);
        assert!(error.message().contains("bob-gmail"));
        assert!(error.hint().unwrap_or_default().contains("bob gmail login"));
    }

    #[test]
    fn maps_remote_step_failures() {
        let error = classify_gog_failure(
            GCAL,
            "",
            "manual auth state missing; start a new manual flow",
        );
        assert_eq!(error.kind(), "stale_link");

        let error = classify_gog_failure(GCAL, "", "no code found in URL");
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
    fn one_api_failure_uses_each_service_profile() {
        let calendar = classify_gog_failure(GCAL, "", "accessNotConfigured");
        assert!(calendar.message().contains("Google Calendar API"));
        assert!(calendar
            .hint()
            .unwrap_or_default()
            .contains("calendar-json"));

        let gmail = classify_gog_failure(GMAIL, "", "accessNotConfigured");
        assert!(gmail.message().contains("Google Gmail API"));
        assert!(gmail
            .hint()
            .unwrap_or_default()
            .contains("gmail.googleapis.com"));
    }
}
