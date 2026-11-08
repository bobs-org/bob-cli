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
    /// Resolve the service-specific executable override, config value, PATH,
    /// then the well-known Homebrew and user-local locations.
    pub(crate) fn resolve(
        service: GoogleService,
        configured: Option<&Path>,
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
            None => match configured {
                Some(path) if bob_env::is_executable_file(path) => {
                    path.to_path_buf()
                }
                Some(path) => {
                    return Err(GoogleError::setup(
                        "gog_missing",
                        format!(
                            "configured gog executable is missing or not executable: {}",
                            path.display()
                        ),
                    )
                    .with_hint(&format!(
                        "fix {} in the Bob config file",
                        service.gog_config_key
                    )));
                }
                None => resolve_gog_from_path_or_fallback().ok_or_else(|| {
                    GoogleError::setup(
                        "gog_missing",
                        "gog not found on PATH or in a standard install location"
                            .to_string(),
                    )
                    .with_hint(&format!(
                        "install gog ≥ v0.44.0 and put it on PATH, or set {} in the Bob config file",
                        service.gog_config_key
                    ))
                })?,
            },
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
            env: vec![(
                OsString::from("XDG_CONFIG_HOME"),
                config_root().into_os_string(),
            )],
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

/// Bob's config root, resolved with the same XDG and tilde rules as
/// `bob/config.yml`. gog always receives this value, independent of how Bob
/// itself was launched.
pub(crate) fn config_root() -> PathBuf {
    bob_env::var_os("XDG_CONFIG_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .map(|path| bob_env::expand_tilde(&path))
        .unwrap_or_else(|| bob_env::home_dir().join(".config"))
}

pub(crate) fn gog_store_dir() -> PathBuf {
    config_root().join("gogcli")
}

/// Display gog's store with Bob's home abbreviated as `~` when possible.
pub(crate) fn gog_store_display() -> String {
    let store = gog_store_dir();
    let home = bob_env::home_dir();
    match store.strip_prefix(&home) {
        Ok(relative) if relative.as_os_str().is_empty() => "~".to_string(),
        Ok(relative) => format!("~/{}", relative.display()),
        Err(_) => store.display().to_string(),
    }
}

fn resolve_gog_from_path_or_fallback() -> Option<PathBuf> {
    let home = bob_env::home_dir();
    resolve_gog_candidates(
        bob_env::find_on_path("gog"),
        [
            home.join(".local/bin/gog"),
            PathBuf::from("/opt/homebrew/bin/gog"),
            PathBuf::from("/usr/local/bin/gog"),
        ],
    )
}

fn resolve_gog_candidates(
    path_match: Option<PathBuf>,
    fallback_candidates: impl IntoIterator<Item = PathBuf>,
) -> Option<PathBuf> {
    path_match.or_else(|| {
        fallback_candidates
            .into_iter()
            .find(|path| bob_env::is_executable_file(path))
    })
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
    if blob.contains("ratelimitexceeded")
        || blob.contains("rate_limit_exceeded")
        || blob.contains("rate-limit-exceeded")
        || blob.contains("rate limit exceeded")
        || blob.contains("quotaexceeded")
        || blob.contains("quota_exceeded")
        || blob.contains("quota-exceeded")
        || blob.contains("quota exceeded")
        || blob.contains("429")
    {
        return GoogleError::runtime(
            "rate_limited",
            format!("Google throttled {} requests", service.service_name),
        )
        .with_hint("wait and run again; the next run resumes");
    }
    if blob.contains("backenderror")
        || blob.contains("internalerror")
        || blob.contains("serviceunavailable")
        || blob.contains("badgateway")
        || blob.contains("gatewaytimeout")
        || blob.contains("\"code\":500")
        || blob.contains("\"code\": 500")
        || blob.contains("\"code\":502")
        || blob.contains("\"code\": 502")
        || blob.contains("\"code\":503")
        || blob.contains("\"code\": 503")
        || blob.contains("\"code\":504")
        || blob.contains("\"code\": 504")
    {
        return GoogleError::runtime(
            "transient",
            format!("Google had a transient {} failure", service.service_name),
        )
        .with_hint("wait and run again; the next run resumes");
    }
    if blob.contains("store secret") || blob.contains("keyring") {
        if blob.contains("integrity check failed") {
            return GoogleError::runtime(
                "keyring",
                "gog could not unlock its file keyring (wrong or empty GOG_KEYRING_PASSWORD)"
                    .to_string(),
            )
            .with_hint(&format!(
                "check the password source used by {}",
                service.gog_config_key
            ));
        }
        if blob.contains("gog_keyring_password")
            || blob.contains("no tty available for keyring")
        {
            return GoogleError::runtime(
                "keyring",
                "gog's file keyring is locked: this run has no GOG_KEYRING_PASSWORD"
                    .to_string(),
            )
            .with_hint(&format!(
                "point {} at a gog wrapper that exports GOG_KEYRING_PASSWORD, for example `~/bin/gog`",
                service.gog_config_key
            ));
        }
        return GoogleError::runtime(
            "keyring",
            "gog could not read or save the client or token in the system keyring"
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
        let message = expected_client_store_dir(stdout, stderr).map_or_else(
            || format!("no {} OAuth client is stored", service.client_bucket),
            |directory| {
                format!(
                    "no {} OAuth client is stored in {}",
                    service.client_bucket,
                    directory.display()
                )
            },
        );
        return GoogleError::setup("no_client", message).with_hint(&format!(
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

fn expected_client_store_dir(stdout: &str, stderr: &str) -> Option<PathBuf> {
    for text in [stdout, stderr] {
        let lower = text.to_ascii_lowercase();
        let Some(start) = lower.find("expected at ") else {
            continue;
        };
        let path_start = start + "expected at ".len();
        let rest = text.get(path_start..)?;
        let path = rest.split(['\n', ')']).next()?.trim().trim_end_matches('.');
        if let Some(parent) = Path::new(path).parent() {
            return Some(parent.to_path_buf());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::super::profile::{GCAL, GMAIL};
    use super::*;
    use std::{
        fs,
        time::{SystemTime, UNIX_EPOCH},
    };

    fn temp_root(label: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        std::env::temp_dir()
            .join(format!("bob-gog-{label}-{}-{nonce}", std::process::id()))
    }

    fn make_executable(path: &Path) {
        fs::create_dir_all(path.parent().expect("parent"))
            .expect("create executable parent");
        fs::write(path, "#!/bin/sh\nexit 0\n").expect("write executable");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(path, fs::Permissions::from_mode(0o755))
                .expect("chmod executable");
        }
    }

    #[test]
    fn semver_parse_and_compare() {
        assert_eq!(parse_semver("0.44.0"), Some((0, 44, 0)));
        assert_eq!(parse_semver("v0.44.1"), Some((0, 44, 1)));
        assert!(parse_semver("0.44.0").unwrap() >= MIN_VERSION);
        assert!(parse_semver("0.43.9").unwrap() < MIN_VERSION);
    }

    #[test]
    fn resolves_gog_in_environment_config_path_then_path_then_fallback_order() {
        let root = temp_root("resolution");
        let home = root.join("home");
        let configured = root.join("configured/gog");
        let override_path = root.join("override/gog");
        let path_bin = root.join("path/gog");
        let fallback = home.join(".local/bin/gog");
        for executable in [&configured, &override_path, &path_bin, &fallback] {
            make_executable(executable);
        }
        let path_dir = path_bin.parent().expect("PATH directory");
        let home_text = home.as_os_str();
        let path_text = path_dir.as_os_str();
        let override_text = override_path.as_os_str();
        let _base = bob_env::TestEnvGuard::set(&[
            ("HOME", Some(home_text)),
            ("PATH", Some(path_text)),
            ("BOB_GCAL_GOG", Some(override_text)),
        ]);

        let resolved =
            Gog::resolve(GCAL, Some(&configured), Duration::from_secs(1))
                .expect("environment override wins");
        assert_eq!(resolved.program(), override_path.as_path());

        let _without_override =
            bob_env::TestEnvGuard::set(&[("BOB_GCAL_GOG", None)]);
        let resolved =
            Gog::resolve(GCAL, Some(&configured), Duration::from_secs(1))
                .expect("configured executable wins");
        assert_eq!(resolved.program(), configured.as_path());

        let resolved = Gog::resolve(GCAL, None, Duration::from_secs(1))
            .expect("PATH executable wins over fallbacks");
        assert_eq!(resolved.program(), path_bin.as_path());

        let _without_path = bob_env::TestEnvGuard::set(&[(
            "PATH",
            Some(std::ffi::OsStr::new("/no-such-bob-gog-path")),
        )]);
        let resolved = Gog::resolve(GCAL, None, Duration::from_secs(1))
            .expect("user-local fallback is found");
        assert_eq!(resolved.program(), fallback.as_path());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn config_root_matches_bobs_xdg_and_home_rules() {
        let home = temp_root("config-root");
        let _env = bob_env::TestEnvGuard::set(&[
            ("HOME", Some(home.as_os_str())),
            ("XDG_CONFIG_HOME", None),
        ]);
        assert_eq!(config_root(), home.join(".config"));
        let xdg = PathBuf::from("~/settings");
        let _xdg = bob_env::TestEnvGuard::set(&[(
            "XDG_CONFIG_HOME",
            Some(xdg.as_os_str()),
        )]);
        assert_eq!(config_root(), home.join("settings"));
        let _ = fs::remove_dir_all(home);
    }

    #[test]
    fn standard_gog_fallbacks_keep_their_priority() {
        let root = temp_root("fallback-order");
        let path_match = root.join("path/gog");
        let user_local = root.join("home/.local/bin/gog");
        let homebrew = root.join("opt-homebrew/bin/gog");
        let local = root.join("usr-local/bin/gog");
        for executable in [&path_match, &user_local, &homebrew, &local] {
            make_executable(executable);
        }
        let candidates =
            || vec![user_local.clone(), homebrew.clone(), local.clone()];
        assert_eq!(
            resolve_gog_candidates(None, candidates()),
            Some(user_local.clone())
        );
        fs::remove_file(&user_local).expect("remove user-local gog");
        assert_eq!(
            resolve_gog_candidates(None, candidates()),
            Some(homebrew.clone())
        );
        fs::remove_file(&homebrew).expect("remove Homebrew gog");
        assert_eq!(
            resolve_gog_candidates(None, candidates()),
            Some(local.clone())
        );
        assert_eq!(
            resolve_gog_candidates(Some(path_match.clone()), candidates()),
            Some(path_match)
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn maps_rate_limit_and_transient_before_redaction() {
        for stderr in [
            "rateLimitExceeded: Rate limit exceeded",
            "User-rate-limit-exceeded",
            "429 Too Many Requests",
            "quotaExceeded: Quota exceeded",
        ] {
            let error = classify_gog_failure(GMAIL, "", stderr);
            assert_eq!(error.kind(), "rate_limited", "{stderr}");
            assert_eq!(error.exit_code(), 1);
        }
        for stderr in [
            "backendError: Backend error",
            "internalError",
            "serviceUnavailable",
            "\"code\":503",
        ] {
            let error = classify_gog_failure(GMAIL, "", stderr);
            assert_eq!(error.kind(), "transient", "{stderr}");
            assert_eq!(error.exit_code(), 1);
        }
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
    fn launch_environment_failures_name_the_keyring_and_store() {
        let locked = classify_gog_failure(
            GCAL,
            "",
            "read encoded file keyring item: no TTY available for keyring file backend password prompt; set GOG_KEYRING_PASSWORD",
        );
        assert_eq!(locked.kind(), "keyring");
        assert_eq!(locked.exit_code(), 1);
        assert_eq!(
            locked.message(),
            "gog's file keyring is locked: this run has no GOG_KEYRING_PASSWORD"
        );
        assert!(locked.hint().unwrap_or_default().contains("gcal.gog"));
        assert!(locked.hint().unwrap_or_default().contains("~/bin/gog"));

        let integrity = classify_gog_failure(
            GCAL,
            "",
            "read encoded file keyring item: aes.KeyUnwrap(): integrity check failed",
        );
        assert_eq!(integrity.kind(), "keyring");
        assert_eq!(integrity.exit_code(), 1);
        assert_eq!(
            integrity.message(),
            "gog could not unlock its file keyring (wrong or empty GOG_KEYRING_PASSWORD)"
        );
        assert!(integrity
            .hint()
            .unwrap_or_default()
            .contains("password source"));

        let missing_client = classify_gog_failure(
            GCAL,
            "",
            "OAuth client credentials missing (OAuth client ID JSON). … (expected at /Users/bbugyi/Library/Application Support/gogcli/credentials-bob-gcal.json)",
        );
        assert_eq!(missing_client.kind(), "no_client");
        assert_eq!(missing_client.exit_code(), 2);
        assert_eq!(
            missing_client.message(),
            "no bob-gcal OAuth client is stored in /Users/bbugyi/Library/Application Support/gogcli"
        );
        assert!(missing_client
            .hint()
            .unwrap_or_default()
            .contains("bob gcal login -c PATH"));
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
