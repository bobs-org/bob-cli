//! `bob gkeep` configuration: the `gkeep:` config section.
//!
//! Parsing lives in `super::config` (`RawGkeep` plus `load_gkeep_config`,
//! following the `highlights` pattern); this module resolves defaults,
//! validates values, derives the device id, and reads the master token.

use crate::native::env as bob_env;
use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::Duration,
};

use super::{super::config as bob_config, ui::warn};
use crate::native::{
    gkeep::GkeepError,
    subprocess_runner::{self, RunSpec},
};

/// Resolved `bob gkeep` configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GkeepConfig {
    email: String,
    token_command: String,
    token_store_command: String,
    device_id: String,
    target: String,
    timeout_secs: u64,
    check_interval: CheckInterval,
}

/// How often an automatic `bob gkeep status -r` may contact Keep.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CheckInterval {
    /// Check when the cached observation is older than this.
    Every(Duration),
    /// Automatic checks are off (`-r` still works with `-m`).
    Off,
}

impl CheckInterval {
    /// Parse `Nm`, `Nh`, or `off` (case-insensitive); 5m to 24h.
    pub(crate) fn parse(text: &str) -> Result<Self, String> {
        let trimmed = text.trim().to_ascii_lowercase();
        if trimmed == "off" || trimmed == "false" {
            return Ok(Self::Off);
        }
        let bad = || {
            format!(
                "gkeep.check_interval {text:?} must be a duration like \
                 30m or 2h between 5m and 24h, or off"
            )
        };
        let (digits, unit_secs) = if let Some(rest) = trimmed.strip_suffix('m')
        {
            (rest, 60)
        } else if let Some(rest) = trimmed.strip_suffix('h') {
            (rest, 3600)
        } else {
            return Err(bad());
        };
        let count: u64 = digits.parse().map_err(|_| bad())?;
        let secs = count.checked_mul(unit_secs).ok_or_else(bad)?;
        if !(MIN_CHECK_INTERVAL_SECS..=MAX_CHECK_INTERVAL_SECS).contains(&secs)
        {
            return Err(bad());
        }
        Ok(Self::Every(Duration::from_secs(secs)))
    }
}

const MIN_CHECK_INTERVAL_SECS: u64 = 5 * 60;
const MAX_CHECK_INTERVAL_SECS: u64 = 24 * 3600;
/// Default `check_interval`: attended hours stay near 16 checks a day.
pub(crate) const DEFAULT_CHECK_INTERVAL: CheckInterval =
    CheckInterval::Every(Duration::from_secs(30 * 60));
/// Deadline for the never-prompting token read.
const UNATTENDED_TOKEN_DEADLINE: Duration = Duration::from_secs(10);

/// Default `token_command`: prints the `aas_et/…` master token.
pub(crate) const DEFAULT_TOKEN_COMMAND: &str = "pass show gkeep/master_token";
/// Default `token_store_command`: `login` pipes the token on stdin.
pub(crate) const DEFAULT_TOKEN_STORE_COMMAND: &str =
    "pass insert -m -f gkeep/master_token";
/// Default vault-relative target note.
pub(crate) const DEFAULT_TARGET: &str = "gkeep_inbox.md";
/// Default adapter timeout in seconds.
pub(crate) const DEFAULT_TIMEOUT_SECS: u64 = 300;
/// The only environment variable `bob gkeep` reads for adapter selection.
pub(crate) const ADAPTER_ENV_VAR: &str = "BOB_GKEEP_ADAPTER";

impl GkeepConfig {
    /// Resolve the config file, apply defaults, and validate.
    ///
    /// `email_override` (from `login -e`) replaces `gkeep.email`.
    /// Every failure is a setup error (exit 2).
    pub(crate) fn resolve(
        email_override: Option<&str>,
    ) -> Result<Self, GkeepError> {
        Self::resolve_at(&bob_config::config_path(), email_override)
    }

    /// Resolve for `bob gkeep status`: `Ok(None)` when Keep is not
    /// configured (no config file, no `gkeep:` section, or no
    /// `gkeep.email`); every other problem is a setup error.
    pub(crate) fn resolve_optional() -> Result<Option<Self>, GkeepError> {
        Self::resolve_optional_at(&bob_config::config_path())
    }

