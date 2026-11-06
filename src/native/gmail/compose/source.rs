//! Pure source model for standalone compose notes and reply sections.
//!
//! Bodies deliberately have no `Debug` implementation. Callers may retain
//! authored message text, so accidental error or log formatting must not
//! disclose it.

use std::{
    ops::Range,
    path::{Component, Path, PathBuf},
};

use pulldown_cmark::{Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use serde_yaml::Value;
use sha2::{Digest, Sha256};

use crate::native::gmail::note;

#[derive(Clone, PartialEq, Eq)]
pub(crate) enum ComposeKind {
    New,
    Reply { all: bool },
}

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct ReplyTarget {
    pub(crate) account: String,
    pub(crate) thread_id: String,
    pub(crate) message_id: String,
    pub(crate) rfc_message_id: Option<String>,
}

pub(crate) struct ComposeSource {
    pub(crate) kind: ComposeKind,
    pub(crate) regions: Vec<Range<usize>>,
    pub(crate) fingerprint: String,
    pub(crate) to: Vec<String>,
    pub(crate) cc: Vec<String>,
    pub(crate) bcc: Vec<String>,
    pub(crate) subject: String,
    pub(crate) attachments: Vec<PathBuf>,
    pub(crate) reply_target: Option<ReplyTarget>,
    pub(crate) draft_id: Option<String>,
    pub(crate) pinned_account: Option<String>,
    pub(crate) remote_fingerprint: Option<String>,
    pub(crate) create_operation: Option<String>,
    body: String,
}

impl ComposeSource {
    pub(crate) fn body(&self) -> &str {
        &self.body
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SourceError {
    InvalidFrontmatter,
    InvalidComposeNote,
    InvalidRecipients,
    InvalidSubject,
    InvalidAttachmentPath,
    MalformedManagedRegion,
    ExistingReply,
    MissingReply,
    ConflictingDraftMarkers,
    InvalidDraftId,
    MissingReplyTarget,
    MissingComposeHeading,
}

impl std::fmt::Display for SourceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::InvalidFrontmatter => {
                "compose note frontmatter is missing or invalid"
            }
            Self::InvalidComposeNote => "note is not a valid Bob compose note",
            Self::InvalidRecipients => "compose recipient fields are malformed",
            Self::InvalidSubject => {
                "compose subject contains an invalid header character"
            }
            Self::InvalidAttachmentPath => {
                "compose attachment path must be a safe vault-relative path"
            }
            Self::MalformedManagedRegion => {
                "saved Gmail note has a malformed managed region"
            }
            Self::ExistingReply => {
                "saved Gmail note already has a Reply section"
            }
            Self::MissingReply => "compose source has no Reply section",
            Self::ConflictingDraftMarkers => {
                "compose note has misplaced or conflicting draft markers"
            }
            Self::InvalidDraftId => {
                "compose draft id contains unsupported characters"
            }
            Self::MissingReplyTarget => {
                "saved Gmail note has no valid reply target"
            }
            Self::MissingComposeHeading => {
                "compose note has no level-one title heading"
            }
        })
    }
}

impl std::error::Error for SourceError {}

/// Parse a standalone compose note or one existing Reply section.
pub(crate) fn parse(contents: &str) -> Result<ComposeSource, SourceError> {
    let frontmatter = frontmatter(contents)?;
    if string_field(&frontmatter.value, "content") == Some("untrusted-email") {
        return parse_reply(contents, frontmatter);
    }
    parse_compose(contents, frontmatter)
}

/// Validate a saved thread and refuse if it already contains any Reply.
pub(crate) fn reply_target_for_append(
    contents: &str,
) -> Result<ReplyTarget, SourceError> {
    let frontmatter = frontmatter(contents)?;
    saved_thread(contents, &frontmatter, false).map(|(target, _)| target)
}

