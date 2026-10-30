//! OAuth client JSON parsing and normalization for `bob gcal login`.
//!
//! Real Google downloads are `{"installed": {...}}`; gog's own stored
//! legacy clients are flat `{"client_id": ..., "client_secret": ...}`.
//! Both are accepted. `web` clients, service accounts, and gcloud
//! `authorized_user` files are rejected with a fix. Secrets never appear
//! in error text.

use super::GcalError;

/// Where an [`OAuthClient`] came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub(crate) enum ClientOrigin {
    File,
    Stored,
    GogClient,
    Downloads,
    Pasted,
}

impl ClientOrigin {
    #[allow(dead_code)]
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::File => "file",
            Self::Stored => "stored",
            Self::GogClient => "gog_client",
            Self::Downloads => "downloads",
            Self::Pasted => "pasted",
        }
    }
}

/// A validated OAuth client ready to pipe to gog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OAuthClient {
    pub client_id: String,
    pub client_secret: String,
    pub project: String,
    pub origin: ClientOrigin,
}

impl OAuthClient {
    /// Emit `{"installed":{"client_id":…,"client_secret":…}}` for
    /// `gog auth credentials set -`. The secret lives only in stdin.
    pub(crate) fn to_gog_json(&self) -> Vec<u8> {
        let value = serde_json::json!({
            "installed": {
                "client_id": self.client_id,
                "client_secret": self.client_secret,
            }
        });
        serde_json::to_vec(&value).unwrap_or_default()
    }
}

/// A user-facing client JSON problem (always `client_invalid`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ClientProblem {
    message: String,
    hint: String,
}

impl ClientProblem {
    fn new(message: impl Into<String>, hint: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            hint: hint.into(),
        }
    }

    #[allow(dead_code)]
    pub(crate) fn message(&self) -> &str {
        &self.message
    }

    #[allow(dead_code)]
    pub(crate) fn hint(&self) -> &str {
        &self.hint
    }

    /// Attach the file path and convert to a `client_invalid` setup error.
    pub(crate) fn into_error(self, file: &str) -> GcalError {
        GcalError::setup(
            "client_invalid",
            format!("{}: {}", file, self.message),
        )
        .with_hint(&self.hint)
    }

    /// Convert without a file (for stdin or already-read buffers).
    #[allow(dead_code)]
    pub(crate) fn into_error_no_file(self) -> GcalError {
        GcalError::setup("client_invalid", self.message).with_hint(&self.hint)
    }
}

const DESKTOP_HINT: &str = "download a Desktop app client JSON from \
    https://console.cloud.google.com/auth/clients/create \
    (OAuth client type Desktop app), then run `bob gcal login -c PATH`";

/// Parse Google-download or legacy-flat client JSON.
pub(crate) fn parse_client_json(
    bytes: &[u8],
    origin: ClientOrigin,
) -> Result<OAuthClient, ClientProblem> {
    let value: serde_json::Value =
        serde_json::from_slice(bytes).map_err(|_| {
            ClientProblem::new(
                "not valid JSON; expected a Google OAuth client download",
                DESKTOP_HINT,
            )
        })?;
    parse_client_value(&value, origin)
}

fn parse_client_value(
    value: &serde_json::Value,
    origin: ClientOrigin,
) -> Result<OAuthClient, ClientProblem> {
    if let Some(obj) = value.as_object()
        && let Some(kind) = obj.get("type").and_then(|v| v.as_str())
    {
        if kind == "service_account" {
            return Err(ClientProblem::new(
                "service account JSON is not an OAuth client; \
                 Bob needs a Desktop app OAuth client",
                DESKTOP_HINT,
            ));
        }
        if kind == "authorized_user" {
            return Err(ClientProblem::new(
                "gcloud credentials (authorized_user) are not an \
                 OAuth client; Bob needs a Desktop app OAuth client",
                DESKTOP_HINT,
            ));
        }
    }

    // `web` clients are never usable for loopback redirects.
    if let Some(web) = value.get("web")
        && web.is_object()
    {
        return Err(ClientProblem::new(
            "Bob needs a Desktop app client; Google only allows \
             any-port loopback redirects for Desktop apps",
            DESKTOP_HINT,
        ));
    }

    // Prefer `installed`, else accept a flat legacy file.
    let (candidate, project_id) = if let Some(installed) =
        value.get("installed")
        && installed.is_object()
    {
        let project = installed
            .get("project_id")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .or_else(|| {
                value
                    .get("project_id")
                    .and_then(|v| v.as_str())
                    .filter(|s| !s.is_empty())
            })
            .map(str::to_string);
        (installed, project)
    } else if value.get("client_id").is_some()
        || value.get("client_secret").is_some()
    {
        let project = value
            .get("project_id")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(str::to_string);
        (value, project)
    } else {
        return Err(ClientProblem::new(
            "missing client_id or client_secret; expected a Google \
             OAuth client download",
            DESKTOP_HINT,
        ));
    };

    let client_id = candidate
        .get("client_id")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            ClientProblem::new(
                "missing client_id; expected a Google OAuth client download",
                DESKTOP_HINT,
            )
        })?;
    let client_secret = candidate
        .get("client_secret")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            ClientProblem::new(
                "missing client_secret; expected a Google OAuth client \
                 download",
                DESKTOP_HINT,
            )
        })?;

    if !client_id.ends_with(".apps.googleusercontent.com") {
        return Err(ClientProblem::new(
            "client_id does not end with \
             .apps.googleusercontent.com; expected a Google OAuth client \
             download",
            DESKTOP_HINT,
        ));
    }

    let project = derive_project(project_id.as_deref(), client_id);
    Ok(OAuthClient {
        client_id: client_id.to_string(),
        client_secret: client_secret.to_string(),
        project,
        origin,
    })
}