    fn resolve_optional_at(path: &Path) -> Result<Option<Self>, GkeepError> {
        let settings = load_settings(path)?;
        if settings
            .email
            .as_deref()
            .is_none_or(|e| e.trim().is_empty())
        {
            return Ok(None);
        }
        Self::from_settings(settings, None).map(Some)
    }

    fn resolve_at(
        path: &Path,
        email_override: Option<&str>,
    ) -> Result<Self, GkeepError> {
        Self::from_settings(load_settings(path)?, email_override)
    }

    fn from_settings(
        settings: bob_config::GkeepSettings,
        email_override: Option<&str>,
    ) -> Result<Self, GkeepError> {
        let email = email_override
            .map(str::trim)
            .filter(|email| !email.is_empty())
            .map(str::to_string)
            .or(settings.email)
            .ok_or_else(|| {
                GkeepError::setup(
                    "config",
                    "gkeep.email is not configured".to_string(),
                )
                .with_hint("set gkeep.email in ~/.config/bob/config.yml")
            })?;
        if !email.contains('@') {
            return Err(GkeepError::setup(
                "config",
                format!("gkeep.email {email:?} must contain '@'"),
            ));
        }

        let device_id = match settings.device_id {
            Some(device_id) => {
                if !is_device_id(&device_id) {
                    return Err(GkeepError::setup(
                        "config",
                        format!(
                            "gkeep.device_id {device_id:?} must be 1-16 \
                             hex digits"
                        ),
                    ));
                }
                device_id
            }
            None => derive_device_id(&email),
        };

        let target = settings
            .target
            .unwrap_or_else(|| DEFAULT_TARGET.to_string());
        let has_parent = Path::new(&target)
            .components()
            .any(|c| matches!(c, std::path::Component::ParentDir));
        if Path::new(&target).is_absolute()
            || has_parent
            || !target.ends_with(".md")
        {
            return Err(GkeepError::setup(
                "config",
                format!(
                    "gkeep.target {target:?} must be vault-relative and \
                     end in .md"
                ),
            ));
        }

        let timeout_secs =
            settings.timeout_secs.unwrap_or(DEFAULT_TIMEOUT_SECS);
        if timeout_secs == 0 {
            return Err(GkeepError::setup(
                "config",
                "gkeep.timeout_secs must be greater than 0".to_string(),
            ));
        }

        let check_interval = match settings.check_interval.as_deref() {
            Some(text) => CheckInterval::parse(text)
                .map_err(|message| GkeepError::setup("config", message))?,
            None => DEFAULT_CHECK_INTERVAL,
        };

        Ok(Self {
            email,
            token_command: settings
                .token_command
                .unwrap_or_else(|| DEFAULT_TOKEN_COMMAND.to_string()),
            token_store_command: settings
                .token_store_command
                .unwrap_or_else(|| DEFAULT_TOKEN_STORE_COMMAND.to_string()),
            device_id,
            target,
            timeout_secs,
            check_interval,
        })
    }

    /// The Keep account email.
    pub(crate) fn email(&self) -> &str {
        &self.email
    }

    /// The configured or derived Android device id.
    pub(crate) fn device_id(&self) -> &str {
        &self.device_id
    }

    /// The vault-relative target note name.
    pub(crate) fn target(&self) -> &str {
        &self.target
    }

    /// The adapter timeout in seconds.
    pub(crate) fn timeout_secs(&self) -> u64 {
        self.timeout_secs
    }

    /// How stale a cached observation may get before an automatic
    /// `status -r` contacts Keep.
    pub(crate) fn check_interval(&self) -> CheckInterval {
        self.check_interval
    }

    /// The command `login` pipes a fresh token into.
    pub(crate) fn token_store_command(&self) -> &str {
        &self.token_store_command
    }

    /// The command that prints the stored master token.
    pub(crate) fn token_command(&self) -> &str {
        &self.token_command
    }