fn parse_compose(
    contents: &str,
    frontmatter: Frontmatter,
) -> Result<ComposeSource, SourceError> {
    if string_field(&frontmatter.value, "type") != Some("[[mail]]")
        || string_field(&frontmatter.value, "status") != Some("draft")
        || string_field(&frontmatter.value, "parent") != Some("[[mail]]")
        || !has_css_class(&frontmatter.value, "bob-mail")
        || string_field(&frontmatter.value, "gmail_thread_id").is_some()
        || string_field(&frontmatter.value, "gmail_account").is_some()
    {
        return Err(SourceError::InvalidComposeNote);
    }
    let subject = string_field(&frontmatter.value, "subject")
        .ok_or(SourceError::InvalidComposeNote)?
        .to_string();
    validate_subject(&subject)?;
    let to = recipient_field(&frontmatter.value, "to")?;
    let cc = recipient_field(&frontmatter.value, "cc")?;
    let bcc = recipient_field(&frontmatter.value, "bcc")?;
    let attachments = attachment_field(&frontmatter.value)?;
    let draft_id = optional_draft_id(&frontmatter.value)?;
    let pinned_account = optional_account(&frontmatter.value)?;
    let remote_fingerprint =
        optional_digest(&frontmatter.value, "gmail_draft_remote_fingerprint")?;
    let create_operation =
        optional_digest(&frontmatter.value, "gmail_draft_create_operation")?;
    validate_draft_metadata(
        draft_id.as_deref(),
        pinned_account.as_deref(),
        remote_fingerprint.as_deref(),
        create_operation.as_deref(),
    )?;
    if !draft_markers(contents, frontmatter.body_start).is_empty() {
        return Err(SourceError::ConflictingDraftMarkers);
    }
    let headings = markdown_headings(contents, frontmatter.body_start);
    let title = headings
        .iter()
        .find(|heading| heading.level == HeadingLevel::H1)
        .ok_or(SourceError::MissingComposeHeading)?;
    let end = headings
        .iter()
        .find(|heading| {
            heading.level == HeadingLevel::H1 && heading.start > title.start
        })
        .map_or(contents.len(), |heading| heading.start);
    let start = skip_blank_lines(contents, title.line_end, end);
    let regions = exclude_notes_sections(&headings, start, end);
    let body = join_regions(contents, &regions);
    let fingerprint = source_fingerprint(
        &ComposeKind::New,
        &to,
        &cc,
        &bcc,
        &subject,
        &attachments,
        &body,
        None,
    );
    Ok(ComposeSource {
        kind: ComposeKind::New,
        regions,
        fingerprint,
        to,
        cc,
        bcc,
        subject,
        attachments,
        reply_target: None,
        draft_id,
        pinned_account,
        remote_fingerprint,
        create_operation,
        body,
    })
}

fn parse_reply(
    contents: &str,
    frontmatter: Frontmatter,
) -> Result<ComposeSource, SourceError> {
    let (target, parsed) = saved_thread(contents, &frontmatter, true)?;
    let headings = markdown_headings(contents, frontmatter.body_start);
    let managed_end = line_end(contents, parsed.end_inner_start);
    let reply_headings: Vec<_> = headings
        .iter()
        .filter(|heading| {
            heading.level == HeadingLevel::H2
                && is_reply_heading(&heading.text)
                && heading.start > managed_end
        })
        .collect();
    if reply_headings.len() != 1 {
        return Err(SourceError::MissingReply);
    }
    let heading = reply_headings[0];
    if !is_exact_reply_heading(heading) {
        return Err(SourceError::MalformedManagedRegion);
    }
    let all = heading.text.trim() == "Reply all";
    let marker_events = draft_markers(contents, frontmatter.body_start);
    if marker_events.len() > 1 {
        return Err(SourceError::ConflictingDraftMarkers);
    }
    if marker_events.iter().any(|marker| marker.id.is_empty()) {
        return Err(SourceError::ConflictingDraftMarkers);
    }
    let draft_id = marker_events.first().map(|marker| marker.id.clone());
    let pinned_account = optional_account(&frontmatter.value)?;
    let remote_fingerprint =
        optional_digest(&frontmatter.value, "gmail_draft_remote_fingerprint")?;
    let create_operation =
        optional_digest(&frontmatter.value, "gmail_draft_create_operation")?;
    validate_draft_metadata(
        draft_id.as_deref(),
        pinned_account.as_deref(),
        remote_fingerprint.as_deref(),
        create_operation.as_deref(),
    )?;
    let mut start = heading.line_end;
    if let Some(marker) = marker_events.first() {
        let marker_line_start = line_start(contents, marker.start);
        if !only_blank_between(contents, heading.line_end, marker_line_start) {
            return Err(SourceError::ConflictingDraftMarkers);
        }
        start = line_end(contents, marker.start);
    }
    let end = headings
        .iter()
        .find(|next| {
            next.start > heading.start
                && matches!(next.level, HeadingLevel::H1 | HeadingLevel::H2)
        })
        .map_or(contents.len(), |next| next.start);
    start = skip_blank_lines(contents, start, end);
    let regions = vec![start..end];
    let body = join_regions(contents, &regions);
    let kind = ComposeKind::Reply { all };
    let fingerprint =
        source_fingerprint(&kind, &[], &[], &[], "", &[], &body, Some(&target));
    Ok(ComposeSource {
        kind,
        regions,
        fingerprint,
        to: Vec::new(),
        cc: Vec::new(),
        bcc: Vec::new(),
        subject: String::new(),
        attachments: Vec::new(),
        reply_target: Some(target),
        draft_id,
        pinned_account,
        remote_fingerprint,
        create_operation,
        body,
    })
}

