//! Google Keep → Obsidian inbox drain: protocol model.
//!
//! Rust owns all hashing. `KeepContent` serializes with
//! `serde_json::to_string` in exactly its field order; that string is the
//! canonical form, and `fingerprint` is the first 12 lowercase hex digits
//! of its SHA-256. Attachments are excluded, because OCR text can arrive
//! asynchronously and must not make a note look revised. The archive guard
//! is structural: Rust sends the `content` object back and the adapter
//! compares dicts, so no cross-language hash vector is needed.

use chrono::{DateTime, Local};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Adapter protocol version spoken over stdin/stdout.
pub(crate) const ADAPTER_PROTOCOL_VERSION: u8 = 1;

/// Short selection id: the first 7 hex digits of `sha256(id)`.
pub(crate) fn note_ref(id: &str) -> String {
    hex_prefix(&sha256_hex(id), 7)
}

/// The canonical content of a Keep note: title, text, and list items.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct KeepContent {
    pub(crate) title: String,
    pub(crate) text: String,
    pub(crate) items: Vec<KeepItem>,
}

impl KeepContent {
    /// The canonical JSON form: struct field order, no whitespace.
    pub(crate) fn canonical_json(&self) -> String {
        serde_json::to_string(self).expect("KeepContent serializes")
    }

    /// The first 12 lowercase hex digits of the canonical SHA-256.
    pub(crate) fn fingerprint(&self) -> String {
        hex_prefix(&sha256_hex(&self.canonical_json()), 12)
    }
}

/// One Keep list item in Keep display order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct KeepItem {
    pub(crate) text: String,
    pub(crate) checked: bool,
    pub(crate) indented: bool,
}

/// One Keep note from a `snapshot` response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct KeepNote {
    pub(crate) id: String,
    #[serde(default)]
    pub(crate) server_id: Option<String>,
    pub(crate) kind: KeepNoteKind,
    pub(crate) content: KeepContent,
    #[serde(default)]
    pub(crate) pinned: bool,
    #[serde(default)]
    pub(crate) archived: bool,
    #[serde(default)]
    pub(crate) shared: bool,
    #[serde(default)]
    pub(crate) labels: Vec<String>,
    #[serde(default)]
    pub(crate) attachments: Vec<Attachment>,
    pub(crate) created: String,
    pub(crate) edited: String,
    #[serde(default)]
    pub(crate) url: Option<String>,
}

impl KeepNote {
    /// The Keep `created` timestamp rendered in local time.
    pub(crate) fn created_local(&self) -> Option<DateTime<Local>> {
        parse_keep_time(&self.created)
    }
}

fn parse_keep_time(value: &str) -> Option<DateTime<Local>> {
    value
        .parse::<DateTime<chrono::Utc>>()
        .ok()
        .map(|time| time.into())
}

/// A note holds free text, a checklist, or attachments only.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum KeepNoteKind {
    Note,
    List,
}

/// A Keep attachment; its OCR text never feeds the fingerprint.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Attachment {
    pub(crate) kind: AttachmentKind,
    #[serde(default)]
    pub(crate) extracted_text: Option<String>,
}

/// Attachment media kinds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum AttachmentKind {
    Image,
    Drawing,
    Audio,
    #[serde(other)]
    Other,
}

/// The Keep credentials every adapter request but `ping` carries.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct AdapterAuth {
    pub(crate) email: String,
    pub(crate) master_token: String,
    pub(crate) device_id: String,
    pub(crate) state_path: String,
}

/// A `ping` request: no auth, no network.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct PingRequest {
    pub(crate) protocol: u8,
    pub(crate) op: &'static str,
}

impl PingRequest {
    pub(crate) fn new() -> Self {
        Self {
            protocol: ADAPTER_PROTOCOL_VERSION,
            op: "ping",
        }
    }
}

impl Default for PingRequest {
    fn default() -> Self {
        Self::new()
    }
}

/// A `snapshot` request: list Home notes, plus Archive when asked.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct SnapshotRequest {
    pub(crate) protocol: u8,
    pub(crate) op: &'static str,
    #[serde(flatten)]
    pub(crate) auth: AdapterAuth,
    pub(crate) include_archived: bool,
}

/// One note the adapter should archive, guarded by expected content.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct ArchiveTarget {
    pub(crate) id: String,
    pub(crate) expect: KeepContent,
}