    /// A config for tests: no file or environment reads.
    #[cfg(test)]
    pub(crate) fn for_tests(
        email: &str,
        device_id: &str,
        timeout_secs: u64,
    ) -> Self {
        Self {
            email: email.to_string(),
            token_command: DEFAULT_TOKEN_COMMAND.to_string(),
            token_store_command: DEFAULT_TOKEN_STORE_COMMAND.to_string(),
            device_id: device_id.to_string(),
            target: DEFAULT_TARGET.to_string(),
            timeout_secs,
            check_interval: DEFAULT_CHECK_INTERVAL,
        }
    }

    /// A test config with a custom token command.
    #[cfg(test)]
    pub(crate) fn with_token_command(mut self, command: &str) -> Self {
        self.token_command = command.to_string();
        self
    }

    /// The target note path under `bob_dir`.
    pub(crate) fn target_path(&self, bob_dir: &Path) -> PathBuf {
        bob_dir.join(&self.target)
    }

    /// The test hook: an executable speaking the adapter protocol that
    /// replaces `uv run --script …`, like `BOB_CLIPBOARD_CMD`.
    pub(crate) fn adapter_override() -> Option<PathBuf> {
        Self::adapter_override_from(bob_env::var_os(ADAPTER_ENV_VAR))
    }

    /// Pure helper for tests: resolve the override from an env value.
    pub(crate) fn adapter_override_from(
        value: Option<std::ffi::OsString>,
    ) -> Option<PathBuf> {
        value.filter(|v| !v.is_empty()).map(PathBuf::from)
    }

    /// Run `token_command` with `sh -c` and return the master token plus
    /// its shape. Stdin and stderr are inherited so `pass` can use
    /// pinentry; the token is the first non-empty stdout line, trimmed.
    /// Tokens never appear in errors.
    pub(crate) fn read_token(
        &self,
    ) -> Result<(String, TokenShape), GkeepError> {
        let output = Command::new("sh")
            .arg("-c")
            .arg(&self.token_command)
            .stdin(Stdio::inherit())
            .stderr(Stdio::inherit())
            .output()
            .map_err(|error| {
                GkeepError::setup(
                    "token",
                    format!("run the token command: {error}"),
                )
                .with_hint("run `bob gkeep login`")
            })?;
        if !output.status.success() {
            return Err(GkeepError::setup(
                "token",
                "the token command failed".to_string(),
            )
            .with_hint("run `bob gkeep login`"));
        }
        let token = String::from_utf8_lossy(&output.stdout)
            .lines()
            .map(str::trim)
            .find(|line| !line.is_empty())
            .unwrap_or_default()
            .to_string();
        if token.is_empty() {
            return Err(GkeepError::setup(
                "token",
                "the token command printed no token".to_string(),
            )
            .with_hint("run `bob gkeep login`"));
        }
        match TokenShape::classify(&token) {
            TokenShape::SignInCookie => Err(GkeepError::setup(
                "token",
                "the stored token is a sign-in cookie, not a master token"
                    .to_string(),
            )
            .with_hint(
                "this is a sign-in cookie, not a master token; run \
                 `bob gkeep login`",
            )),
            TokenShape::Unknown => {
                warn(
                    "the stored token has an unrecognized shape; trying it \
                     anyway",
                );
                Ok((token, TokenShape::Unknown))
            }
            TokenShape::MasterToken => Ok((token, TokenShape::MasterToken)),
        }
    }
}

impl GkeepConfig {
    /// Read the master token without ever prompting.
    ///
    /// Runs `token_command` with `sh -c`: stdin is empty, stdout is
    /// piped, stderr is captured (only a scrubbed first line is kept),
    /// `GPG_TTY` is removed, and `--pinentry-mode error --batch` is
    /// appended to `PASSWORD_STORE_GPG_OPTS` so `pass` fails instead of
    /// asking for a passphrase. A 10 s deadline kills the whole process
    /// group. Any failure, timeout, or empty output is a `locked` error;
    /// a sign-in cookie is `auth`. The token never appears in an error.
    pub(crate) fn read_token_unattended(&self) -> Result<String, GkeepError> {
        self.read_token_unattended_within(UNATTENDED_TOKEN_DEADLINE)
    }