fn saved_thread(
    contents: &str,
    frontmatter: &Frontmatter,
    require_reply: bool,
) -> Result<(ReplyTarget, note::parse::ParsedNote), SourceError> {
    if frontmatter.value.as_mapping().is_some_and(|mapping| {
        mapping.contains_key(Value::String("gmail_draft_id".to_string()))
    }) {
        return Err(SourceError::ConflictingDraftMarkers);
    }
    let parsed = note::parse::parse(contents)
        .map_err(|_| SourceError::MalformedManagedRegion)?;
    let target = reply_target(contents, &parsed)?;
    let headings = markdown_headings(contents, frontmatter.body_start);
    let managed_end = line_end(contents, parsed.end_inner_start);
    let replies: Vec<_> = headings
        .iter()
        .filter(|heading| {
            heading.level == HeadingLevel::H2 && is_reply_heading(&heading.text)
        })
        .collect();
    if replies.iter().any(|heading| heading.start < managed_end) {
        return Err(SourceError::MalformedManagedRegion);
    }
    if replies.len() > 1 || (!require_reply && !replies.is_empty()) {
        return Err(SourceError::ExistingReply);
    }
    let markers = draft_markers(contents, frontmatter.body_start);
    if markers.len() > 1 {
        return Err(SourceError::ConflictingDraftMarkers);
    }
    if require_reply {
        let Some(reply) = replies.first() else {
            return Err(SourceError::MissingReply);
        };
        if !is_exact_reply_heading(reply) {
            return Err(SourceError::MalformedManagedRegion);
        }
        if let Some(marker) = markers.first()
            && !only_blank_between(
                contents,
                reply.line_end,
                line_start(contents, marker.start),
            )
        {
            return Err(SourceError::ConflictingDraftMarkers);
        }
    } else if !markers.is_empty() {
        return Err(SourceError::ConflictingDraftMarkers);
    }
    Ok((target, parsed))
}

fn reply_target(
    contents: &str,
    parsed: &note::parse::ParsedNote,
) -> Result<ReplyTarget, SourceError> {
    let card = parsed.cards.last().ok_or(SourceError::MissingReplyTarget)?;
    let block = &contents[parsed.begin_inner_start..parsed.end_inner_start];
    let marker = block.lines().find_map(|line| {
        let line = line.trim();
        let body = line.strip_prefix("<!-- bob:gmail:msg ")?;
        let body = body.strip_suffix(" -->")?;
        let mut fields = body.split_ascii_whitespace();
        let message_id = fields.next()?;
        let rfc_id = fields.next()?;
        (fields.next().is_none() && message_id == card.id)
            .then_some((message_id, rfc_id))
    });
    let Some((message_id, rfc_id)) = marker else {
        return Err(SourceError::MissingReplyTarget);
    };
    let rfc_message_id = if rfc_id == "-" {
        None
    } else {
        Some(
            percent_decode_marker(rfc_id)
                .ok_or(SourceError::MissingReplyTarget)?,
        )
    };
    Ok(ReplyTarget {
        account: parsed.account.clone(),
        thread_id: parsed.thread_id.clone(),
        message_id: message_id.to_string(),
        rfc_message_id,
    })
}

#[derive(Clone)]
struct Frontmatter {
    value: Value,
    body_start: usize,
}

fn frontmatter(contents: &str) -> Result<Frontmatter, SourceError> {
    let mut lines = contents.split_inclusive('\n');
    let first = lines.next().ok_or(SourceError::InvalidFrontmatter)?;
    if first.trim_end_matches(['\r', '\n']).trim() != "---" {
        return Err(SourceError::InvalidFrontmatter);
    }
    let mut yaml = String::new();
    let mut offset = first.len();
    for line in lines {
        let line_without_eol = line.trim_end_matches(['\r', '\n']);
        if line_without_eol.trim() == "---" {
            let value: Value = serde_yaml::from_str(&yaml)
                .map_err(|_| SourceError::InvalidFrontmatter)?;
            if !value.is_mapping() {
                return Err(SourceError::InvalidFrontmatter);
            }
            return Ok(Frontmatter {
                value,
                body_start: offset + line.len(),
            });
        }
        yaml.push_str(line);
        offset += line.len();
    }
    Err(SourceError::InvalidFrontmatter)
}

fn string_field<'a>(value: &'a Value, name: &str) -> Option<&'a str> {
    value
        .as_mapping()?
        .get(Value::String(name.to_string()))?
        .as_str()
}

fn has_css_class(value: &Value, expected: &str) -> bool {
    match value.as_mapping().and_then(|mapping| {
        mapping.get(Value::String("cssclasses".to_string()))
    }) {
        Some(Value::Sequence(values)) => {
            values.iter().any(|value| value.as_str() == Some(expected))
        }
        Some(Value::String(value)) => value == expected,
        _ => false,
    }
}

fn recipient_field(
    value: &Value,
    name: &str,
) -> Result<Vec<String>, SourceError> {
    let Some(field) = value
        .as_mapping()
        .and_then(|mapping| mapping.get(Value::String(name.to_string())))
    else {
        return Ok(Vec::new());
    };
    match field {
        Value::Null => Ok(Vec::new()),
        Value::String(value) => parse_recipient_list(value),
        Value::Sequence(values) => {
            let mut recipients = Vec::new();
            for value in values {
                let Some(value) = value.as_str() else {
                    return Err(SourceError::InvalidRecipients);
                };
                recipients.extend(parse_recipient_list(value)?);
            }
            Ok(recipients)
        }
        _ => Err(SourceError::InvalidRecipients),
    }
}

