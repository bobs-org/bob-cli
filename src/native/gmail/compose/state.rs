//! Durable `gmail_send` frontmatter state for the send-recovery engine.
//!
//! The state lives with the note so crash recovery is not lost with one
//! host's cache. It carries metadata only — operation and digest identities,
//! never subject text, recipient addresses, bodies, MIME, or attachment
//! bytes. Journals are equally body-free: only ids, digests, and the outcome.

use std::{
    fs,
    path::Path,
    thread,
    time::{Duration, Instant},
};

use chrono::SecondsFormat;
use sha2::{Digest, Sha256};

use crate::native::{
    capture::StagedTextFile, env as bob_env, pref_journal, vault_sync,
};

use super::{source::SourceError, GmailError};

pub(crate) const SEND_VERSION: u8 = 1;
const SEND_KEY: &str = "gmail_send";

/// Durable stage for one compose/reply send operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SendStage {
    Sending,
    Acknowledged,
    Complete,
}

impl SendStage {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Sending => "sending",
            Self::Acknowledged => "acknowledged",
            Self::Complete => "complete",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        match value {
            "sending" => Some(Self::Sending),
            "acknowledged" => Some(Self::Acknowledged),
            "complete" => Some(Self::Complete),
            _ => None,
        }
    }
}

/// Versioned durable send state stored as one `gmail_send` frontmatter
/// object. Every string field is an opaque identity or a hex digest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SendState {
    pub(crate) version: u8,
    pub(crate) operation_id: String,
    pub(crate) stage: SendStage,
    pub(crate) account: String,
    pub(crate) draft_id: String,
    pub(crate) source_digest: String,
    pub(crate) preview_digest: String,
    pub(crate) draft_message_id: Option<String>,
    pub(crate) draft_rfc_id: Option<String>,
    pub(crate) thread_id: String,
    pub(crate) subject_hash: String,
    pub(crate) recipient_count: u32,
    pub(crate) attempt_time: String,
    pub(crate) sent_message_id: Option<String>,
    pub(crate) sent_thread_id: Option<String>,
    pub(crate) remote_fingerprint: Option<String>,
    pub(crate) completed_note_digest: Option<String>,
    /// Exact vault-relative paths included in the completion commit. This
    /// lets an interrupted commit resume without staging unrelated changes.
    pub(crate) completed_commit_paths: Vec<String>,
}