/// Project id from a stored client file, even when the secret is absent
/// (keyring-held files hold only `client_id`).
pub(crate) fn peek_project(bytes: &[u8]) -> Option<String> {
    let value: serde_json::Value = serde_json::from_slice(bytes).ok()?;
    let installed = value.get("installed").filter(|v| v.is_object());
    let source = installed.unwrap_or(&value);
    let client_id = source
        .get("client_id")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())?;
    let project_id = source
        .get("project_id")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .or_else(|| {
            value
                .get("project_id")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
        });
    Some(derive_project(project_id, client_id))
}

/// `project_id` when present, else the numeric `client_id` prefix.
pub(crate) fn derive_project(
    project_id: Option<&str>,
    client_id: &str,
) -> String {
    if let Some(project) = project_id.filter(|s| !s.is_empty()) {
        return project.to_string();
    }
    // Typical: `123456789012-abc…apps.googleusercontent.com`.
    let prefix = client_id.split('-').next().unwrap_or(client_id);
    prefix.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> Result<OAuthClient, ClientProblem> {
        parse_client_json(text.as_bytes(), ClientOrigin::File)
    }

    #[test]
    fn installed_client_parses_with_project_id() {
        let client = parse(
            r#"{"installed":{"client_id":"123-abc.apps.googleusercontent.com","client_secret":"s","project_id":"my-proj"}}"#,
        )
        .expect("installed");
        assert_eq!(client.client_id, "123-abc.apps.googleusercontent.com");
        assert_eq!(client.client_secret, "s");
        assert_eq!(client.project, "my-proj");
        assert_eq!(client.origin, ClientOrigin::File);
    }

    #[test]
    fn legacy_flat_client_derives_numeric_prefix() {
        let client = parse(
            r#"{"client_id":"123456789012-abc.apps.googleusercontent.com","client_secret":"s"}"#,
        )
        .expect("flat");
        assert_eq!(client.project, "123456789012");
    }

    #[test]
    fn to_gog_json_wraps_installed() {
        let client = parse(
            r#"{"client_id":"1-a.apps.googleusercontent.com","client_secret":"s"}"#,
        )
        .expect("flat");
        let payload = client.to_gog_json();
        let value: serde_json::Value =
            serde_json::from_slice(&payload).expect("json");
        assert_eq!(
            value["installed"]["client_id"],
            "1-a.apps.googleusercontent.com"
        );
        assert_eq!(value["installed"]["client_secret"], "s");
        assert!(value.get("web").is_none());
    }

    #[test]
    fn web_client_is_rejected() {
        let error = parse(
            r#"{"web":{"client_id":"1-a.apps.googleusercontent.com","client_secret":"s"}}"#,
        )
        .expect_err("web rejected");
        assert!(error.message().contains("Desktop app"));
    }

    #[test]
    fn service_account_is_rejected() {
        let error = parse(r#"{"type":"service_account","project_id":"p"}"#)
            .expect_err("service account rejected");
        assert!(error.message().contains("service account"));
    }

    #[test]
    fn authorized_user_is_rejected() {
        let error = parse(r#"{"type":"authorized_user"}"#)
            .expect_err("authorized_user rejected");
        assert!(error.message().contains("gcloud"));
    }

    #[test]
    fn non_json_is_rejected() {
        let error = parse("not json").expect_err("non-JSON rejected");
        assert!(error.message().contains("not valid JSON"));
    }

    #[test]
    fn missing_secret_is_rejected() {
        let error = parse(r#"{"client_id":"1-a.apps.googleusercontent.com"}"#)
            .expect_err("missing secret");
        assert!(error.message().contains("client_secret"));
    }

    #[test]
    fn missing_id_is_rejected() {
        let error = parse(r#"{"client_secret":"s"}"#).expect_err("missing id");
        assert!(error.message().contains("client_id"));
    }

    #[test]
    fn bad_client_id_suffix_is_rejected() {
        let error =
            parse(r#"{"client_id":"not-a-client","client_secret":"s"}"#)
                .expect_err("bad suffix");
        assert!(error.message().contains(".apps.googleusercontent.com"));
    }

    #[test]
    fn peek_project_from_id_only_file() {
        let project = peek_project(
            br#"{"client_id":"123456789012-abc.apps.googleusercontent.com"}"#,
        )
        .expect("project");
        assert_eq!(project, "123456789012");
    }

    #[test]
    fn empty_secret_is_rejected() {
        let error = parse(
            r#"{"client_id":"1-a.apps.googleusercontent.com","client_secret":""}"#,
        )
        .expect_err("empty secret");
        assert!(error.message().contains("client_secret"));
    }

    #[test]
    fn secret_never_appears_in_messages() {
        let secret = "super-secret-value-123";
        let text =
            format!(r#"{{"client_id":"bad","client_secret":"{secret}"}}"#);
        let error = parse(&text).expect_err("rejected");
        assert!(!error.message().contains(secret));
        assert!(!error.hint().contains(secret));
    }
}