pub(crate) fn parse_recipient_list(
    raw: &str,
) -> Result<Vec<String>, SourceError> {
    validate_header(raw, SourceError::InvalidRecipients)?;
    let pieces = split_addresses(raw)?;
    let mut recipients = Vec::with_capacity(pieces.len());
    for piece in pieces {
        let value = piece.trim();
        if value.is_empty() {
            return Err(SourceError::InvalidRecipients);
        }
        let address = if let Some(open) = value.rfind('<') {
            if !value.ends_with('>') || value[..open].contains(['<', '>']) {
                return Err(SourceError::InvalidRecipients);
            }
            value[open + 1..value.len() - 1].trim()
        } else {
            value
        };
        validate_address(address)?;
        recipients.push(value.to_string());
    }
    Ok(recipients)
}

fn split_addresses(raw: &str) -> Result<Vec<&str>, SourceError> {
    if raw.trim().is_empty() {
        return Ok(Vec::new());
    }
    let mut pieces = Vec::new();
    let mut start = 0;
    let mut quoted = false;
    let mut escaped = false;
    let mut angle_depth = 0usize;
    for (index, character) in raw.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        match character {
            '\\' if quoted => escaped = true,
            '"' => quoted = !quoted,
            '<' if !quoted => {
                angle_depth += 1;
                if angle_depth > 1 {
                    return Err(SourceError::InvalidRecipients);
                }
            }
            '>' if !quoted => {
                angle_depth = angle_depth
                    .checked_sub(1)
                    .ok_or(SourceError::InvalidRecipients)?;
            }
            ',' if !quoted && angle_depth == 0 => {
                pieces.push(&raw[start..index]);
                start = index + character.len_utf8();
            }
            _ => {}
        }
    }
    if quoted || angle_depth != 0 || escaped {
        return Err(SourceError::InvalidRecipients);
    }
    pieces.push(&raw[start..]);
    Ok(pieces)
}

fn validate_address(address: &str) -> Result<(), SourceError> {
    if address.is_empty()
        || address.chars().any(char::is_whitespace)
        || address.chars().any(char::is_control)
    {
        return Err(SourceError::InvalidRecipients);
    }
    let Some((local, domain)) = address.split_once('@') else {
        return Err(SourceError::InvalidRecipients);
    };
    if local.is_empty()
        || domain.is_empty()
        || domain.contains('@')
        || local.starts_with('.')
        || local.ends_with('.')
        || local.contains("..")
        || !local.bytes().all(|byte| {
            byte.is_ascii_alphanumeric()
                || b".!#$%&'*+/=?^_`{|}~-".contains(&byte)
        })
        || !domain.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-')
        })
        || domain.starts_with('.')
        || domain.ends_with('.')
        || domain.split('.').any(|part| {
            part.is_empty() || part.starts_with('-') || part.ends_with('-')
        })
    {
        return Err(SourceError::InvalidRecipients);
    }
    Ok(())
}

fn validate_header(value: &str, error: SourceError) -> Result<(), SourceError> {
    if value.chars().any(char::is_control) {
        Err(error)
    } else {
        Ok(())
    }
}

pub(crate) fn validate_subject(subject: &str) -> Result<(), SourceError> {
    validate_header(subject, SourceError::InvalidSubject)
}

fn attachment_field(value: &Value) -> Result<Vec<PathBuf>, SourceError> {
    let Some(field) = value.as_mapping().and_then(|mapping| {
        mapping.get(Value::String("attachments".to_string()))
    }) else {
        return Ok(Vec::new());
    };
    let Value::Sequence(values) = field else {
        return Err(SourceError::InvalidAttachmentPath);
    };
    values
        .iter()
        .map(|value| {
            let path =
                value.as_str().ok_or(SourceError::InvalidAttachmentPath)?;
            let path = Path::new(path);
            if path.as_os_str().is_empty()
                || path.is_absolute()
                || path
                    .components()
                    .any(|component| !matches!(component, Component::Normal(_)))
                || path.to_string_lossy().chars().any(char::is_control)
            {
                return Err(SourceError::InvalidAttachmentPath);
            }
            Ok(path.to_path_buf())
        })
        .collect()
}

fn optional_draft_id(value: &Value) -> Result<Option<String>, SourceError> {
    let Some(raw) = value.as_mapping().and_then(|mapping| {
        mapping.get(Value::String("gmail_draft_id".to_string()))
    }) else {
        return Ok(None);
    };
    let Some(value) = raw.as_str() else {
        return Err(SourceError::InvalidDraftId);
    };
    validate_draft_id(value)?;
    Ok(Some(value.to_string()))
}

fn optional_account(value: &Value) -> Result<Option<String>, SourceError> {
    let Some(raw) = value.as_mapping().and_then(|mapping| {
        mapping.get(Value::String("gmail_draft_account".to_string()))
    }) else {
        return Ok(None);
    };
    let Some(account) = raw.as_str() else {
        return Err(SourceError::InvalidRecipients);
    };
    let parsed = parse_recipient_list(account)?;
    if parsed.len() != 1 || parsed[0] != account || !account.contains('@') {
        return Err(SourceError::InvalidRecipients);
    }
    Ok(Some(account.to_string()))
}