/// Read the `gmail_send` object when present. Malformed state is a typed
/// refusal, never a silent default.
pub(crate) fn load_send_state(
    contents: &str,
) -> Result<Option<SendState>, SourceError> {
    let value = frontmatter_value(contents)?;
    let Some(mapping) = value.as_mapping() else {
        return Err(SourceError::InvalidFrontmatter);
    };
    let Some(raw) =
        mapping.get(serde_yaml::Value::String(SEND_KEY.to_string()))
    else {
        return Ok(None);
    };
    if raw.is_null() {
        return Ok(None);
    }
    let mapping = raw.as_mapping().ok_or(SourceError::InvalidSendState)?;
    let get = |name: &str| -> Result<&str, SourceError> {
        mapping
            .get(serde_yaml::Value::String(name.to_string()))
            .and_then(serde_yaml::Value::as_str)
            .ok_or(SourceError::InvalidSendState)
    };
    let optional = |name: &str| -> Result<Option<String>, SourceError> {
        match mapping.get(serde_yaml::Value::String(name.to_string())) {
            None | Some(serde_yaml::Value::Null) => Ok(None),
            Some(value) => {
                let text =
                    value.as_str().ok_or(SourceError::InvalidSendState)?;
                if text.is_empty() {
                    return Ok(None);
                }
                Ok(Some(text.to_string()))
            }
        }
    };
    let version = mapping
        .get(serde_yaml::Value::String("version".to_string()))
        .and_then(serde_yaml::Value::as_u64)
        .ok_or(SourceError::InvalidSendState)?;
    if version != u64::from(SEND_VERSION) {
        return Err(SourceError::InvalidSendState);
    }
    let stage =
        SendStage::parse(get("stage")?).ok_or(SourceError::InvalidSendState)?;
    let operation_id = get("operation_id")?.to_string();
    let account = get("account")?.to_string();
    let draft_id = get("draft_id")?.to_string();
    let source_digest = get("source_digest")?.to_string();
    let preview_digest = get("preview_digest")?.to_string();
    let thread_id = get("thread_id")?.to_string();
    let subject_hash = get("subject_hash")?.to_string();
    let recipient_count = mapping
        .get(serde_yaml::Value::String("recipient_count".to_string()))
        .and_then(serde_yaml::Value::as_u64)
        .and_then(|value| u32::try_from(value).ok())
        .ok_or(SourceError::InvalidSendState)?;
    let attempt_time = get("attempt_time")?.to_string();
    let draft_message_id = optional("draft_message_id")?;
    let draft_rfc_id = optional("draft_rfc_id")?;
    let sent_message_id = optional("sent_message_id")?;
    let sent_thread_id = optional("sent_thread_id")?;
    let remote_fingerprint = optional("remote_fingerprint")?;
    let completed_note_digest = optional("completed_note_digest")?;
    let completed_commit_paths = match mapping.get(serde_yaml::Value::String(
        "completed_commit_paths".to_string(),
    )) {
        None => Vec::new(),
        Some(serde_yaml::Value::Sequence(values)) => values
            .iter()
            .map(|value| {
                value
                    .as_str()
                    .map(str::to_string)
                    .ok_or(SourceError::InvalidSendState)
            })
            .collect::<Result<Vec<_>, _>>()?,
        _ => return Err(SourceError::InvalidSendState),
    };

    validate_hex_digest(&operation_id)?;
    validate_opaque_id(&account, true)?;
    validate_opaque_id(&draft_id, false)?;
    validate_hex_digest(&source_digest)?;
    validate_hex_digest(&preview_digest)?;
    validate_opaque_id(&thread_id, false)?;
    validate_hex_digest(&subject_hash)?;
    validate_attempt_time(&attempt_time)?;
    for id in [&draft_message_id, &sent_message_id, &sent_thread_id]
        .into_iter()
        .flatten()
    {
        validate_opaque_id(id, false)?;
    }
    if let Some(rfc) = draft_rfc_id.as_deref() {
        validate_rfc_id(rfc)?;
    }
    if let Some(fingerprint) = remote_fingerprint.as_deref() {
        validate_hex_digest(fingerprint)?;
    }
    if let Some(digest) = completed_note_digest.as_deref() {
        validate_hex_digest(digest)?;
    }
    for path in &completed_commit_paths {
        validate_commit_path(path)?;
    }
    // Acknowledged state must name the proven sent identity; sending state
    // must not claim one yet.
    match stage {
        SendStage::Sending => {
            if sent_message_id.is_some() || sent_thread_id.is_some() {
                return Err(SourceError::InvalidSendState);
            }
        }
        SendStage::Acknowledged => {
            if sent_message_id.is_none() || sent_thread_id.is_none() {
                return Err(SourceError::InvalidSendState);
            }
        }
        SendStage::Complete => {
            if sent_message_id.is_none()
                || sent_thread_id.is_none()
                || completed_note_digest.is_none()
                || mapping
                    .get(serde_yaml::Value::String(
                        "completed_commit_paths".to_string(),
                    ))
                    .is_none()
            {
                return Err(SourceError::InvalidSendState);
            }
        }
    }
    if stage != SendStage::Complete
        && (completed_note_digest.is_some()
            || !completed_commit_paths.is_empty())
    {
        return Err(SourceError::InvalidSendState);
    }
    if !account.contains('@') || account.chars().any(char::is_control) {
        return Err(SourceError::InvalidSendState);
    }

    Ok(Some(SendState {
        version: SEND_VERSION,
        operation_id,
        stage,
        account,
        draft_id,
        source_digest,
        preview_digest,
        draft_message_id,
        draft_rfc_id,
        thread_id,
        subject_hash,
        recipient_count,
        attempt_time,
        sent_message_id,
        sent_thread_id,
        remote_fingerprint,
        completed_note_digest,
        completed_commit_paths,
    }))
}

