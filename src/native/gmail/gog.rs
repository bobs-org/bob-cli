//! Gmail API methods layered on the shared Google gog transport.
//!
//! Thread reads return the typed [`GmailThread`] envelope directly: the
//! transport's original response bytes go through
//! [`GmailThread::from_gog_json`] so every thread consumer shares one
//! explicit adapter. Errors stay body-free and never embed serde input
//! values or raw responses.

use std::ffi::OsString;

use super::GmailError;
pub(crate) use crate::native::google::Gog;

use super::model::GmailThread;
use crate::native::google::parse_json;

pub(crate) const CLIENT: &str = crate::native::google::GMAIL.client_bucket;
pub(crate) const API_CONSOLE_URL: &str =
    crate::native::google::GMAIL.api_console_url;
pub(crate) use crate::native::google::REVOKE_URL;

/// Read-only Gmail commands Bob may invoke.
const READ_COMMANDS: &str =
    "gmail.thread.get,gmail.get,gmail.messages,gmail.search,gmail.url,gmail.labels.list";

/// Read-only Discovery commands Bob may invoke to fetch attachment bytes.
/// The legacy `gmail.attachment` command is deliberately absent: it writes a
/// cache file, which breaks the no-body-cache and dry-run contracts.
const ATTACHMENT_API_COMMANDS: &str =
    "api.call,api.gmail.users.messages.attachments.get";

/// Explicit Gmail read-only OAuth scope for the Discovery attachment call.
const READONLY_SCOPE: &str = "https://www.googleapis.com/auth/gmail.readonly";

#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
pub(crate) struct GmailLabel {
    pub(crate) id: String,
    pub(crate) name: String,
}

fn read_flags(account: &str) -> Vec<OsString> {
    vec![
        OsString::from("--gmail-no-send"),
        OsString::from("--enable-commands"),
        OsString::from(READ_COMMANDS),
        OsString::from("--account"),
        OsString::from(account),
    ]
}

impl Gog {
    fn gmail_run(
        &self,
        account: &str,
        command: Vec<OsString>,
    ) -> Result<String, GmailError> {
        let mut args = self.common_flags(Some(account));
        args.extend(read_flags(account));
        args.extend(command);
        let output = self.run(args).map_err(redact_gmail_read_error)?;
        Ok(output.stdout)
    }

    // Consumed by gmail-save and search-peek; shell check uses `search` only.
    #[allow(dead_code)]
    /// Fetch one thread with all messages as a typed envelope.
    ///
    /// The transport's original response bytes go straight through the typed
    /// [`GmailThread::from_gog_json`] parser: every thread consumer shares
    /// this adapter instead of stripping the outer `thread` field into an
    /// untyped value. Failures are body-free redaction (`bad_json` with no
    /// input bytes or raw response).
    pub(crate) fn thread_get(
        &self,
        account: &str,
        hex: &str,
    ) -> Result<GmailThread, GmailError> {
        let stdout = self.gmail_run(
            account,
            vec![
                OsString::from("gmail"),
                OsString::from("thread"),
                OsString::from("get"),
                OsString::from(hex),
                OsString::from("--full"),
            ],
        )?;
        GmailThread::from_gog_json(stdout.as_bytes())
            .map_err(|error| GmailError::runtime("bad_json", error.to_string()))
    }

    // Consumed by outer gmail-save and search-peek once they land.
    #[allow(dead_code)]
    /// Fetch one message.
    pub(crate) fn message_get(
        &self,
        account: &str,
        hex: &str,
    ) -> Result<serde_json::Value, GmailError> {
        let stdout = self.gmail_run(
            account,
            vec![
                OsString::from("gmail"),
                OsString::from("get"),
                OsString::from(hex),
                OsString::from("--format"),
                OsString::from("full"),
            ],
        )?;
        let value = parse_json(&stdout)?;
        Ok(value.get("message").cloned().unwrap_or(value))
    }

    /// Fetch message identity and headers without requesting the message body.
    pub(crate) fn message_metadata(
        &self,
        account: &str,
        hex: &str,
    ) -> Result<serde_json::Value, GmailError> {
        let stdout = self.gmail_run(
            account,
            vec![
                OsString::from("gmail"),
                OsString::from("get"),
                OsString::from(hex),
                OsString::from("--format"),
                OsString::from("metadata"),
            ],
        )?;
        let value = parse_json(&stdout)?;
        Ok(value.get("message").cloned().unwrap_or(value))
    }