fn optional_digest(
    value: &Value,
    name: &str,
) -> Result<Option<String>, SourceError> {
    let Some(raw) = value
        .as_mapping()
        .and_then(|mapping| mapping.get(Value::String(name.to_string())))
    else {
        return Ok(None);
    };
    let Some(digest) = raw.as_str() else {
        return Err(SourceError::ConflictingDraftMarkers);
    };
    if digest.len() != 64
        || !digest.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(SourceError::ConflictingDraftMarkers);
    }
    Ok(Some(digest.to_ascii_lowercase()))
}

fn validate_draft_metadata(
    draft_id: Option<&str>,
    account: Option<&str>,
    remote_fingerprint: Option<&str>,
    create_operation: Option<&str>,
) -> Result<(), SourceError> {
    if draft_id.is_some() && account.is_none()
        || remote_fingerprint.is_some() && draft_id.is_none()
        || create_operation.is_some() && account.is_none()
        || create_operation.is_some() && remote_fingerprint.is_some()
    {
        return Err(SourceError::ConflictingDraftMarkers);
    }
    Ok(())
}

/// Patch Bob's reserved draft identity fields while preserving every other
/// frontmatter line and all authored Markdown bytes.
pub(crate) fn persist_draft_metadata(
    contents: &str,
    account: &str,
    draft_id: Option<&str>,
    create_operation: Option<&str>,
    remote_fingerprint: Option<&str>,
) -> Result<String, SourceError> {
    let source = parse(contents)?;
    if let Some(id) = draft_id {
        validate_draft_id(id)?;
    }
    for digest in [create_operation, remote_fingerprint].into_iter().flatten() {
        if digest.len() != 64
            || !digest.bytes().all(|byte| byte.is_ascii_hexdigit())
        {
            return Err(SourceError::ConflictingDraftMarkers);
        }
    }
    let fields = [
        ("gmail_draft_account", Some(account.to_string())),
        (
            "gmail_draft_remote_fingerprint",
            remote_fingerprint.map(str::to_string),
        ),
        (
            "gmail_draft_create_operation",
            create_operation.map(str::to_string),
        ),
    ];
    let mut updated = patch_frontmatter(contents, &fields)?;
    match source.kind {
        ComposeKind::New => {
            updated = patch_frontmatter(
                &updated,
                &[("gmail_draft_id", draft_id.map(str::to_string))],
            )?;
        }
        ComposeKind::Reply { .. } => {
            updated = patch_reply_marker(&updated, draft_id)?;
        }
    }
    Ok(updated)
}

fn patch_frontmatter(
    contents: &str,
    fields: &[(&str, Option<String>)],
) -> Result<String, SourceError> {
    let parsed = frontmatter(contents)?;
    let closing = line_start(contents, parsed.body_start.saturating_sub(1));
    let mut seen = std::collections::BTreeSet::new();
    let mut output = String::with_capacity(contents.len() + 128);
    let prefix = &contents[..closing];
    let mut offset = 0usize;
    for line in prefix.split_inclusive('\n') {
        let bare = line.trim_end_matches(['\r', '\n']);
        let replacement = fields.iter().find_map(|(key, value)| {
            bare.strip_prefix(key)
                .and_then(|rest| rest.starts_with(':').then_some((*key, value)))
        });
        if let Some((key, value)) = replacement {
            if !seen.insert(key) {
                return Err(SourceError::ConflictingDraftMarkers);
            }
            if let Some(value) = value {
                let yaml = serde_yaml::to_string(&Value::String(value.clone()))
                    .map_err(|_| SourceError::InvalidFrontmatter)?;
                let scalar = yaml.trim_end();
                output.push_str(key);
                output.push_str(": ");
                output.push_str(scalar);
                output.push_str(if line.ends_with("\r\n") {
                    "\r\n"
                } else {
                    "\n"
                });
            }
        } else {
            output.push_str(line);
        }
        offset += line.len();
    }
    let _ = offset;
    for (key, value) in fields {
        if seen.contains(key) {
            continue;
        }
        if let Some(value) = value {
            let yaml = serde_yaml::to_string(&Value::String(value.clone()))
                .map_err(|_| SourceError::InvalidFrontmatter)?;
            output.push_str(key);
            output.push_str(": ");
            output.push_str(yaml.trim_end());
            output.push('\n');
        }
    }
    output.push_str(&contents[closing..]);
    Ok(output)
}

