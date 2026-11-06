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
    "gmail.thread.get,gmail.get,gmail.messages,gmail.search,gmail.url,gmail.attachment";

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
        let output = self.run(args)?;
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

    // Consumed by outer gmail-save attachment hydration once it lands.
    #[allow(dead_code)]
    /// Download one attachment as bytes via `--inline` base64.
    ///
    /// Strict decoding via the shared `base64` crate accepts standard and
    /// base64url alphabets in padded and unpadded forms; malformed padding
    /// or trailing data fails deterministically without echoing content.
    /// Returned bytes are raw attachment bytes for [`FetchedBodies`]: callers
    /// must not base64-decode them again (MIME hydration decodes charset
    /// only).
    pub(crate) fn attachment(
        &self,
        account: &str,
        message: &str,
        attachment: &str,
    ) -> Result<Vec<u8>, GmailError> {
        let stdout = self.gmail_run(
            account,
            vec![
                OsString::from("gmail"),
                OsString::from("attachment"),
                OsString::from(message),
                OsString::from(attachment),
                OsString::from("--inline"),
            ],
        )?;
        let value = parse_json(&stdout)?;
        let encoded = value
            .get("contentBase64")
            .and_then(|v| v.as_str())
            .or_else(|| value.get("data").and_then(|v| v.as_str()))
            .ok_or_else(|| {
                GmailError::runtime(
                    "bad_json",
                    "gog attachment printed no content".to_string(),
                )
            })?;
        let cleaned: String =
            encoded.chars().filter(|ch| !ch.is_whitespace()).collect();
        decode_attachment_base64(&cleaned).ok_or_else(|| {
            GmailError::runtime(
                "bad_json",
                "gog attachment content is not base64".to_string(),
            )
        })
    }
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