/// Replace or remove the `gmail_send` block while preserving every other
/// frontmatter line and all authored Markdown bytes.
pub(crate) fn patch_send_state(
    contents: &str,
    state: Option<&SendState>,
) -> Result<String, SourceError> {
    let body_start = frontmatter_body_start(contents)?;
    let closing = line_start(contents, body_start.saturating_sub(1));
    let prefix = &contents[..closing];
    let suffix = &contents[closing..];
    let newline = if contents.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    };

    let mut kept = String::with_capacity(prefix.len() + 256);
    let mut lines = prefix.split_inclusive('\n').peekable();
    let mut removed = false;
    while let Some(line) = lines.next() {
        let bare = line.trim_end_matches(['\r', '\n']);
        if !removed
            && (bare == SEND_KEY
                || bare.starts_with(&format!("{SEND_KEY}:"))
                || bare == format!("{SEND_KEY}:"))
        {
            // Drop the key line plus its indented children.
            removed = true;
            if !bare.trim_end().ends_with(':') {
                continue;
            }
            while let Some(next) = lines.peek() {
                let next_bare = next.trim_end_matches(['\r', '\n']);
                if next_bare.trim().is_empty() {
                    break;
                }
                if next_bare.starts_with(' ') || next_bare.starts_with('\t') {
                    let _ = lines.next();
                } else {
                    break;
                }
            }
            continue;
        }
        // A second top-level gmail_send block is malformed state.
        if bare == SEND_KEY || bare.starts_with(&format!("{SEND_KEY}:")) {
            return Err(SourceError::InvalidSendState);
        }
        kept.push_str(line);
    }

    if let Some(state) = state {
        let block = render_send_block(state, newline);
        // Insert before the closing `---`, after the last kept frontmatter
        // line. `kept` already ends with a newline when the frontmatter is
        // well-formed.
        if !kept.ends_with('\n') {
            kept.push_str(newline);
        }
        kept.push_str(&block);
    }
    kept.push_str(suffix);
    // The patched note must still parse, and a present state must round-trip.
    let reparsed = load_send_state(&kept)?;
    match (state, reparsed) {
        (None, None) => {}
        (Some(expected), Some(actual)) => {
            if &actual != expected {
                return Err(SourceError::InvalidSendState);
            }
        }
        _ => return Err(SourceError::InvalidSendState),
    }
    Ok(kept)
}

fn render_send_block(state: &SendState, newline: &str) -> String {
    let mut out = String::from("gmail_send:");
    out.push_str(newline);
    out.push_str("  version: 1");
    out.push_str(newline);
    push_field(&mut out, "operation_id", &state.operation_id, newline);
    push_field(&mut out, "stage", state.stage.as_str(), newline);
    push_field(&mut out, "account", &state.account, newline);
    push_field(&mut out, "draft_id", &state.draft_id, newline);
    push_field(&mut out, "source_digest", &state.source_digest, newline);
    push_field(&mut out, "preview_digest", &state.preview_digest, newline);
    if let Some(value) = state.draft_message_id.as_deref() {
        push_field(&mut out, "draft_message_id", value, newline);
    }
    if let Some(value) = state.draft_rfc_id.as_deref() {
        push_field(&mut out, "draft_rfc_id", value, newline);
    }
    push_field(&mut out, "thread_id", &state.thread_id, newline);
    push_field(&mut out, "subject_hash", &state.subject_hash, newline);
    out.push_str("  recipient_count: ");
    out.push_str(&state.recipient_count.to_string());
    out.push_str(newline);
    push_field(&mut out, "attempt_time", &state.attempt_time, newline);
    if let Some(value) = state.sent_message_id.as_deref() {
        push_field(&mut out, "sent_message_id", value, newline);
    }
    if let Some(value) = state.sent_thread_id.as_deref() {
        push_field(&mut out, "sent_thread_id", value, newline);
    }
    if let Some(value) = state.remote_fingerprint.as_deref() {
        push_field(&mut out, "remote_fingerprint", value, newline);
    }
    if let Some(value) = state.completed_note_digest.as_deref() {
        push_field(&mut out, "completed_note_digest", value, newline);
    }
    out.push_str("  completed_commit_paths:");
    if state.completed_commit_paths.is_empty() {
        out.push_str(" []");
        out.push_str(newline);
    } else {
        out.push_str(newline);
        for path in &state.completed_commit_paths {
            out.push_str("    - ");
            out.push_str(&yaml_scalar(path));
            out.push_str(newline);
        }
    }
    out
}