fn patch_reply_marker(
    contents: &str,
    draft_id: Option<&str>,
) -> Result<String, SourceError> {
    let parsed = frontmatter(contents)?;
    let headings = markdown_headings(contents, parsed.body_start);
    let heading = headings
        .iter()
        .find(|heading| {
            heading.level == HeadingLevel::H2
                && is_reply_heading(&heading.text)
                && is_exact_reply_heading(heading)
        })
        .ok_or(SourceError::MissingReply)?;
    let markers = draft_markers(contents, parsed.body_start);
    if markers.len() > 1 {
        return Err(SourceError::ConflictingDraftMarkers);
    }
    let Some(id) = draft_id else {
        if let Some(marker) = markers.first() {
            let end = line_end(contents, marker.start);
            let mut updated = contents.to_string();
            updated.replace_range(marker.start..end, "");
            return Ok(updated);
        }
        return Ok(contents.to_string());
    };
    validate_draft_id(id)?;
    let marker = format!("<!-- bob:gmail:draft {id} -->");
    let newline = if contents.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    };
    if let Some(existing) = markers.first() {
        let start = existing.start;
        let end = line_end(contents, start);
        let mut updated = contents.to_string();
        updated.replace_range(start..end, &marker);
        Ok(updated)
    } else {
        let mut updated = contents.to_string();
        updated.insert_str(heading.line_end, &format!("{marker}{newline}"));
        Ok(updated)
    }
}

fn validate_draft_id(value: &str) -> Result<(), SourceError> {
    if value.is_empty()
        || value.len() > 256
        || !value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-')
        })
    {
        return Err(SourceError::InvalidDraftId);
    }
    Ok(())
}

#[derive(Clone)]
struct Heading {
    level: HeadingLevel,
    start: usize,
    line_end: usize,
    raw: String,
    text: String,
}

#[derive(Clone)]
struct DraftMarker {
    start: usize,
    id: String,
}

fn markdown_headings(contents: &str, body_start: usize) -> Vec<Heading> {
    let markdown = &contents[body_start..];
    let parser = Parser::new_ext(markdown, Options::empty()).into_offset_iter();
    let mut headings = Vec::new();
    let mut active: Option<(HeadingLevel, usize, String)> = None;
    for (event, range) in parser {
        match event {
            Event::Start(Tag::Heading { level, .. }) => {
                let offset = body_start + range.start;
                let start = line_start(contents, offset);
                // CommonMark headings inside a callout or block quote are not
                // document sections, even though the parser emits headings.
                active = Some((level, start, String::new()));
            }
            Event::Text(text) | Event::Code(text) => {
                if let Some((_, _, value)) = active.as_mut() {
                    value.push_str(&text);
                }
            }
            Event::SoftBreak | Event::HardBreak => {
                if let Some((_, _, value)) = active.as_mut() {
                    value.push(' ');
                }
            }
            Event::End(TagEnd::Heading(_)) => {
                if let Some((level, start, text)) = active.take() {
                    let offset = start.min(contents.len());
                    let line_end = line_end(contents, offset);
                    let raw = contents
                        [offset..line_content_end(contents, offset)]
                        .trim()
                        .to_string();
                    if !raw.trim_start().starts_with('>') {
                        headings.push(Heading {
                            level,
                            start: offset,
                            line_end,
                            raw,
                            text: text.trim().to_string(),
                        });
                    }
                }
            }
            _ => {}
        }
    }
    headings
}

fn draft_markers(contents: &str, body_start: usize) -> Vec<DraftMarker> {
    let markdown = &contents[body_start..];
    let parser = Parser::new_ext(markdown, Options::empty()).into_offset_iter();
    let mut markers = Vec::new();
    for (event, range) in parser {
        let comment = match event {
            Event::Html(value) | Event::InlineHtml(value) => value,
            _ => continue,
        };
        if !comment.contains("bob:gmail:draft") {
            continue;
        }
        let start = body_start + range.start;
        let line_start = line_start(contents, start);
        let line_end = line_content_end(contents, start);
        let line = contents[line_start..line_end].trim();
        let Some(id) = line
            .strip_prefix("<!-- bob:gmail:draft ")
            .and_then(|line| line.strip_suffix(" -->"))
        else {
            markers.push(DraftMarker {
                start,
                id: String::new(),
            });
            continue;
        };
        if validate_draft_id(id).is_err() {
            markers.push(DraftMarker {
                start,
                id: String::new(),
            });
        } else {
            markers.push(DraftMarker {
                start: line_start,
                id: id.to_string(),
            });
        }
    }
    markers
}

fn is_reply_heading(text: &str) -> bool {
    matches!(text.trim(), "Reply" | "Reply all")
}

fn is_exact_reply_heading(heading: &Heading) -> bool {
    matches!(heading.raw.as_str(), "## Reply" | "## Reply all")
        && matches!(heading.text.as_str(), "Reply" | "Reply all")
}

fn exclude_notes_sections(
    headings: &[Heading],
    start: usize,
    end: usize,
) -> Vec<Range<usize>> {
    let notes: Vec<_> = headings
        .iter()
        .filter(|heading| {
            heading.level == HeadingLevel::H2
                && heading.text == "Notes"
                && heading.start >= start
                && heading.start < end
        })
        .collect();
    if notes.is_empty() {
        return vec![start..end];
    }
    let mut ranges = Vec::new();
    let mut cursor = start;
    for heading in notes {
        if cursor < heading.start {
            ranges.push(cursor..heading.start);
        }
        let next = headings
            .iter()
            .find(|next| {
                next.start > heading.start
                    && matches!(next.level, HeadingLevel::H1 | HeadingLevel::H2)
                    && next.start < end
            })
            .map_or(end, |next| next.start);
        cursor = next;
    }
    if cursor < end {
        ranges.push(cursor..end);
    }
    if ranges.is_empty() {
        ranges.push(start..start);
    }
    ranges
}