    fn read_token_unattended_within(
        &self,
        deadline: Duration,
    ) -> Result<String, GkeepError> {
        let mut gpg_opts =
            bob_env::var_os("PASSWORD_STORE_GPG_OPTS").unwrap_or_default();
        if !gpg_opts.is_empty() {
            gpg_opts.push(" ");
        }
        gpg_opts.push("--pinentry-mode error --batch");
        let locked = |message: String| {
            GkeepError::runtime("locked", message).with_hint(
                "run `bob gkeep list` in a terminal once to unlock the token",
            )
        };
        let output = subprocess_runner::run(&RunSpec {
            program: PathBuf::from("sh"),
            args: vec![
                OsString::from("-c"),
                OsString::from(&self.token_command),
            ],
            stdin: None,
            env: vec![(OsString::from("PASSWORD_STORE_GPG_OPTS"), gpg_opts)],
            env_remove: vec![OsString::from("GPG_TTY")],
            timeout: deadline,
        })
        .map_err(|error| locked(format!("run the token command: {error}")))?;
        if output.timed_out {
            return Err(locked(format!(
                "the token command did not finish within {}s without a \
                 prompt",
                deadline.as_secs().max(1)
            )));
        }
        if output.status != Some(0) {
            return Err(locked(with_detail(
                "the token command failed without a prompt",
                &output.stderr,
            )));
        }
        let token = output
            .stdout
            .lines()
            .map(str::trim)
            .find(|line| !line.is_empty())
            .unwrap_or_default()
            .to_string();
        if token.is_empty() {
            return Err(locked(with_detail(
                "the token command printed no token",
                &output.stderr,
            )));
        }
        if TokenShape::classify(&token) == TokenShape::SignInCookie {
            return Err(GkeepError::runtime(
                "auth",
                "the stored token is a sign-in cookie, not a master token"
                    .to_string(),
            )
            .with_hint("run `bob gkeep login`"));
        }
        Ok(token)
    }
}

/// `message`, plus the first non-empty stderr line with token-shaped
/// words redacted and the line capped.
fn with_detail(message: &str, stderr: &str) -> String {
    let Some(line) = stderr.lines().map(str::trim).find(|l| !l.is_empty())
    else {
        return message.to_string();
    };
    let scrubbed = line
        .split_whitespace()
        .map(|word| {
            if word.starts_with("aas_et/") || word.starts_with("oauth2_4/") {
                "[redacted]"
            } else {
                word
            }
        })
        .collect::<Vec<_>>()
        .join(" ");
    let capped: String = scrubbed.chars().take(160).collect();
    format!("{message}: {capped}")
}

fn load_settings(path: &Path) -> Result<bob_config::GkeepSettings, GkeepError> {
    bob_config::load_gkeep_config(path).map_err(|error| {
        let message = match error {
            bob_config::ConfigError::Read(message)
            | bob_config::ConfigError::Invalid(message) => message,
        };
        GkeepError::setup("config", format!("read the gkeep config: {message}"))
            .with_hint(
                "set BOB_CONFIG_FILE or run 'chezmoi apply \
                 ~/.config/bob/config.yml'",
            )
    })
}

/// Whether `device_id` is 1–16 hex digits.
fn is_device_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 16
        && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// The default device id: `hex(sha256("bob-gkeep-device:" +
/// lowercase(email)))[..16]`, so every host presents one stable Android
/// device id with zero config.
pub(crate) fn derive_device_id(email: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(b"bob-gkeep-device:");
    hasher.update(email.to_lowercase().as_bytes());
    hex::encode(hasher.finalize()).chars().take(16).collect()
}

/// The shape of a stored token.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TokenShape {
    /// `aas_et/…`: the Keep master token.
    MasterToken,
    /// `oauth2_4/…`: a sign-in cookie; an error, not a token.
    SignInCookie,
    /// Anything else: a warning; still tried.
    Unknown,
}