fn validate_commit_path(value: &str) -> Result<(), SourceError> {
    let path = std::path::Path::new(value);
    if value.is_empty()
        || path.is_absolute()
        || path.components().any(|component| {
            !matches!(component, std::path::Component::Normal(_))
        })
    {
        return Err(SourceError::InvalidSendState);
    }
    Ok(())
}

fn push_field(out: &mut String, name: &str, value: &str, newline: &str) {
    out.push_str("  ");
    out.push_str(name);
    out.push_str(": ");
    out.push_str(&yaml_scalar(value));
    out.push_str(newline);
}

fn yaml_scalar(value: &str) -> String {
    serde_yaml::to_string(&serde_yaml::Value::String(value.to_string()))
        .map(|encoded| encoded.trim_end().to_string())
        .unwrap_or_else(|_| format!("{value:?}"))
}

/// Persist one send-state transition under the vault maintenance lock with a
/// full-note preimage check. The caller supplies the source fingerprint it
/// based its decision on; a mismatch refuses without writing.
pub(crate) fn persist_send_state(
    vault: &Path,
    relative: &Path,
    expected_source_fingerprint: &str,
    state: Option<SendState>,
) -> Result<(), GmailError> {
    let _maintenance = super::maintenance_lock()?;
    let path = vault.join(relative);
    let original = fs::read_to_string(&path).map_err(|error| {
        GmailError::runtime(
            "note",
            format!("could not re-read compose note: {error}"),
        )
    })?;
    let current = super::source::parse(&original)
        .map_err(|error| GmailError::setup("note", error.to_string()))?;
    if current.fingerprint != expected_source_fingerprint {
        return Err(GmailError::runtime(
            "compose_changed",
            "the compose source changed while Bob was recording send state"
                .to_string(),
        ));
    }
    // A malformed existing send block is a typed refusal, never a silent
    // overwrite.
    if load_send_state(&original).is_err() {
        return Err(GmailError::runtime(
            "send_state_invalid",
            "the compose note has an invalid durable send state".to_string(),
        )
        .with_hint("inspect the gmail_send frontmatter before retrying"));
    }
    let updated = patch_send_state(&original, state.as_ref())
        .map_err(|error| GmailError::runtime("note", error.to_string()))?;
    super::write_and_commit(
        vault,
        vec![StagedTextFile {
            target: path,
            target_existed: true,
            original_target: original,
            updated_target: updated,
        }],
        "gmail: send state",
    )
}

/// Serialize concurrent send/draft work on one note through a host-local
/// lock. The lock file is shared with the draft transport so a draft update
/// and a send can never interleave on the same note.
pub(crate) fn operation_lock(relative: &Path) -> Result<fs::File, GmailError> {
    use fs2::FileExt as _;

    let directory = bob_env::bob_cli_state_dir().join("gmail/compose-locks");
    let id = sha256_hex(path_to_slashes(relative).as_bytes());
    let file =
        bob_env::create_state_lock_file(&directory, &format!("{id}.lock"))
            .map_err(|error| {
                GmailError::runtime(
                    "lock",
                    format!("could not open compose-operation lock: {error}"),
                )
            })?;
    let start = Instant::now();
    loop {
        match file.try_lock_exclusive() {
            Ok(()) => return Ok(file),
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                if start.elapsed() >= Duration::from_secs(30) {
                    return Err(GmailError::runtime(
                        "lock",
                        "another compose operation held the lock for 30 s"
                            .to_string(),
                    ));
                }
                thread::sleep(Duration::from_millis(100));
            }
            Err(error) => {
                return Err(GmailError::runtime(
                    "lock",
                    format!("could not lock compose operation: {error}"),
                ));
            }
        }
    }
}

/// Deterministic send-operation identity bound to the approved preview.
pub(crate) fn send_operation_id(
    note: &str,
    preview_digest: &str,
    account: &str,
) -> String {
    let mut hash = Sha256::new();
    hash.update(b"gmail-send-v1\0");
    for value in [note, preview_digest, account] {
        hash.update((value.len() as u64).to_be_bytes());
        hash.update(value.as_bytes());
    }
    hex::encode(hash.finalize())
}

/// Metadata-only subject commitment included in the durable state and the
/// SENT fallback lookup. Raw subject text never enters the state.
pub(crate) fn subject_hash(subject: &str) -> String {
    sha256_hex(subject.as_bytes())
}

