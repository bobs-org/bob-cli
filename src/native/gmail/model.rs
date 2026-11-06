//! Typed view of gog's ordinary `gmail threads get --json` envelope.

use serde::Deserialize;

use super::body::mime::{validate_tree, MAX_MIME_DEPTH, MAX_MIME_NODES};

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct GmailThread {
    pub(crate) id: String,
    pub(crate) history_id: Option<String>,
    pub(crate) messages: Vec<GmailMessage>,
}

/// One stored Gmail draft and its fully hydrated message envelope as
/// returned by `gog gmail drafts get --format full`.
pub(crate) struct GmailDraft {
    pub(crate) id: String,
    pub(crate) message: GmailMessage,
}

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct GmailMessage {
    pub(crate) id: String,
    pub(crate) thread_id: String,
    pub(crate) label_ids: Vec<String>,
    pub(crate) internal_date: Option<String>,
    pub(crate) snippet: Option<String>,
    pub(crate) headers: Vec<Header>,
    pub(crate) payload: Option<MimePart>,
}

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct Header {
    pub(crate) name: String,
    pub(crate) value: String,
}

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct MimePart {
    pub(crate) part_id: Option<String>,
    pub(crate) mime_type: String,
    pub(crate) filename: Option<String>,
    pub(crate) headers: Vec<Header>,
    pub(crate) body: Option<MimeBody>,
    pub(crate) parts: Vec<MimePart>,
}

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct MimeBody {
    pub(crate) size: Option<u64>,
    pub(crate) data: Option<String>,
    pub(crate) attachment_id: Option<String>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum HeaderKind {
    From,
    To,
    Cc,
    Date,
    Subject,
    MessageId,
    ListId,
    ListUnsubscribe,
    Precedence,
    ContentType,
    ContentDisposition,
    ContentId,
}

impl HeaderKind {
    fn name(self) -> &'static str {
        match self {
            Self::From => "From",
            Self::To => "To",
            Self::Cc => "Cc",
            Self::Date => "Date",
            Self::Subject => "Subject",
            Self::MessageId => "Message-ID",
            Self::ListId => "List-Id",
            Self::ListUnsubscribe => "List-Unsubscribe",
            Self::Precedence => "Precedence",
            Self::ContentType => "Content-Type",
            Self::ContentDisposition => "Content-Disposition",
            Self::ContentId => "Content-ID",
        }
    }
}

impl GmailThread {
    /// Parse gog's `{ "thread": ... }` JSON envelope and validate required identity
    /// fields and bounded recursive MIME structure. Unrelated envelope keys are ignored.
    pub(crate) fn from_gog_json(bytes: &[u8]) -> Result<Self, BodyError> {
        let raw: RawEnvelope = serde_json::from_slice(bytes).map_err(|_| {
            BodyError::new(BodyErrorKind::InvalidEnvelope, Vec::new())
        })?;
        let raw_thread = raw.thread.ok_or_else(|| {
            BodyError::new(BodyErrorKind::MissingThread, Vec::new())
        })?;
        let id = required_nonempty(raw_thread.id).ok_or_else(|| {
            BodyError::new(BodyErrorKind::MissingThreadId, Vec::new())
        })?;
        if let Some(history_id) = raw_thread.history_id.as_deref() {
            validate_decimal(history_id)
                .map_err(|kind| BodyError::new(kind, Vec::new()))?;
        }

        let mut messages = Vec::with_capacity(raw_thread.messages.len());
        for (message_index, message) in
            raw_thread.messages.into_iter().enumerate()
        {
            messages.push(GmailMessage::from_raw(message, &[message_index])?);
        }

        Ok(Self {
            id,
            history_id: raw_thread.history_id,
            messages,
        })
    }
}

impl GmailDraft {
    /// Parse the body-free gog draft envelope without echoing response bytes
    /// through errors or debug formatting.
    pub(crate) fn from_gog_json(bytes: &[u8]) -> Result<Self, BodyError> {
        let raw: RawDraftEnvelope =
            serde_json::from_slice(bytes).map_err(|_| {
                BodyError::new(BodyErrorKind::InvalidEnvelope, Vec::new())
            })?;
        let draft = raw.draft.ok_or_else(|| {
            BodyError::new(BodyErrorKind::MissingThread, Vec::new())
        })?;
        let id = required_nonempty(draft.id).ok_or_else(|| {
            BodyError::new(BodyErrorKind::MissingThreadId, Vec::new())
        })?;
        let message = draft.message.ok_or_else(|| {
            BodyError::new(BodyErrorKind::MissingMessageId, Vec::new())
        })?;
        let message = GmailMessage::from_raw(message, &[])?;
        Ok(Self { id, message })
    }
}

