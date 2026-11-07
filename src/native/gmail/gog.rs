//! Gmail API methods layered on the shared Google gog transport.
//!
//! Thread reads return the typed [`GmailThread`] envelope directly: the
//! transport's original response bytes go through
//! [`GmailThread::from_gog_json`] so every thread consumer shares one
//! explicit adapter. Errors stay body-free and never embed serde input
//! values or raw responses.

use std::{ffi::OsString, path::Path};

use super::GmailError;
pub(crate) use crate::native::google::Gog;

use super::model::{GmailDraft, GmailThread};
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
    /// Gmail's label `type`: `user` or `system`.
    #[serde(default, rename = "type")]
    pub(crate) kind: Option<String>,
}

impl GmailLabel {
    /// Whether this is a label the user made. Without a `type`, Gmail's
    /// `Label_` id prefix (system ids are names such as `INBOX`) decides.
    pub(crate) fn is_user(&self) -> bool {
        match self.kind.as_deref() {
            Some(kind) => kind.eq_ignore_ascii_case("user"),
            None => self.id.starts_with("Label_"),
        }
    }
}

/// Complete compose input for one draft create/update. Bodies and attachment
/// bytes are passed through private files; this type is never debug-printed.
pub(crate) struct DraftCompose<'a> {
    pub(crate) to: &'a [String],
    pub(crate) cc: &'a [String],
    pub(crate) bcc: &'a [String],
    pub(crate) subject: &'a str,
    pub(crate) body_file: &'a Path,
    pub(crate) html_file: &'a Path,
    pub(crate) attachments: &'a [std::path::PathBuf],
    pub(crate) signature: bool,
    pub(crate) reply_to_message_id: Option<&'a str>,
    pub(crate) reply_all: bool,
    pub(crate) clear_reply_context: bool,
}

/// Opaque ID returned by the remote draft create/update call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DraftReceipt {
    pub(crate) id: String,
}

/// Sent message identity returned by `drafts send`.
///
/// gog v0.44.0 `gmail drafts send <draftId>` submits only the recorded draft
/// ID. Google deletes the sent draft and creates a different message with the
/// SENT label, so the returned message ID never equals the draft ID. The
/// decoder accepts both the bare message envelope and the nested
/// `{"message": ...}` shape without echoing response bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SentReceipt {
    pub(crate) message_id: String,
    pub(crate) thread_id: String,
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

    /// Create a Gmail draft. This intentionally omits `--readonly`, while
    /// `--gmail-no-send` and an exact command allowlist keep the transport
    /// limited to draft creation.
    pub(crate) fn draft_create(
        &self,
        account: &str,
        compose: &DraftCompose<'_>,
    ) -> Result<DraftReceipt, GmailError> {
        let mut args = draft_write_flags(account, "gmail.drafts.create");
        args.extend([
            OsString::from("gmail"),
            OsString::from("drafts"),
            OsString::from("create"),
        ]);
        append_draft_compose(&mut args, compose, false);
        let output = self.run(args).map_err(redact_gmail_draft_error)?;
        parse_draft_receipt(&output.stdout)
    }

    /// Update an existing draft by its durable Gmail draft ID. Empty
    /// recipient lists and attachment lists are explicit clears.
    pub(crate) fn draft_update(
        &self,
        account: &str,
        draft_id: &str,
        compose: &DraftCompose<'_>,
    ) -> Result<DraftReceipt, GmailError> {
        let mut args = draft_write_flags(account, "gmail.drafts.update");
        args.extend([
            OsString::from("gmail"),
            OsString::from("drafts"),
            OsString::from("update"),
            OsString::from(draft_id),
        ]);
        append_draft_compose(&mut args, compose, true);
        let output = self.run(args).map_err(redact_gmail_draft_error)?;
        parse_draft_receipt(&output.stdout)
    }

    /// Read a stored draft's complete MIME tree into memory. Never pass
    /// `--download`: gog's attachment cache is outside Bob's write boundary.
    pub(crate) fn draft_get(
        &self,
        account: &str,
        draft_id: &str,
    ) -> Result<GmailDraft, GmailError> {
        let mut args = draft_read_flags(account);
        args.extend([
            OsString::from("gmail"),
            OsString::from("drafts"),
            OsString::from("get"),
            OsString::from(draft_id),
            OsString::from("--format"),
            OsString::from("full"),
        ]);
        let output = self.run(args).map_err(redact_gmail_draft_error)?;
        GmailDraft::from_gog_json(output.stdout.as_bytes())
            .map_err(|_| bad_draft_response("gog returned an invalid draft"))
    }

    /// Submit exactly the recorded draft ID.
    ///
    /// This is the only mutating Gmail send in the compose boundary. It
    /// enables solely `gmail.drafts.send` through an exact-command allowlist
    /// and intentionally omits both `--readonly` and `--gmail-no-send` only
    /// here, after the caller has validated an approved confirmation. There
    /// is no general mutating runner and no direct `messages.send` path.
    /// The draft ID travels as a positional argument, never as message text.
    pub(crate) fn draft_send(
        &self,
        account: &str,
        draft_id: &str,
    ) -> Result<SentReceipt, GmailError> {
        if !valid_draft_id(draft_id) {
            return Err(GmailError::setup(
                "draft_id",
                "the recorded Gmail draft id is invalid".to_string(),
            ));
        }
        let mut args = draft_send_flags(account);
        args.extend([
            OsString::from("gmail"),
            OsString::from("drafts"),
            OsString::from("send"),
            OsString::from(draft_id),
        ]);
        let output = self.run(args).map_err(redact_gmail_send_error)?;
        parse_sent_receipt(&output.stdout)
    }

    /// Fetch a thread for its headers, labels, and message dates.
    ///
    /// gog's `gmail thread get` has no metadata-only mode: it always asks
    /// Gmail for the `full` format, so every message carries all of its
    /// headers (the turn classifier needs `From`, `To`, `Cc`, `Bcc`,
    /// `Auto-Submitted`, `X-Autoreply`, `X-Autorespond`, `Precedence`, and
    /// `Content-Type`), and the JSON also carries body parts. Callers of this
    /// entry point read only identity, labels, dates, headers, and snippets;
    /// it omits `--full`, which changes only gog's human output.
    pub(crate) fn thread_metadata(
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
            ],
        )?;
        GmailThread::from_gog_json(stdout.as_bytes())
            .map_err(|error| GmailError::runtime("bad_json", error.to_string()))
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