    /// Search threads with Gmail query syntax.
    pub(crate) fn search(
        &self,
        account: &str,
        query: &str,
        max: u32,
    ) -> Result<serde_json::Value, GmailError> {
        let stdout = self.gmail_run(
            account,
            vec![
                OsString::from("gmail"),
                OsString::from("search"),
                OsString::from(query),
                OsString::from("--max"),
                OsString::from(max.to_string()),
            ],
        )?;
        parse_json(&stdout)
    }

    /// Read one page of matching thread IDs. The `--ids-only` mode avoids
    /// fetching per-thread message data while resolving an RFC message ID.
    pub(crate) fn search_page(
        &self,
        account: &str,
        query: &str,
        max: u32,
        page: Option<&str>,
    ) -> Result<serde_json::Value, GmailError> {
        let mut command = vec![
            OsString::from("gmail"),
            OsString::from("search"),
            OsString::from(query),
            OsString::from("--max"),
            OsString::from(max.to_string()),
            OsString::from("--ids-only"),
        ];
        if let Some(page) = page {
            command.push(OsString::from("--page"));
            command.push(OsString::from(page));
        }
        let stdout = self.gmail_run(account, command)?;
        parse_json(&stdout)
    }

    /// Read Gmail label names and opaque IDs without changing labels.
    pub(crate) fn labels_list(
        &self,
        account: &str,
    ) -> Result<Vec<GmailLabel>, GmailError> {
        let stdout = self.gmail_run(
            account,
            vec![
                OsString::from("gmail"),
                OsString::from("labels"),
                OsString::from("list"),
            ],
        )?;
        let value = parse_json(&stdout)?;
        let labels = value
            .get("labels")
            .cloned()
            .unwrap_or(serde_json::Value::Null);
        serde_json::from_value(labels).map_err(|_| {
            GmailError::runtime(
                "bad_json",
                "gog returned an invalid Gmail label list".to_string(),
            )
        })
    }

    // Consumed by outer mail-note link resolution once it lands.
    #[allow(dead_code)]
    /// Print Gmail web URLs for threads.
    pub(crate) fn url(
        &self,
        account: &str,
        hex: &str,
    ) -> Result<serde_json::Value, GmailError> {
        let stdout = self.gmail_run(
            account,
            vec![
                OsString::from("gmail"),
                OsString::from("url"),
                OsString::from(hex),
            ],
        )?;
        parse_json(&stdout)
    }

    // Consumed by gmail-save attachment hydration (prepare-content).
    #[allow(dead_code)]
    /// Download one attachment's bytes through the narrowly allowed
    /// Discovery read
    /// (`api call gmail v1 gmail.users.messages.attachments.get`).
    ///
    /// `--params` is a serialized JSON argument holding only `userId: me`,
    /// the message ID, and the attachment ID — never shell code. `--no-cache`
    /// and the explicit read-only scope apply, and the allowlist for this
    /// call is exactly `api.call` plus the attachments-get method. The
    /// response `{data,size}` is strictly decoded (standard and base64url
    /// alphabets, padded and unpadded); the declared size is validated
    /// against the actual decoded length and raw bytes are returned once.
    /// Callers must not base64-decode them again (MIME hydration decodes
    /// charset only).
    ///
    /// A gog build that refuses this generic read surfaces a
    /// setup/capability hint naming the minimum gog. There is deliberately no
    /// fallback to the `gmail attachment` command: it writes a cache file,
    /// which breaks the no-body-cache and dry-run contracts.
    pub(crate) fn attachment_bytes(
        &self,
        account: &str,
        message_id: &str,
        attachment_id: &str,
    ) -> Result<Vec<u8>, GmailError> {
        let args = attachment_api_argv(account, message_id, attachment_id);
        let output = self.run(args).map_err(redact_attachment_error)?;
        parse_attachment_response(&output.stdout).map_err(|error| {
            if error.kind() == "bad_json" {
                error
            } else {
                redact_attachment_error(error)
            }
        })
    }
}