pub(crate) fn attempt_timestamp() -> String {
    pref_journal::event_timestamp().to_rfc3339_opts(SecondsFormat::Secs, false)
}

pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

/// Hash a note while excluding the durable send block. This lets a completed
/// finalization retry its exact-path commit without committing later authored
/// edits to the same note.
pub(crate) fn content_digest(contents: &str) -> Result<String, SourceError> {
    let without_state = patch_send_state(contents, None)?;
    Ok(sha256_hex(without_state.as_bytes()))
}

fn path_to_slashes(path: &Path) -> String {
    path.components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

fn frontmatter_value(contents: &str) -> Result<serde_yaml::Value, SourceError> {
    let mut lines = contents.split_inclusive('\n');
    let first = lines.next().ok_or(SourceError::InvalidFrontmatter)?;
    if first.trim_end_matches(['\r', '\n']).trim() != "---" {
        return Err(SourceError::InvalidFrontmatter);
    }
    let mut yaml = String::new();
    for line in lines {
        let bare = line.trim_end_matches(['\r', '\n']);
        if bare.trim() == "---" {
            let value: serde_yaml::Value = serde_yaml::from_str(&yaml)
                .map_err(|_| SourceError::InvalidFrontmatter)?;
            if !value.is_mapping() {
                return Err(SourceError::InvalidFrontmatter);
            }
            return Ok(value);
        }
        yaml.push_str(line);
    }
    Err(SourceError::InvalidFrontmatter)
}

fn frontmatter_body_start(contents: &str) -> Result<usize, SourceError> {
    let mut lines = contents.split_inclusive('\n');
    let first = lines.next().ok_or(SourceError::InvalidFrontmatter)?;
    if first.trim_end_matches(['\r', '\n']).trim() != "---" {
        return Err(SourceError::InvalidFrontmatter);
    }
    let mut offset = first.len();
    for line in lines {
        let bare = line.trim_end_matches(['\r', '\n']);
        offset += line.len();
        if bare.trim() == "---" {
            return Ok(offset);
        }
    }
    Err(SourceError::InvalidFrontmatter)
}

fn line_start(contents: &str, offset: usize) -> usize {
    contents[..offset.min(contents.len())]
        .rfind('\n')
        .map_or(0, |index| index + 1)
}

fn validate_hex_digest(value: &str) -> Result<(), SourceError> {
    if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(SourceError::InvalidSendState);
    }
    Ok(())
}

fn validate_opaque_id(value: &str, allow_at: bool) -> Result<(), SourceError> {
    if value.is_empty()
        || value.len() > 256
        || value.chars().any(char::is_control)
        || value.chars().any(char::is_whitespace)
    {
        return Err(SourceError::InvalidSendState);
    }
    if !value.bytes().all(|byte| {
        byte.is_ascii_alphanumeric()
            || matches!(byte, b'_' | b'-' | b'.')
            || (allow_at && byte == b'@')
    }) {
        // Accounts contain dots and one at-sign; ids stay alnum plus _-.
        if !(allow_at && value.contains('@')) {
            return Err(SourceError::InvalidSendState);
        }
    }
    Ok(())
}

fn validate_rfc_id(value: &str) -> Result<(), SourceError> {
    if value.len() > 1024
        || value.chars().any(char::is_control)
        || !value.starts_with('<')
        || !value.ends_with('>')
        || !value.contains('@')
    {
        return Err(SourceError::InvalidSendState);
    }
    Ok(())
}

fn validate_attempt_time(value: &str) -> Result<(), SourceError> {
    chrono::DateTime::parse_from_rfc3339(value)
        .map(|_| ())
        .map_err(|_| SourceError::InvalidSendState)
}

const SEND_JOURNAL: pref_journal::Namespace = pref_journal::Namespace {
    dir: ".bob/gmail",
    noun: "gmail",
};

pub(crate) fn append_send_event(
    vault: &Path,
    kind: &str,
    value: serde_json::Value,
) -> Result<(), GmailError> {
    let host = vault_sync::hostname_slug();
    let mut line = serde_json::to_string(&serde_json::json!({
        "v": 1,
        "ts": pref_journal::event_timestamp().to_rfc3339_opts(SecondsFormat::Secs, false),
        "host": host,
        "kind": kind,
        "value": value,
    }))
    .unwrap_or_else(|_| "{}".to_string());
    line.push('\n');
    pref_journal::append_line(vault, SEND_JOURNAL, &host, &line).map_err(
        |error| {
            GmailError::runtime(
                "journal",
                format!(
                    "could not record the body-free Gmail send event: {error}"
                ),
            )
        },
    )
}