fn draft_read_flags(account: &str) -> Vec<OsString> {
    let mut flags = draft_base_flags(account);
    flags.push(OsString::from("--readonly"));
    flags.push(OsString::from("--enable-commands-exact"));
    flags.push(OsString::from("gmail.drafts.get"));
    flags
}

fn draft_write_flags(account: &str, allowed_command: &str) -> Vec<OsString> {
    let mut flags = draft_base_flags(account);
    flags.push(OsString::from("--enable-commands-exact"));
    flags.push(OsString::from(allowed_command));
    flags
}

fn draft_base_flags(account: &str) -> Vec<OsString> {
    vec![
        OsString::from("--no-input"),
        OsString::from("--json"),
        OsString::from("--color"),
        OsString::from("never"),
        OsString::from("--client"),
        OsString::from(CLIENT),
        OsString::from("--account"),
        OsString::from(account),
        OsString::from("--gmail-no-send"),
    ]
}

fn draft_send_flags(account: &str) -> Vec<OsString> {
    vec![
        OsString::from("--no-input"),
        OsString::from("--json"),
        OsString::from("--color"),
        OsString::from("never"),
        OsString::from("--client"),
        OsString::from(CLIENT),
        OsString::from("--account"),
        OsString::from(account),
        OsString::from("--enable-commands-exact"),
        OsString::from("gmail.drafts.send"),
    ]
}

fn append_draft_compose(
    args: &mut Vec<OsString>,
    compose: &DraftCompose<'_>,
    update: bool,
) {
    push_flag(args, "--subject", compose.subject);
    push_flag_path(args, "--body-file", compose.body_file);
    push_flag_path(args, "--body-html-file", compose.html_file);
    append_recipient(args, "--to", compose.to, update);
    append_recipient(args, "--cc", compose.cc, update);
    append_recipient(args, "--bcc", compose.bcc, update);
    if compose.attachments.is_empty() {
        if update {
            args.push(OsString::from("--clear-attachments"));
        }
    } else {
        for attachment in compose.attachments {
            push_flag_path(args, "--attach", attachment);
        }
    }
    if compose.signature {
        args.push(OsString::from("--signature"));
    }
    if update && compose.clear_reply_context {
        args.push(OsString::from("--clear-reply-context"));
    }
    if let Some(message_id) = compose.reply_to_message_id {
        push_flag(args, "--reply-to-message-id", message_id);
        if compose.reply_all {
            args.push(OsString::from("--reply-all"));
        }
    }
}

fn append_recipient(
    args: &mut Vec<OsString>,
    flag: &str,
    recipients: &[String],
    include_empty: bool,
) {
    if recipients.is_empty() {
        if include_empty {
            args.push(OsString::from(format!("{flag}=")));
        }
    } else {
        push_flag(args, flag, &recipients.join(", "));
    }
}

fn push_flag(args: &mut Vec<OsString>, flag: &str, value: &str) {
    args.push(OsString::from(flag));
    args.push(OsString::from(value));
}