impl TokenShape {
    /// Classify the first non-empty token line.
    pub(crate) fn classify(token: &str) -> Self {
        if token.starts_with("aas_et/") {
            Self::MasterToken
        } else if token.starts_with("oauth2_4/") {
            Self::SignInCookie
        } else {
            Self::Unknown
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_id_is_derived_and_pinned() {
        assert_eq!(
            derive_device_id("bryanbugyi34@gmail.com"),
            "9dafe47ebb91af00"
        );
        assert_eq!(
            derive_device_id("BryanBugyi34@Gmail.Com"),
            derive_device_id("bryanbugyi34@gmail.com")
        );
    }

    #[test]
    fn token_shape_classifier() {
        assert_eq!(TokenShape::classify("aas_et/abc"), TokenShape::MasterToken);
        assert_eq!(
            TokenShape::classify("oauth2_4/abc"),
            TokenShape::SignInCookie
        );
        assert_eq!(TokenShape::classify("xyz"), TokenShape::Unknown);
        assert_eq!(TokenShape::classify(""), TokenShape::Unknown);
    }

    #[test]
    fn device_id_validation() {
        assert!(is_device_id("3f9c0a1b2c3d4e5f"));
        assert!(is_device_id("a"));
        assert!(!is_device_id(""));
        assert!(!is_device_id("3f9c0a1b2c3d4e5f00"));
        assert!(!is_device_id("xyz"));
    }

    fn resolve_with(
        dir: &tempfile::TempDir,
        text: &str,
        email: Option<&str>,
    ) -> Result<GkeepConfig, GkeepError> {
        let path = dir.path().join("config.yml");
        std::fs::write(&path, text).expect("write config");
        GkeepConfig::resolve_at(&path, email)
    }

    fn fresh_dir() -> tempfile::TempDir {
        tempfile::tempdir().expect("temp dir")
    }

    #[test]
    fn resolve_applies_defaults_and_derives_device_id() {
        let dir = fresh_dir();
        let config = resolve_with(
            &dir,
            "gkeep:\n  email: bryanbugyi34@gmail.com\n",
            None,
        )
        .expect("valid config resolves");
        assert_eq!(config.email(), "bryanbugyi34@gmail.com");
        assert_eq!(config.device_id(), "9dafe47ebb91af00");
        assert_eq!(config.target(), "gkeep_inbox.md");
        assert_eq!(config.timeout_secs(), 300);
        assert_eq!(
            config.target_path(Path::new("/vault")),
            PathBuf::from("/vault/gkeep_inbox.md")
        );
        assert_eq!(
            config.token_store_command(),
            "pass insert -m -f gkeep/master_token"
        );
    }

    #[test]
    fn resolve_email_override_wins() {
        let dir = fresh_dir();
        let config = resolve_with(
            &dir,
            "gkeep:\n  email: bryanbugyi34@gmail.com\n",
            Some("other@example.com"),
        )
        .expect("override resolves");
        assert_eq!(config.email(), "other@example.com");
        assert_eq!(config.device_id(), derive_device_id("other@example.com"));
    }

    #[test]
    fn resolve_rejects_bad_config() {
        let dir = fresh_dir();
        for (name, text) in [
            ("missing email", "gkeep:\n  target: gkeep_inbox.md\n"),
            ("bad email", "gkeep:\n  email: not-an-email\n"),
            (
                "bad device id",
                "gkeep:\n  email: a@b.c\n  device_id: xyz\n",
            ),
            (
                "absolute target",
                "gkeep:\n  email: a@b.c\n  target: /x.md\n",
            ),
            (
                "parent target",
                "gkeep:\n  email: a@b.c\n  target: ../escape.md\n",
            ),
            (
                "nested parent target",
                "gkeep:\n  email: a@b.c\n  target: sub/../../escape.md\n",
            ),
            (
                "non-markdown target",
                "gkeep:\n  email: a@b.c\n  target: notes.txt\n",
            ),
            (
                "zero timeout",
                "gkeep:\n  email: a@b.c\n  timeout_secs: 0\n",
            ),
        ] {
            let error = resolve_with(&dir, text, None)
                .expect_err(&format!("{name} must fail"));
            assert_eq!(error.exit_code(), 2, "{name} exits 2");
        }
    }

    fn config_with_token_command(command: &str) -> GkeepConfig {
        let dir = fresh_dir();
        // The file must outlive `resolve_at` only; the config is owned.
        resolve_with(
            &dir,
            &format!(
                "gkeep:\n  email: a@b.c\n  token_command: \"{command}\"\n"
            ),
            None,
        )
        .expect("config resolves")
    }

    #[test]
    fn read_token_returns_first_non_empty_line() {
        let config =
            config_with_token_command("printf '\\n  aas_et/test-token \\n\\n'");
        let (token, shape) = config.read_token().expect("token reads");
        assert_eq!(token, "aas_et/test-token");
        assert_eq!(shape, TokenShape::MasterToken);
    }

    #[test]
    fn read_token_rejects_sign_in_cookie() {
        let config = config_with_token_command("printf 'oauth2_4/cookie\\n'");
        let error = config.read_token().expect_err("cookie must fail");
        assert_eq!(error.exit_code(), 2);
        assert!(
            error
                .hint()
                .unwrap_or_default()
                .contains("run `bob gkeep login`"),
            "cookie hint names login"
        );
    }

    #[test]
    fn read_token_rejects_empty_and_failing_commands() {
        for command in ["printf ''", "exit 3"] {
            let config = config_with_token_command(command);
            let error = config
                .read_token()
                .expect_err(&format!("{command} must fail"));
            assert_eq!(error.exit_code(), 2);
        }
    }

    #[test]
    fn read_token_warns_but_keeps_unknown_shapes() {
        let config = config_with_token_command("printf 'weird\\n'");
        let (token, shape) = config.read_token().expect("unknown kept");
        assert_eq!(token, "weird");
        assert_eq!(shape, TokenShape::Unknown);
    }

    #[test]
    fn check_interval_parses_minutes_hours_and_off() {
        let every = |secs| CheckInterval::Every(Duration::from_secs(secs));
        assert_eq!(CheckInterval::parse("30m"), Ok(every(1800)));
        assert_eq!(CheckInterval::parse(" 5m "), Ok(every(300)));
        assert_eq!(CheckInterval::parse("2H"), Ok(every(7200)));
        assert_eq!(CheckInterval::parse("24h"), Ok(every(86400)));
        assert_eq!(CheckInterval::parse("off"), Ok(CheckInterval::Off));
        assert_eq!(CheckInterval::parse("OFF"), Ok(CheckInterval::Off));
        for bad in [
            "4m", "25h", "1441m", "0m", "30", "m", "", "1d", "1.5h", "-5m",
        ] {
            assert!(
                CheckInterval::parse(bad).is_err(),
                "{bad:?} must be rejected"
            );
        }
    }

    #[test]
    fn resolve_check_interval_defaults_and_validates() {
        let dir = fresh_dir();
        let config =
            resolve_with(&dir, "gkeep:\n  email: a@b.c\n", None).expect("ok");
        assert_eq!(config.check_interval(), DEFAULT_CHECK_INTERVAL);
        for (value, expected) in [
            ("2h", CheckInterval::Every(Duration::from_secs(7200))),
            ("off", CheckInterval::Off),
            ("false", CheckInterval::Off),
        ] {
            let config = resolve_with(
                &dir,
                &format!("gkeep:\n  email: a@b.c\n  check_interval: {value}\n"),
                None,
            )
            .expect("valid interval");
            assert_eq!(config.check_interval(), expected, "{value}");
        }
        for value in ["1m", "48h", "30", "true"] {
            let error = resolve_with(
                &dir,
                &format!("gkeep:\n  email: a@b.c\n  check_interval: {value}\n"),
                None,
            )
            .expect_err(&format!("{value} must fail"));
            assert_eq!(error.exit_code(), 2, "{value}");
            assert_eq!(error.kind(), "config", "{value}");
        }
    }

    #[test]
    fn resolve_optional_treats_missing_setup_as_unconfigured() {
        let dir = fresh_dir();
        let at = |text: &str| {
            let path = dir.path().join("optional.yml");
            std::fs::write(&path, text).expect("write config");
            GkeepConfig::resolve_optional_at(&path)
        };
        assert_eq!(at("").expect("empty file"), None);
        assert_eq!(at("other: 1\n").expect("no gkeep section"), None);
        assert_eq!(at("gkeep:\n  target: x.md\n").expect("no email"), None);
        assert!(at("gkeep:\n  email: a@b.c\n")
            .expect("configured")
            .is_some());
        assert_eq!(
            GkeepConfig::resolve_optional_at(Path::new(
                "/definitely/missing.yml"
            ))
            .expect("missing file"),
            None
        );
        let error = at("gkeep: [unterminated").expect_err("invalid yaml");
        assert_eq!(error.exit_code(), 2);
        let error =
            at("gkeep:\n  email: not-an-email\n").expect_err("bad email");
        assert_eq!(error.exit_code(), 2);
    }

    fn unattended(
        command: &str,
        deadline: Duration,
    ) -> Result<String, GkeepError> {
        GkeepConfig::for_tests("a@b.c", "abc123", 30)
            .with_token_command(command)
            .read_token_unattended_within(deadline)
    }

    #[test]
    fn unattended_read_returns_the_first_line() {
        let token = unattended(
            "printf '\\n  aas_et/quiet \\n'",
            Duration::from_secs(5),
        )
        .expect("token reads");
        assert_eq!(token, "aas_et/quiet");
    }

    #[test]
    fn unattended_read_failures_are_locked_without_leaking() {
        for command in [
            "exit 3",
            "printf ''",
            "cat",
            "echo 'gpg: aas_et/leaky failed' >&2; exit 2",
        ] {
            let error = unattended(command, Duration::from_secs(5))
                .expect_err(&format!("{command} must fail"));
            assert_eq!(error.kind(), "locked", "{command}");
            assert!(!error.message().contains("aas_et/leaky"), "{command}");
            assert!(error.hint().is_some(), "{command}");
        }
        let error = unattended(
            "echo 'gpg: aas_et/leaky failed' >&2; exit 2",
            Duration::from_secs(5),
        )
        .expect_err("fails");
        assert!(
            error.message().contains("[redacted]"),
            "{}",
            error.message()
        );
    }

    #[test]
    fn unattended_read_kills_a_command_that_outlives_the_deadline() {
        let started = std::time::Instant::now();
        let error = unattended("sleep 30", Duration::from_millis(300))
            .expect_err("deadline");
        assert!(started.elapsed() < Duration::from_secs(10));
        assert_eq!(error.kind(), "locked");
        assert!(
            error.message().contains("did not finish"),
            "{}",
            error.message()
        );
    }

    #[test]
    fn unattended_read_reports_a_sign_in_cookie_as_auth() {
        let error =
            unattended("printf 'oauth2_4/cookie\\n'", Duration::from_secs(5))
                .expect_err("cookie must fail");
        assert_eq!(error.kind(), "auth");
        assert!(!error.message().contains("oauth2_4/cookie"));
    }

    #[test]
    fn unattended_read_appends_pinentry_options() {
        // Overrides are thread-local; GPG_TTY removal is covered by the
        // `bob gkeep status` integration test, which controls the env.
        let _opts = bob_env::TestEnvGuard::set(&[(
            "PASSWORD_STORE_GPG_OPTS",
            Some(std::ffi::OsStr::new("--quiet")),
        )]);
        let token = unattended(
            "case \"$PASSWORD_STORE_GPG_OPTS\" in \
               '--quiet --pinentry-mode error --batch') echo aas_et/hardened;; \
               *) exit 8;; esac",
            Duration::from_secs(5),
        );
        assert_eq!(token.expect("hardened env"), "aas_et/hardened");

        let _unset =
            bob_env::TestEnvGuard::set(&[("PASSWORD_STORE_GPG_OPTS", None)]);
        let token = unattended(
            "[ \"$PASSWORD_STORE_GPG_OPTS\" = '--pinentry-mode error --batch' ] \
             && echo aas_et/bare",
            Duration::from_secs(5),
        );
        assert_eq!(token.expect("bare opts"), "aas_et/bare");
    }

    #[test]
    fn adapter_override_reads_env() {
        assert_eq!(GkeepConfig::adapter_override_from(None), None);
        assert_eq!(
            GkeepConfig::adapter_override_from(Some(std::ffi::OsString::from(
                ""
            ))),
            None
        );
        assert_eq!(
            GkeepConfig::adapter_override_from(Some(std::ffi::OsString::from(
                "/tmp/fake-adapter"
            ))),
            Some(PathBuf::from("/tmp/fake-adapter"))
        );
    }
}