fn join_regions(contents: &str, regions: &[Range<usize>]) -> String {
    let mut body = String::new();
    for range in regions {
        let part = contents[range.clone()].trim_matches('\n');
        if part.trim().is_empty() {
            continue;
        }
        if !body.is_empty() {
            body.push_str("\n\n");
        }
        body.push_str(part);
    }
    body
}

fn source_fingerprint(
    kind: &ComposeKind,
    to: &[String],
    cc: &[String],
    bcc: &[String],
    subject: &str,
    attachments: &[PathBuf],
    body: &str,
    target: Option<&ReplyTarget>,
) -> String {
    let mut hash = Sha256::new();
    let mut add = |bytes: &[u8]| {
        hash.update((bytes.len() as u64).to_be_bytes());
        hash.update(bytes);
    };
    add(match kind {
        ComposeKind::New => b"new",
        ComposeKind::Reply { all: false } => b"reply",
        ComposeKind::Reply { all: true } => b"reply-all",
    });
    for list in [to, cc, bcc] {
        add(&(list.len() as u64).to_be_bytes());
        for value in list {
            add(value.as_bytes());
        }
    }
    add(subject.as_bytes());
    add(&(attachments.len() as u64).to_be_bytes());
    for path in attachments {
        add(path.to_string_lossy().as_bytes());
    }
    add(body.as_bytes());
    if let Some(target) = target {
        add(target.account.as_bytes());
        add(target.thread_id.as_bytes());
        add(target.message_id.as_bytes());
        add(target.rfc_message_id.as_deref().unwrap_or("").as_bytes());
    }
    hex::encode(hash.finalize())
}

fn percent_decode_marker(value: &str) -> Option<String> {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut cursor = 0;
    while cursor < bytes.len() {
        if bytes[cursor] == b'%' {
            let high = *bytes.get(cursor + 1)?;
            let low = *bytes.get(cursor + 2)?;
            decoded.push(hex_digit(high)? << 4 | hex_digit(low)?);
            cursor += 3;
        } else {
            decoded.push(bytes[cursor]);
            cursor += 1;
        }
    }
    let decoded = String::from_utf8(decoded).ok()?;
    (!decoded.chars().any(char::is_control)).then_some(decoded)
}

fn hex_digit(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        b'A'..=b'F' => Some(value - b'A' + 10),
        _ => None,
    }
}

fn line_start(contents: &str, offset: usize) -> usize {
    contents[..offset.min(contents.len())]
        .rfind('\n')
        .map_or(0, |index| index + 1)
}

fn line_content_end(contents: &str, offset: usize) -> usize {
    contents[offset.min(contents.len())..]
        .find('\n')
        .map_or(contents.len(), |relative| offset + relative)
}

fn line_end(contents: &str, offset: usize) -> usize {
    contents[offset.min(contents.len())..]
        .find('\n')
        .map_or(contents.len(), |relative| offset + relative + 1)
}

fn only_blank_between(contents: &str, start: usize, end: usize) -> bool {
    start <= end
        && contents
            .get(start..end)
            .is_some_and(|between| between.trim().is_empty())
}

fn skip_blank_lines(contents: &str, mut start: usize, end: usize) -> usize {
    while start < end {
        let content_end = line_content_end(contents, start).min(end);
        if !contents[start..content_end].trim().is_empty() {
            break;
        }
        let next = line_end(contents, start).min(end);
        if next == start {
            break;
        }
        start = next;
    }
    start
}

#[cfg(test)]
mod tests {
    use super::*;

    fn compose_note(extra_frontmatter: &str, body: &str) -> String {
        format!(
            "---\nparent: \"[[mail]]\"\ntype: \"[[mail]]\"\ncssclasses: [bob-mail]\nstatus: draft\nto: '\"Ada, A\" <ada@example.com>, lee@example.com'\ncc:\n  - cc@example.com\nbcc: []\nsubject: Lease renewal\nattachments: []\n{extra_frontmatter}---\n\n# Lease renewal\n\n{body}"
        )
    }

    fn saved_note(tail: &str) -> String {
        format!(
            "---\ntype: \"[[mail]]\"\ncontent: untrusted-email\ngmail_account: you@example.com\ngmail_thread_id: 0123456789abcdef\ntitle: Lease\nwho: Dana\nstarted: 2026-11-05T10:00:00+00:00\nupdated: 2026-11-05T10:00:00+00:00\nmessages: 1\n---\n# Lease\n\n· Dana ↔ you · 1 message · Nov 5\n\n## Notes\n\nPrivate note\n\n## Conversation\n\n<!-- bob:gmail:begin v1 -->\n<!-- bob:gmail:msg 0123456789abcdef %3Cmail%40example.test%3E -->\n> [!mail+] **Dana** · Nov 5\n> Hello\n^m-0123456789abcdef\n<!-- bob:gmail:end -->\n\n{tail}"
        )
    }