fn push_flag_path(args: &mut Vec<OsString>, flag: &str, path: &Path) {
    args.push(OsString::from(flag));
    args.push(path.as_os_str().to_os_string());
}

fn parse_draft_receipt(stdout: &str) -> Result<DraftReceipt, GmailError> {
    let value: serde_json::Value = serde_json::from_str(stdout.trim())
        .map_err(|_| {
            bad_draft_response("gog returned an invalid draft receipt")
        })?;
    let id = value
        .get("draftId")
        .or_else(|| value.get("id"))
        .or_else(|| value.get("draft").and_then(|draft| draft.get("id")))
        .and_then(serde_json::Value::as_str)
        .filter(|id| valid_draft_id(id))
        .ok_or_else(|| bad_draft_response("gog returned no valid draft id"))?;
    Ok(DraftReceipt { id: id.to_string() })
}

fn valid_draft_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 256
        && value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-')
        })
}

fn valid_sent_id(value: &str) -> bool {
    valid_draft_id(value)
}

fn parse_sent_receipt(stdout: &str) -> Result<SentReceipt, GmailError> {
    let value: serde_json::Value = serde_json::from_str(stdout.trim())
        .map_err(|_| {
            bad_draft_response("gog returned an invalid send receipt")
        })?;
    let envelope = value.get("message").unwrap_or(&value);
    let message_id = envelope
        .get("id")
        .or_else(|| envelope.get("messageId"))
        .or_else(|| value.get("messageId"))
        .and_then(serde_json::Value::as_str)
        .filter(|id| valid_sent_id(id))
        .ok_or_else(|| {
            bad_draft_response("gog returned no valid sent message id")
        })?;
    let thread_id = envelope
        .get("threadId")
        .or_else(|| envelope.get("thread_id"))
        .or_else(|| value.get("threadId"))
        .or_else(|| value.get("thread_id"))
        .and_then(serde_json::Value::as_str)
        .filter(|id| valid_sent_id(id))
        .ok_or_else(|| {
            bad_draft_response("gog returned no valid sent thread id")
        })?;
    Ok(SentReceipt {
        message_id: message_id.to_string(),
        thread_id: thread_id.to_string(),
    })
}

fn bad_draft_response(message: &str) -> GmailError {
    GmailError::runtime("bad_json", message.to_string())
}

fn redact_gmail_send_error(error: GmailError) -> GmailError {
    let (message, hint) = match error.kind() {
        "not_found" => (
            "the recorded Gmail draft no longer exists; recovery will determine whether it was sent",
            None,
        ),
        "invalid_grant" => ("the bob-gmail sign-in needs renewal", None),
        "not_logged_in" => ("no bob-gmail Gmail grant is stored", None),
        "insufficient_scope" => (
            "the bob-gmail grant lacks the compose scope",
            Some(
                "run `bob gmail login -s compose` on the approved compose host",
            ),
        ),
        "network" => (
            "gog could not reach Google while sending the draft",
            None,
        ),
        "timeout" => ("gog timed out while sending the draft", None),
        "api_disabled" => {
            ("the Gmail API is not enabled for this Cloud project", None)
        }
        _ => return redact_gmail_setup_error(error, "Gmail send operation"),
    };
    redact_keeping_hint(&error, message, hint)
}

fn redact_gmail_draft_error(error: GmailError) -> GmailError {
    let (message, hint) = match error.kind() {
        "not_found" => (
            "the selected Gmail draft or reply target no longer exists",
            None,
        ),
        "invalid_grant" => ("the bob-gmail sign-in needs renewal", None),
        "not_logged_in" => ("no bob-gmail Gmail grant is stored", None),
        "insufficient_scope" => (
            "the bob-gmail grant lacks the compose scope",
            Some(
                "run `bob gmail login -s compose` on the approved compose host",
            ),
        ),
        "network" => {
            ("gog could not reach Google while handling the draft", None)
        }
        "timeout" => ("gog timed out while handling the draft", None),
        "api_disabled" => {
            ("the Gmail API is not enabled for this Cloud project", None)
        }
        _ => return redact_gmail_setup_error(error, "Gmail draft operation"),
    };
    redact_keeping_hint(&error, message, hint)
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
        _ => return redact_gmail_setup_error(error, "read-only Gmail request"),
    };
    redact_keeping_hint(&error, message, None)
}