/// An `archive` request: archive notes whose content still matches.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct ArchiveRequest {
    pub(crate) protocol: u8,
    pub(crate) op: &'static str,
    #[serde(flatten)]
    pub(crate) auth: AdapterAuth,
    pub(crate) notes: Vec<ArchiveTarget>,
}

/// An `exchange` request: trade a sign-in cookie for a master token.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct ExchangeRequest {
    pub(crate) protocol: u8,
    pub(crate) op: &'static str,
    pub(crate) email: String,
    pub(crate) oauth_token: String,
    pub(crate) device_id: String,
}

/// A `ping` success response.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub(crate) struct PingResponse {
    pub(crate) ok: bool,
    pub(crate) protocol: u8,
    pub(crate) python: String,
    pub(crate) gkeepapi: String,
    pub(crate) gpsoauth: String,
}

/// A `snapshot` success response.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub(crate) struct SnapshotResponse {
    pub(crate) ok: bool,
    pub(crate) account: String,
    #[serde(default)]
    pub(crate) notes: Vec<KeepNote>,
}

/// One per-note archive outcome.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub(crate) struct ArchiveResult {
    pub(crate) id: String,
    pub(crate) status: ArchiveStatus,
    #[serde(default)]
    pub(crate) detail: Option<String>,
}

/// An `archive` success response.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub(crate) struct ArchiveResponse {
    pub(crate) ok: bool,
    #[serde(default)]
    pub(crate) results: Vec<ArchiveResult>,
}

/// An `exchange` success response.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub(crate) struct ExchangeResponse {
    pub(crate) ok: bool,
    pub(crate) master_token: String,
}

/// Per-note archive outcomes; `archived` and `already_archived` succeed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ArchiveStatus {
    Archived,
    AlreadyArchived,
    Changed,
    Missing,
    Error,
}

impl ArchiveStatus {
    /// Whether the note counts as archived in the vault's favor.
    pub(crate) fn is_success(self) -> bool {
        matches!(self, Self::Archived | Self::AlreadyArchived)
    }

    /// The wire string for JSON and human output.
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Archived => "archived",
            Self::AlreadyArchived => "already_archived",
            Self::Changed => "changed",
            Self::Missing => "missing",
            Self::Error => "error",
        }
    }
}

fn sha256_hex(text: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(text.as_bytes());
    hex::encode(hasher.finalize())
}