    #[test]
    fn compose_source_accepts_scalar_and_sequence_recipients_and_excludes_notes(
    ) {
        let source = parse(&compose_note(
            "",
            "Opening sentence.\n\n## Notes\n\nNever send this.\n\n### Nested note\n\nStill private.\n\n## More message\n\nThis is sent.\n\n```markdown\n## Notes\nexample stays literal\n```\n\n> ## Reply\n> quoted example",
        ))
        .expect("compose source");
        assert_eq!(
            source.to,
            ["\"Ada, A\" <ada@example.com>", "lee@example.com"]
        );
        assert_eq!(source.cc, ["cc@example.com"]);
        assert!(source.bcc.is_empty());
        assert!(source.body().contains("Opening sentence."));
        assert!(source.body().contains("## More message"));
        assert!(source.body().contains("example stays literal"));
        assert!(!source.body().contains("Never send this"));
        assert!(!source.body().contains("Still private"));
        assert!(!source.fingerprint.is_empty());
        assert!(matches!(source.kind, ComposeKind::New));
    }

    #[test]
    fn source_fingerprint_ignores_notes_and_attachment_paths_are_safe() {
        let first =
            parse(&compose_note("", "Body text.\n\n## Notes\n\nPrivate one."))
                .expect("first compose source");
        let second =
            parse(&compose_note("", "Body text.\n\n## Notes\n\nPrivate two."))
                .expect("second compose source");
        assert_eq!(first.fingerprint, second.fingerprint);

        let unsafe_attachment = compose_note("", "Body text.")
            .replace("attachments: []", "attachments: [../outside.txt]");
        assert_eq!(
            parse(&unsafe_attachment).err(),
            Some(SourceError::InvalidAttachmentPath)
        );
    }

    #[test]
    fn reply_source_uses_exact_target_and_skips_code_and_callout_headings() {
        let contents = saved_note(
            "```markdown\n## Reply\n```\n\n> ## Reply all\n> quoted example\n\n## Reply all\n\n<!-- bob:gmail:draft draft_123 -->\n\nLet's proceed.",
        )
        .replace(
            "gmail_account: you@example.com\n",
            "gmail_account: you@example.com\ngmail_draft_account: you@example.com\n",
        );
        let source = parse(&contents).expect("reply source");
        assert!(matches!(source.kind, ComposeKind::Reply { all: true }));
        assert_eq!(source.draft_id.as_deref(), Some("draft_123"));
        let target = source.reply_target.as_ref().expect("reply target");
        assert_eq!(target.account, "you@example.com");
        assert_eq!(target.thread_id, "0123456789abcdef");
        assert_eq!(target.message_id, "0123456789abcdef");
        assert_eq!(
            target.rfc_message_id.as_deref(),
            Some("<mail@example.test>")
        );
        assert_eq!(source.body(), "Let's proceed.");
        assert!(!source.body().contains("draft_123"));
    }

    #[test]
    fn reply_creation_refuses_any_existing_reply_and_preserves_source_bytes() {
        let contents = saved_note("## Reply\n\nExisting text.\n");
        assert!(matches!(
            reply_target_for_append(&contents),
            Err(SourceError::ExistingReply)
        ));
        assert!(contents.ends_with("## Reply\n\nExisting text.\n"));
    }

    #[test]
    fn header_validation_rejects_crlf_and_malformed_recipient_values() {
        assert_eq!(
            parse_recipient_list("ada@example.com\nBcc: attacker@example.com"),
            Err(SourceError::InvalidRecipients)
        );
        assert_eq!(
            parse_recipient_list("not-an-address"),
            Err(SourceError::InvalidRecipients)
        );
        let malformed = compose_note("", "body")
            .replace("subject: Lease renewal", "subject: \"Line\\nbreak\"");
        assert_eq!(parse(&malformed).err(), Some(SourceError::InvalidSubject));
    }

    #[test]
    fn malformed_and_conflicting_draft_markers_are_typed_refusals() {
        let malformed = saved_note(
            "## Reply\n\n<!-- bob:gmail:draft bad/marker -->\n\nbody",
        );
        assert_eq!(
            parse(&malformed).err(),
            Some(SourceError::ConflictingDraftMarkers)
        );
        let duplicate = saved_note(
            "## Reply\n<!-- bob:gmail:draft one -->\n\n<!-- bob:gmail:draft two -->\nbody",
        );
        assert_eq!(
            parse(&duplicate).err(),
            Some(SourceError::ConflictingDraftMarkers)
        );
        let malformed_region = saved_note("## Reply\n\nbody")
            .replace("<!-- bob:gmail:end -->\n", "");
        assert!(matches!(
            reply_target_for_append(&malformed_region),
            Err(SourceError::MalformedManagedRegion)
        ));
    }
}