/// Ensure the public send journal contains one event for the acknowledged
/// Gmail message. The payload is intentionally limited to the consumer
/// contract fields; the message/thread pair is the replay identity.
pub(crate) fn ensure_sent_event(
    vault: &Path,
    state: &SendState,
) -> Result<(), GmailError> {
    let message_id = state.sent_message_id.as_deref().ok_or_else(|| {
        GmailError::runtime(
            "send_state_invalid",
            "the completed send has no sent message identity".to_string(),
        )
    })?;
    let thread_id = state.sent_thread_id.as_deref().ok_or_else(|| {
        GmailError::runtime(
            "send_state_invalid",
            "the completed send has no sent thread identity".to_string(),
        )
    })?;
    let (events, _) =
        pref_journal::read_raw(vault, SEND_JOURNAL).map_err(|error| {
            GmailError::runtime(
                "journal",
                format!("could not read the Gmail send journal: {error}"),
            )
        })?;
    let already_recorded = events.iter().any(|event| {
        event.kind == "gmail.sent"
            && event
                .value
                .get("value")
                .and_then(|value| value.get("thread_id"))
                .and_then(|value| value.as_str())
                == Some(thread_id)
            && event
                .value
                .get("value")
                .and_then(|value| value.get("message_id"))
                .and_then(|value| value.as_str())
                == Some(message_id)
    });
    if already_recorded {
        return Ok(());
    }
    append_send_event(
        vault,
        "gmail.sent",
        serde_json::json!({
            "thread_id": thread_id,
            "message_id": message_id,
            "recipient_count": state.recipient_count,
            "subject_hash": state.subject_hash,
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn compose_note() -> String {
        "---\nparent: \"[[mail]]\"\ntype: \"[[mail]]\"\ncssclasses: [bob-mail]\nstatus: draft\nto: ada@example.com\ncc: []\nbcc: []\nsubject: Lease renewal\nattachments: []\n---\n\n# Lease renewal\n\nHello.\n"
            .to_string()
    }

    fn sample_state() -> SendState {
        SendState {
            version: SEND_VERSION,
            operation_id: "a".repeat(64),
            stage: SendStage::Sending,
            account: "you@example.com".to_string(),
            draft_id: "draft-1".to_string(),
            source_digest: "b".repeat(64),
            preview_digest: "c".repeat(64),
            draft_message_id: None,
            draft_rfc_id: None,
            thread_id: "thread-1".to_string(),
            subject_hash: "d".repeat(64),
            recipient_count: 2,
            attempt_time: "2026-11-06T12:00:00+00:00".to_string(),
            sent_message_id: None,
            sent_thread_id: None,
            remote_fingerprint: Some("e".repeat(64)),
            completed_note_digest: None,
            completed_commit_paths: Vec::new(),
        }
    }

    #[test]
    fn send_block_round_trips_and_preserves_authored_text() {
        let note = compose_note();
        let state = sample_state();
        let patched =
            patch_send_state(&note, Some(&state)).expect("patch send state");
        assert!(patched.contains("gmail_send:"));
        assert!(patched.contains("Hello."));
        let loaded = load_send_state(&patched).expect("load send state");
        assert_eq!(loaded, Some(state));
        let cleared = patch_send_state(&patched, None).expect("clear state");
        assert!(!cleared.contains("gmail_send:"));
        assert_eq!(load_send_state(&cleared).expect("load cleared"), None);
    }

    #[test]
    fn malformed_send_state_is_a_typed_refusal() {
        let note = compose_note().replace(
            "subject: Lease renewal",
            "subject: Lease renewal\ngmail_send:\n  version: 99",
        );
        assert_eq!(
            load_send_state(&note).err(),
            Some(SourceError::InvalidSendState)
        );
    }

    #[test]
    fn subject_hash_never_contains_subject_text() {
        let hash = subject_hash("Lease renewal");
        assert_eq!(hash.len(), 64);
        assert!(!hash.contains("Lease"));
    }
}