fn hex_prefix(hex: &str, len: usize) -> String {
    hex.chars().take(len).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_content() -> KeepContent {
        KeepContent {
            title: "Call dentist".to_string(),
            text: "They close at 5".to_string(),
            items: vec![KeepItem {
                text: "floss".to_string(),
                checked: false,
                indented: false,
            }],
        }
    }

    #[test]
    fn canonical_json_and_fingerprint_are_pinned() {
        let content = sample_content();
        assert_eq!(
            content.canonical_json(),
            r#"{"title":"Call dentist","text":"They close at 5","items":[{"text":"floss","checked":false,"indented":false}]}"#
        );
        assert_eq!(content.fingerprint(), "9f5cc2208fdf");
    }

    #[test]
    fn fingerprint_ignores_json_key_order_and_attachments() {
        let content = sample_content();
        let reordered: KeepContent =
            serde_json::from_str(
                r#"{"items":[{"indented":false,"checked":false,"text":"floss"}],"text":"They close at 5","title":"Call dentist"}"#,
            )
            .expect("reordered content parses");
        assert_eq!(reordered.fingerprint(), content.fingerprint());

        let note: KeepNote = serde_json::from_str(
            r#"{"id":"n1","kind":"note","content":{"title":"Call dentist","text":"They close at 5","items":[{"text":"floss","checked":false,"indented":false}]},"attachments":[{"kind":"image","extracted_text":"RECEIPT TOTAL 12.99"}],"created":"2026-09-27T21:14:03Z","edited":"2026-09-27T21:14:03Z"}"#,
        )
        .expect("note with attachment parses");
        assert_eq!(note.content.fingerprint(), content.fingerprint());
    }

    #[test]
    fn note_ref_is_pinned() {
        assert_eq!(note_ref("note-id-abc.123"), "8bd891d");
    }

    #[test]
    fn requests_serialize_with_protocol_and_op() {
        let auth = AdapterAuth {
            email: "a@b.c".to_string(),
            master_token: "aas_et/x".to_string(),
            device_id: "abc123".to_string(),
            state_path: "/state/auth.json".to_string(),
        };
        assert_eq!(
            serde_json::to_value(PingRequest::new()).expect("json"),
            serde_json::json!({"protocol": 1, "op": "ping"})
        );
        let snapshot = SnapshotRequest {
            protocol: ADAPTER_PROTOCOL_VERSION,
            op: "snapshot",
            auth: auth.clone(),
            include_archived: true,
        };
        let json = serde_json::to_value(&snapshot).expect("json");
        assert_eq!(json["op"], serde_json::json!("snapshot"));
        assert_eq!(json["email"], serde_json::json!("a@b.c"));
        assert_eq!(json["include_archived"], serde_json::json!(true));

        let archive = ArchiveRequest {
            protocol: ADAPTER_PROTOCOL_VERSION,
            op: "archive",
            auth,
            notes: vec![ArchiveTarget {
                id: "n1".to_string(),
                expect: sample_content(),
            }],
        };
        let json = serde_json::to_value(&archive).expect("json");
        assert_eq!(
            json["notes"][0]["expect"]["title"],
            serde_json::json!("Call dentist")
        );

        let exchange = ExchangeRequest {
            protocol: ADAPTER_PROTOCOL_VERSION,
            op: "exchange",
            email: "a@b.c".to_string(),
            oauth_token: "oauth2_4/x".to_string(),
            device_id: "abc123".to_string(),
        };
        let json = serde_json::to_value(&exchange).expect("json");
        assert_eq!(json["op"], serde_json::json!("exchange"));
    }

    #[test]
    fn responses_parse() {
        let ping: PingResponse = serde_json::from_str(
            r#"{"ok":true,"protocol":1,"python":"3.12.3","gkeepapi":"0.17.1","gpsoauth":"2.0.0"}"#,
        )
        .expect("ping parses");
        assert_eq!(ping.gkeepapi, "0.17.1");

        let snapshot: SnapshotResponse = serde_json::from_str(
            r#"{"ok":true,"account":"a@b.c","notes":[{"id":"n1","kind":"list","content":{"title":"","text":"","items":[]},"pinned":true,"archived":false,"shared":false,"labels":["errands"],"attachments":[],"created":"2026-09-27T21:14:03Z","edited":"2026-09-27T21:14:03Z","url":null}]}"#,
        )
        .expect("snapshot parses");
        let note = &snapshot.notes[0];
        assert_eq!(note.kind, KeepNoteKind::List);
        assert!(note.pinned);
        assert_eq!(note.labels, vec!["errands"]);
        assert!(note.created_local().is_some());

        let response: ArchiveResponse = serde_json::from_str(
            r#"{"ok":true,"results":[{"id":"n1","status":"already_archived"},{"id":"n2","status":"changed"}]}"#,
        )
        .expect("archive parses");
        assert!(response.results[0].status.is_success());
        assert!(!response.results[1].status.is_success());

        // `adapter::into_result` reads raw JSON on purpose; failures are
        // asserted through the client error kinds.
        let failure: serde_json::Value = serde_json::from_str(
            r#"{"ok":false,"error":{"kind":"auth","message":"bad token"}}"#,
        )
        .expect("failure parses");
        assert_eq!(failure["error"]["kind"], serde_json::json!("auth"));
    }

    #[test]
    fn archive_success_counts_archived_and_already_archived() {
        assert!(ArchiveStatus::Archived.is_success());
        assert!(ArchiveStatus::AlreadyArchived.is_success());
        assert!(!ArchiveStatus::Changed.is_success());
        assert!(!ArchiveStatus::Missing.is_success());
        assert!(!ArchiveStatus::Error.is_success());
    }

    #[test]
    fn unknown_attachment_kind_deserializes_to_other() {
        let note: KeepNote = serde_json::from_str(
            r#"{"id":"n1","kind":"note","content":{"title":"t","text":"","items":[]},"attachments":[{"kind":"nonetype"}],"created":"2026-09-27T21:14:03Z","edited":"2026-09-27T21:14:03Z"}"#,
        )
        .expect("note with unknown attachment kind parses");
        assert_eq!(note.attachments.len(), 1);
        assert_eq!(note.attachments[0].kind, AttachmentKind::Other);
    }
}