/// Build the exact argv for one Discovery attachment read: shared read-only
/// transport flags, the narrow command allowlist, and the serialized
/// `--params` argument. Pure so tests pin the shape without a subprocess.
fn attachment_api_argv(
    account: &str,
    message_id: &str,
    attachment_id: &str,
) -> Vec<OsString> {
    let params = serde_json::json!({
        "userId": "me",
        "messageId": message_id,
        "id": attachment_id,
    })
    .to_string();
    vec![
        OsString::from("--no-input"),
        OsString::from("--json"),
        OsString::from("--color"),
        OsString::from("never"),
        OsString::from("--readonly"),
        OsString::from("--client"),
        OsString::from(CLIENT),
        OsString::from("--account"),
        OsString::from(account),
        OsString::from("--gmail-no-send"),
        OsString::from("--enable-commands"),
        OsString::from(ATTACHMENT_API_COMMANDS),
        OsString::from("--account"),
        OsString::from(account),
        OsString::from("api"),
        OsString::from("call"),
        OsString::from("gmail"),
        OsString::from("v1"),
        OsString::from("gmail.users.messages.attachments.get"),
        OsString::from("--params"),
        OsString::from(params),
        OsString::from("--scope"),
        OsString::from(READONLY_SCOPE),
        OsString::from("--no-cache"),
    ]
}

/// Parse one Discovery attachments-get response. Body-free: failures never
/// echo the response bytes.
fn parse_attachment_response(stdout: &str) -> Result<Vec<u8>, GmailError> {
    let bad =
        |message: &str| GmailError::runtime("bad_json", message.to_string());
    let value: serde_json::Value = serde_json::from_str(stdout.trim())
        .map_err(|_| bad("gog attachment lookup returned invalid JSON"))?;
    let encoded = value
        .get("data")
        .and_then(|value| value.as_str())
        .ok_or_else(|| bad("gog attachment lookup returned no data"))?;
    let cleaned: String =
        encoded.chars().filter(|ch| !ch.is_whitespace()).collect();
    let bytes = decode_attachment_base64(&cleaned).ok_or_else(|| {
        bad("gog attachment lookup returned undecodable data")
    })?;
    if let Some(size) = value.get("size").and_then(|value| value.as_u64())
        && size != bytes.len() as u64
    {
        return Err(bad("gog attachment lookup returned a size mismatch"));
    }
    Ok(bytes)
}

/// Keep attachment-read failures body-free. Classified transport and auth
/// failures keep their kinds so later workers can tell transient failures
/// from permanent ones; an unclassified failure of this fixed-shape
/// read-only call means the gog build cannot perform generic reads, so it
/// becomes a setup/capability error instead of falling back to a
/// cache-writing command.
fn redact_attachment_error(error: GmailError) -> GmailError {
    let redacted = redact_gmail_read_error(error);
    if redacted.kind() == "gog" {
        return GmailError::setup(
            "attachment_unsupported",
            "this gog build refused the read-only Gmail attachment lookup"
                .to_string(),
        )
        .with_hint(
            "install gog ≥ v0.44.0 with `api call` support, or set \
             BOB_GMAIL_GOG to a build that has it",
        );
    }
    redacted
}

fn redact_gmail_read_error(error: GmailError) -> GmailError {
    let message = match error.kind() {
        "not_found" => "Google has no matching Gmail item",
        "invalid_grant" => "the bob-gmail sign-in needs renewal",
        "not_logged_in" => "no bob-gmail Gmail grant is stored",
        "insufficient_scope" => {
            "the bob-gmail grant lacks read-only Gmail access"
        }
        "network" => "gog could not reach Google",
        "timeout" => "gog timed out while reading Gmail",
        "api_disabled" => "the Gmail API is not enabled for this Cloud project",
        _ => "gog could not complete a read-only Gmail request",
    };
    GmailError::runtime(error.kind(), message.to_string())
}