impl GmailMessage {
    fn from_raw(raw: RawMessage, path: &[usize]) -> Result<Self, BodyError> {
        let id = required_nonempty(raw.id).ok_or_else(|| {
            BodyError::new(BodyErrorKind::MissingMessageId, path.to_vec())
        })?;
        let thread_id = required_nonempty(raw.thread_id).ok_or_else(|| {
            BodyError::new(BodyErrorKind::MissingMessageThreadId, path.to_vec())
        })?;
        if let Some(internal_date) = raw.internal_date.as_deref() {
            validate_decimal(internal_date)
                .map_err(|kind| BodyError::new(kind, path.to_vec()))?;
        }
        let payload = raw
            .payload
            .map(|payload| MimePart::from_raw(payload, path))
            .transpose()?;
        if let Some(payload) = &payload {
            validate_tree(payload, MAX_MIME_DEPTH, MAX_MIME_NODES, path)?;
        }
        let headers = payload
            .as_ref()
            .map(|payload| payload.headers.clone())
            .unwrap_or_default();
        Ok(Self {
            id,
            thread_id,
            label_ids: raw.label_ids,
            internal_date: raw.internal_date,
            snippet: raw.snippet,
            headers,
            payload,
        })
    }

    /// Return every matching value in original wire order, matching names without case.
    pub(crate) fn header_values<'a>(
        &'a self,
        name: &'a str,
    ) -> impl Iterator<Item = &'a str> + 'a {
        self.headers
            .iter()
            .filter(move |header| header.name.eq_ignore_ascii_case(name))
            .map(|header| header.value.as_str())
    }

    pub(crate) fn header_values_of(
        &self,
        kind: HeaderKind,
    ) -> impl Iterator<Item = &str> {
        self.header_values(kind.name())
    }

    pub(crate) fn first_header(&self, kind: HeaderKind) -> Option<&str> {
        self.header_values_of(kind).next()
    }
}

impl MimePart {
    fn from_raw(raw: RawMimePart, path: &[usize]) -> Result<Self, BodyError> {
        let mime_type = match raw.mime_type {
            Some(value) if value.trim().is_empty() => {
                return Err(BodyError::new(
                    BodyErrorKind::InvalidMimeStructure,
                    path.to_vec(),
                ));
            }
            Some(value) => value,
            None => "application/octet-stream".to_string(),
        };
        let headers = raw
            .headers
            .into_iter()
            .enumerate()
            .map(|(index, header)| Header::from_raw(header, path, index))
            .collect::<Result<Vec<_>, _>>()?;
        let mut parts = Vec::with_capacity(raw.parts.len());
        for (index, child) in raw.parts.into_iter().enumerate() {
            let mut child_path = path.to_vec();
            child_path.push(index);
            parts.push(Self::from_raw(child, &child_path)?);
        }
        Ok(Self {
            part_id: raw.part_id,
            mime_type,
            filename: raw.filename.filter(|value| !value.is_empty()),
            headers,
            body: raw.body.map(|body| MimeBody {
                size: body.size,
                data: body.data,
                attachment_id: body
                    .attachment_id
                    .filter(|value| !value.is_empty()),
            }),
            parts,
        })
    }
}

impl Header {
    fn from_raw(
        raw: RawHeader,
        path: &[usize],
        header_index: usize,
    ) -> Result<Self, BodyError> {
        let mut location = path.to_vec();
        location.push(header_index);
        let name = raw.name.filter(|name| !name.trim().is_empty()).ok_or_else(
            || {
                BodyError::new(
                    BodyErrorKind::InvalidMimeStructure,
                    location.clone(),
                )
            },
        )?;
        let value = raw.value.ok_or_else(|| {
            BodyError::new(BodyErrorKind::InvalidMimeStructure, location)
        })?;
        Ok(Self { name, value })
    }
}

fn required_nonempty(value: Option<String>) -> Option<String> {
    value.filter(|value| !value.trim().is_empty())
}

fn validate_decimal(value: &str) -> Result<(), BodyErrorKind> {
    if !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit()) {
        Ok(())
    } else {
        Err(BodyErrorKind::InvalidDecimalMetadata)
    }
}

#[derive(Deserialize)]
struct RawEnvelope {
    thread: Option<RawThread>,
}