/// Failures every Gmail operation shares: a missing or unknown OAuth client
/// and an unreadable keyring mean this host is not signed in, so they keep
/// the classifier's sign-in hint. Anything unclassified stays a fixed,
/// body-free message naming only the operation.
fn redact_gmail_setup_error(error: GmailError, operation: &str) -> GmailError {
    let message = match error.kind() {
        "no_client" => "no bob-gmail OAuth client is stored on this host",
        "invalid_client" => {
            "Google does not know the stored bob-gmail OAuth client"
        }
        "keyring" => {
            "gog could not read the bob-gmail sign-in from the system keyring"
        }
        _ => {
            return GmailError::runtime(
                error.kind(),
                format!("gog could not complete the {operation}"),
            );
        }
    };
    redact_keeping_hint(&error, message, None)
}

/// Replace a classified gog failure's message with a fixed, body-free one.
///
/// The classifier's hint is a fixed string built from the service profile,
/// never from gog's output, so it survives unless the operation supplies a
/// more specific hint. Setup failures keep exit status 2.
fn redact_keeping_hint(
    error: &GmailError,
    message: &str,
    hint: Option<&str>,
) -> GmailError {
    let redacted = if error.exit_code() == 2 {
        GmailError::setup(error.kind(), message.to_string())
    } else {
        GmailError::runtime(error.kind(), message.to_string())
    };
    match hint.or_else(|| error.hint()) {
        Some(hint) => redacted.with_hint(hint),
        None => redacted,
    }
}

/// Strict attachment decoding: standard and base64url alphabets, padded and
/// unpadded. Each engine rejects malformed padding and trailing data, so only
/// a fully valid encoding succeeds. Body-free: returns `None` without
/// echoing input on failure.
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
    fn read_errors_keep_sign_in_hints_and_setup_status() {
        use crate::native::google::{gog::classify_gog_failure, GMAIL};

        for (stderr, kind, exit_code) in [
            (
                "OAuth client credentials missing (OAuth client ID JSON)",
                "no_client",
                2,
            ),
            ("no token stored for PRIVATE_SENTINEL", "not_logged_in", 2),
            (
                "invalid_grant: PRIVATE_SENTINEL expired",
                "invalid_grant",
                1,
            ),
            ("keyring: PRIVATE_SENTINEL locked", "keyring", 1),
        ] {
            let error = redact_gmail_read_error(classify_gog_failure(
                GMAIL, "", stderr,
            ));
            assert_eq!(error.kind(), kind, "{stderr}");
            assert_eq!(error.exit_code(), exit_code, "{stderr}");
            assert!(!error.message().contains("PRIVATE_SENTINEL"));
            assert!(
                error
                    .hint()
                    .is_some_and(|hint| hint.contains("bob gmail login")),
                "{kind} must point at login: {:?}",
                error.hint()
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
    fn send_flags_enable_only_drafts_send_without_read_guards() {
        let argv = draft_send_flags("you@example.com");
        let args: Vec<String> = argv
            .iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        for flag in ["--no-input", "--json", "--client", "--account"] {
            assert!(
                args.iter().any(|arg| arg == flag),
                "missing {flag}: {args:?}"
            );
        }
        assert!(
            !args.iter().any(|arg| arg == "--readonly"),
            "send must omit --readonly: {args:?}"
        );
        assert!(
            !args.iter().any(|arg| arg == "--gmail-no-send"),
            "send must omit --gmail-no-send: {args:?}"
        );
        let exact = args
            .windows(2)
            .find(|pair| pair[0] == "--enable-commands-exact")
            .map(|pair| pair[1].clone())
            .expect("exact allowlist must be present");
        assert_eq!(exact, "gmail.drafts.send");
        assert!(args.iter().any(|arg| arg == "you@example.com"));
    }

    #[test]
    fn send_receipt_accepts_bare_and_nested_shapes_body_free() {
        let bare =
            parse_sent_receipt(r#"{"id":"sent-1","threadId":"thread-1"}"#)
                .expect("bare shape");
        assert_eq!(bare.message_id, "sent-1");
        assert_eq!(bare.thread_id, "thread-1");
        let nested = parse_sent_receipt(
            r#"{"message":{"id":"sent-2","threadId":"thread-2"}}"#,
        )
        .expect("nested shape");
        assert_eq!(nested.message_id, "sent-2");
        assert_eq!(nested.thread_id, "thread-2");
        for bad in [
            r#"{"id":"","threadId":"thread-1"}"#.to_string(),
            r#"{"message":{"id":"sent-1"}}"#.to_string(),
            "SECRET-SEND-BYTES {{{".to_string(),
        ] {
            let error = parse_sent_receipt(&bad).err().expect("fail");
            assert_eq!(error.kind(), "bad_json");
            assert!(!error.message().contains("SECRET"));
        }
        let error = redact_gmail_send_error(GmailError::runtime(
            "timeout",
            "PRIVATE_SENTINEL slow".to_string(),
        ));
        assert_eq!(error.kind(), "timeout");
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