/// Strict attachment decoding: standard and base64url alphabets, padded and
/// unpadded. Each engine rejects malformed padding and trailing data, so only
/// a fully valid encoding succeeds. Body-free: returns `None` without
/// echoing input on failure.
#[allow(dead_code)]
fn decode_attachment_base64(cleaned: &str) -> Option<Vec<u8>> {
    use base64::engine::general_purpose::{
        STANDARD, STANDARD_NO_PAD, URL_SAFE, URL_SAFE_NO_PAD,
    };
    use base64::Engine as _;
    // Padded forms first (strict padding), then unpadded. Both alphabets are
    // accepted to retain the transport's historical standard/base64url forms.
    STANDARD
        .decode(cleaned)
        .or_else(|_| URL_SAFE.decode(cleaned))
        .or_else(|_| STANDARD_NO_PAD.decode(cleaned))
        .or_else(|_| URL_SAFE_NO_PAD.decode(cleaned))
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_standard_and_url_safe_padded_and_unpadded() {
        // "hello-world?" in standard padded, url-safe unpadded, and standard
        // unpadded forms.
        assert_eq!(
            decode_attachment_base64("aGVsbG8td29ybGQ/").as_deref(),
            Some(b"hello-world?".as_slice())
        );
        assert_eq!(
            decode_attachment_base64("aGVsbG8td29ybGQ_").as_deref(),
            Some(b"hello-world?".as_slice())
        );
        assert_eq!(
            decode_attachment_base64("aGVsbG8td29ybGQ/".trim_end_matches('='))
                .as_deref(),
            Some(b"hello-world?".as_slice())
        );
    }

    #[test]
    fn rejects_malformed_padding_and_trailing_data_without_echo() {
        for bad in ["aGVsbG8===", "aGVsbG8td29ybGQ/!!!", "!!!"] {
            let cleaned: String =
                bad.chars().filter(|ch| !ch.is_whitespace()).collect();
            assert!(
                decode_attachment_base64(&cleaned).is_none(),
                "accepted malformed {bad:?}"
            );
        }
    }

    #[test]
    fn read_errors_keep_types_without_echoing_remote_output() {
        let error = redact_gmail_read_error(GmailError::runtime(
            "gog",
            "PRIVATE_SENTINEL raw remote response".to_string(),
        ));
        assert_eq!(error.kind(), "gog");
        assert!(!error.message().contains("PRIVATE_SENTINEL"));

        let error = redact_gmail_read_error(GmailError::runtime(
            "not_found",
            "PRIVATE_SENTINEL raw remote response".to_string(),
        ));
        assert_eq!(error.kind(), "not_found");
        assert!(!error.message().contains("PRIVATE_SENTINEL"));
    }

    #[test]
    fn attachment_argv_is_a_narrow_discovery_read() {
        let argv = attachment_api_argv("me@example.com", "msg-1", "att-9");
        let args: Vec<String> = argv
            .iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        // Read-only transport guards stay on.
        for flag in [
            "--no-input",
            "--json",
            "--readonly",
            "--gmail-no-send",
            "--no-cache",
        ] {
            assert!(
                args.iter().any(|arg| arg == flag),
                "missing {flag}: {args:?}"
            );
        }
        // The allowlist is exactly the api call plus attachments-get.
        let allowlist = args
            .windows(2)
            .find(|pair| pair[0] == "--enable-commands")
            .map(|pair| pair[1].clone())
            .expect("enable-commands must be present");
        assert_eq!(
            allowlist,
            "api.call,api.gmail.users.messages.attachments.get"
        );
        // The cache-writing attachment command is never invoked.
        assert!(
            !args.windows(2).any(|pair| {
                pair == ["gmail".to_string(), "attachment".to_string()]
            }),
            "must not invoke gmail attachment: {args:?}"
        );
        // Discovery method path in order.
        let method = [
            "api",
            "call",
            "gmail",
            "v1",
            "gmail.users.messages.attachments.get",
        ];
        let start = args
            .windows(method.len())
            .position(|window| {
                window.iter().map(String::as_str).eq(method.iter().copied())
            })
            .expect("api call method path must be present");
        let _ = start;
        // --params is one serialized JSON argument with only the three IDs.
        let params_raw = args
            .windows(2)
            .find(|pair| pair[0] == "--params")
            .map(|pair| pair[1].clone())
            .expect("--params must be present");
        let params: serde_json::Value =
            serde_json::from_str(&params_raw).expect("params must be JSON");
        assert_eq!(
            params,
            serde_json::json!({
                "userId": "me",
                "messageId": "msg-1",
                "id": "att-9",
            })
        );
        // Explicit read-only scope, selected account, and client bucket.
        let scope = args
            .windows(2)
            .find(|pair| pair[0] == "--scope")
            .map(|pair| pair[1].clone())
            .expect("--scope must be present");
        assert_eq!(scope, "https://www.googleapis.com/auth/gmail.readonly");
        assert!(args.iter().any(|arg| arg == "me@example.com"));
        assert!(args
            .windows(2)
            .any(|pair| { pair[0] == "--client" && pair[1] == CLIENT }));
    }

    #[test]
    fn attachment_response_parses_data_and_validates_size() {
        use base64::engine::general_purpose::URL_SAFE_NO_PAD;
        use base64::Engine as _;

        // Discovery `{data,size}` decodes once; no double decoding by callers.
        let raw = URL_SAFE_NO_PAD.encode(b"fetched-bytes");
        let stdout = format!(r#"{{"data":"{raw}","size":13}}"#);
        assert_eq!(
            parse_attachment_response(&stdout).as_deref(),
            Ok(b"fetched-bytes".as_slice())
        );
        // Standard padded alphabet and surrounding whitespace are tolerated.
        let stdout = "  {\"data\":\"aGVsbG8td29ybGQ/\",\"size\":12}  \n";
        assert_eq!(
            parse_attachment_response(stdout).as_deref(),
            Ok(b"hello-world?".as_slice())
        );
        // Declared size is validated against the actual decoded length.
        let error = parse_attachment_response(&format!(
            r#"{{"data":"{raw}","size":14}}"#
        ))
        .err()
        .expect("size mismatch must fail");
        assert_eq!(error.kind(), "bad_json");
        // Missing data, undecodable data, and malformed JSON stay body-free.
        for stdout in [
            r#"{"size":3}"#.to_string(),
            r#"{"data":"!!!","size":3}"#.to_string(),
            "SECRET-BYTES {{{".to_string(),
        ] {
            let error = parse_attachment_response(&stdout).err().expect("fail");
            assert_eq!(error.kind(), "bad_json");
            assert!(
                !error.message().contains("SECRET"),
                "echoed input: {}",
                error.message()
            );
            assert!(
                !error.message().contains("!!!"),
                "echoed input: {}",
                error.message()
            );
        }
    }

    #[test]
    fn attachment_refusal_becomes_setup_hint_without_body_echo() {
        let error = redact_attachment_error(GmailError::runtime(
            "gog",
            "PRIVATE_SENTINEL unknown command".to_string(),
        ));
        assert_eq!(error.kind(), "attachment_unsupported");
        assert_eq!(error.exit_code(), 2);
        assert!(!error.message().contains("PRIVATE_SENTINEL"));
        assert!(
            error.hint().is_some_and(|hint| hint.contains("v0.44.0")),
            "hint must name the minimum gog: {:?}",
            error.hint()
        );

        // Classified transport and auth failures keep their kinds.
        let error = redact_attachment_error(GmailError::runtime(
            "timeout",
            "PRIVATE_SENTINEL slow".to_string(),
        ));
        assert_eq!(error.kind(), "timeout");
        assert!(!error.message().contains("PRIVATE_SENTINEL"));
        let error = redact_attachment_error(GmailError::runtime(
            "not_found",
            "PRIVATE_SENTINEL gone".to_string(),
        ));
        assert_eq!(error.kind(), "not_found");
        assert!(!error.message().contains("PRIVATE_SENTINEL"));
    }

    #[test]
    fn typed_thread_parse_hydration_and_redaction_stay_body_free() {
        use crate::native::gmail::body::{render_message, FetchedBodies};
        use base64::engine::general_purpose::URL_SAFE_NO_PAD;
        use base64::Engine as _;

        // Envelope that looks like `gog thread get --full --json`: outer
        // `thread` plus an attachment-backed body needing hydration.
        let plain = URL_SAFE_NO_PAD.encode(b"Hi");
        let envelope = format!(
            r#"{{"thread":{{"id":"t9","messages":[{{"id":"m3","threadId":"t9","labelIds":["INBOX"],"payload":{{"mimeType":"text/plain","body":{{"size":8,"attachmentId":"body-3"}}}}}},{{"id":"m1","threadId":"t9","labelIds":["INBOX"],"payload":{{"mimeType":"text/plain","body":{{"size":2,"data":"{plain}"}}}}}}]}}}}"#
        );
        let thread = GmailThread::from_gog_json(envelope.as_bytes())
            .expect("typed envelope must parse");
        assert_eq!(thread.id, "t9");
        // Missing body stays a fetch need with provisional fallback, then
        // hydrates without double base64-decoding fetched bytes.
        let pending =
            render_message(&thread.messages[0], &FetchedBodies::default())
                .expect("pending must render");
        assert_eq!(pending.needs_fetch.len(), 1);
        let raw = decode_attachment_base64(&URL_SAFE_NO_PAD.encode(b"fetched"))
            .expect("attachment bytes decode");
        let mut fetched = FetchedBodies::default();
        fetched.insert("m3", "body-3", raw);
        let resolved = render_message(&thread.messages[0], &fetched)
            .expect("hydrated must render");
        assert!(resolved.needs_fetch.is_empty());
        assert_eq!(resolved.markdown, "fetched");

        // Malformed responses redact without echoing input bytes.
        let secret = "SECRET-THREAD-BYTES";
        let error = GmailThread::from_gog_json(secret.as_bytes())
            .err()
            .expect("fail");
        assert!(!error.to_string().contains(secret));
        assert!(!format!("{error:?}").contains(secret));
        let mapped = GmailError::runtime("bad_json", error.to_string());
        assert!(!mapped.message().contains(secret));
    }
}