#[derive(Deserialize)]
struct RawDraftEnvelope {
    draft: Option<RawDraft>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawDraft {
    id: Option<String>,
    message: Option<RawMessage>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawThread {
    id: Option<String>,
    history_id: Option<String>,
    #[serde(default)]
    messages: Vec<RawMessage>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawMessage {
    id: Option<String>,
    thread_id: Option<String>,
    #[serde(default)]
    label_ids: Vec<String>,
    internal_date: Option<String>,
    snippet: Option<String>,
    #[serde(default)]
    payload: Option<RawMimePart>,
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawHeader {
    name: Option<String>,
    value: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawMimePart {
    part_id: Option<String>,
    mime_type: Option<String>,
    filename: Option<String>,
    #[serde(default)]
    headers: Vec<RawHeader>,
    body: Option<RawMimeBody>,
    #[serde(default)]
    parts: Vec<RawMimePart>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawMimeBody {
    size: Option<u64>,
    data: Option<String>,
    attachment_id: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BodyErrorKind {
    InvalidEnvelope,
    MissingThread,
    MissingThreadId,
    MissingMessageId,
    MissingMessageThreadId,
    InvalidMimeStructure,
    InvalidDecimalMetadata,
    InvalidBase64,
    UnsupportedCharset,
    TraversalLimitExceeded,
    /// The caller tried to render a `DRAFT`-labelled message. Drafts are never
    /// imported; the caller must filter them with `importable_messages`.
    /// Carries no sender bytes, only the refusal reason.
    DraftRefused,
}

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct BodyError {
    pub(crate) kind: BodyErrorKind,
    /// Numeric message/part indexes only; sender-controlled IDs and text are omitted.
    pub(crate) location: Vec<usize>,
}

impl BodyError {
    pub(crate) fn new(kind: BodyErrorKind, location: Vec<usize>) -> Self {
        Self { kind, location }
    }
}

impl std::fmt::Debug for BodyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BodyError")
            .field("kind", &self.kind)
            .field("location", &self.location)
            .finish()
    }
}

impl std::fmt::Display for BodyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Gmail body error: {:?}", self.kind)?;
        if !self.location.is_empty() {
            write!(f, " at structural location {:?}", self.location)?;
        }
        Ok(())
    }
}

impl std::error::Error for BodyError {}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(json: &str) -> Result<GmailThread, BodyError> {
        GmailThread::from_gog_json(json.as_bytes())
    }

    #[test]
    fn parses_gog_thread_envelope_and_decimal_strings_without_float_conversion()
    {
        let thread = parse(
            r#"{"thread":{"id":"thread-1","historyId":"184467440737095516151234567890","messages":[{"id":"msg-1","threadId":"thread-1","labelIds":["INBOX"],"internalDate":"184467440737095516159999999999","snippet":"hello","payload":{"mimeType":"text/plain","body":{"size":5,"data":"aGVsbG8"},"headers":[{"name":"Subject","value":"first"},{"name":"subject","value":"second"}]}}]},"downloaded":[]}"#,
        )
        .unwrap();
        assert_eq!(
            thread.history_id.as_deref(),
            Some("184467440737095516151234567890")
        );
        let message = &thread.messages[0];
        assert_eq!(
            message.internal_date.as_deref(),
            Some("184467440737095516159999999999")
        );
        assert_eq!(
            message
                .header_values_of(HeaderKind::Subject)
                .collect::<Vec<_>>(),
            ["first", "second"]
        );
    }

    #[test]
    fn tolerates_empty_threads_and_missing_optional_fields() {
        let thread =
            parse(r#"{"thread":{"id":"t","messages":[]},"unrelated":true}"#)
                .unwrap();
        assert!(thread.messages.is_empty());
        let thread = parse(
            r#"{"thread":{"id":"t","messages":[{"id":"m","threadId":"t"}]}}"#,
        )
        .unwrap();
        assert!(thread.messages[0].payload.is_none());
        assert!(thread.messages[0].snippet.is_none());
    }

    #[test]
    fn rejects_missing_ids_and_non_decimal_metadata_without_echoing_input() {
        let error = parse(r#"{"thread":{"historyId":"SECRET","messages":[]}}"#)
            .err()
            .unwrap();
        assert_eq!(error.kind, BodyErrorKind::MissingThreadId);
        assert!(!error.to_string().contains("SECRET"));
        let error = parse(
            r#"{"thread":{"id":"t","historyId":"BAD_SECRET","messages":[]}}"#,
        )
        .err()
        .unwrap();
        assert_eq!(error.kind, BodyErrorKind::InvalidDecimalMetadata);
        assert!(!format!("{error:?}").contains("BAD_SECRET"));
    }

    #[test]
    fn omitted_and_empty_mime_values_remain_distinct() {
        let thread = parse(r#"{"thread":{"id":"t","messages":[{"id":"m","threadId":"t","payload":{"mimeType":"text/plain","body":{"size":0,"data":""}}}]}}"#).unwrap();
        let payload = thread.messages[0].payload.as_ref().unwrap();
        assert!(payload.body.as_ref().unwrap().data.as_deref() == Some(""));
        let thread = parse(r#"{"thread":{"id":"t","messages":[{"id":"m","threadId":"t","payload":{"mimeType":"text/plain"}}]}}"#).unwrap();
        assert!(thread.messages[0].payload.as_ref().unwrap().body.is_none());
    }

    #[test]
    fn rejects_invalid_mime_structure_with_body_free_error() {
        let error = parse(
            r#"{"thread":{"id":"t","messages":[{"id":"m","threadId":"t","payload":{"mimeType":"","body":{"data":"SECRET"}}}]}}"#,
        )
        .err()
        .unwrap();
        assert_eq!(error.kind, BodyErrorKind::InvalidMimeStructure);
        assert!(!error.to_string().contains("SECRET"));
        let error = parse(
            r#"{"thread":{"id":"t","messages":[{"id":"m","threadId":"t","payload":{"headers":[{"value":"SECRET"}]}}]}}"#,
        )
        .err()
        .unwrap();
        assert_eq!(error.kind, BodyErrorKind::InvalidMimeStructure);
        assert!(!format!("{error:?}").contains("SECRET"));
    }
}
