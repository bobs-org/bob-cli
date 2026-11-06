//! Deterministic Gmail MIME walking and private payload decoding.

use base64::engine::general_purpose::{URL_SAFE, URL_SAFE_NO_PAD};
use base64::Engine as _;
use encoding_rs::Encoding;

use super::{
    Attachment, BodyCandidate, CandidateKind, FetchNeed, FetchedBodies,
    InlineImage, InlineImageSource,
};
use crate::native::gmail::model::{
    BodyError, BodyErrorKind, GmailMessage, MimePart,
};

pub(crate) const MAX_MIME_DEPTH: usize = 64;
pub(crate) const MAX_MIME_NODES: usize = 4096;

#[derive(Clone, Default, PartialEq, Eq)]
pub(crate) struct MimeInventory {
    pub(crate) html_candidates: Vec<BodyCandidate>,
    pub(crate) plain_candidates: Vec<BodyCandidate>,
    pub(crate) inline_images: Vec<InlineImage>,
    pub(crate) attachments: Vec<Attachment>,
    pub(crate) needs_fetch: Vec<FetchNeed>,
}

pub(crate) fn collect_message_bodies(
    message: &GmailMessage,
    fetched: &FetchedBodies,
) -> Result<MimeInventory, BodyError> {
    let Some(root) = message.payload.as_ref() else {
        return Ok(MimeInventory::default());
    };
    validate_tree(root, MAX_MIME_DEPTH, MAX_MIME_NODES, &[])?;

    let mut inventory = MimeInventory::default();
    collect_resources(
        root,
        &message.id,
        fetched,
        &mut inventory,
        &mut Vec::new(),
    )?;
    collect_candidates(
        root,
        &message.id,
        fetched,
        &mut inventory,
        &mut Vec::new(),
        true,
    )?;
    Ok(inventory)
}

pub(crate) fn validate_tree(
    root: &MimePart,
    max_depth: usize,
    max_nodes: usize,
    message_path: &[usize],
) -> Result<(), BodyError> {
    let mut stack = vec![(root, 0usize, Vec::<usize>::new())];
    let mut count = 0usize;
    while let Some((part, depth, path)) = stack.pop() {
        count = count.saturating_add(1);
        if depth > max_depth || count > max_nodes {
            let mut location = message_path.to_vec();
            location.extend(path);
            return Err(BodyError::new(
                BodyErrorKind::TraversalLimitExceeded,
                location,
            ));
        }
        if count
            .saturating_add(stack.len())
            .saturating_add(part.parts.len())
            > max_nodes
        {
            let mut location = message_path.to_vec();
            location.extend(path);
            return Err(BodyError::new(
                BodyErrorKind::TraversalLimitExceeded,
                location,
            ));
        }
        for (index, child) in part.parts.iter().enumerate().rev() {
            let mut child_path = path.clone();
            child_path.push(index);
            stack.push((child, depth + 1, child_path));
        }
    }
    Ok(())
}

fn collect_resources(
    part: &MimePart,
    message_id: &str,
    fetched: &FetchedBodies,
    inventory: &mut MimeInventory,
    path: &mut Vec<usize>,
) -> Result<(), BodyError> {
    let mime = base_mime_type(&part.mime_type);
    if is_attachment(part) {
        inventory.attachments.push(Attachment {
            // Numbered at assembly time in render.rs; zero here keeps the
            // inventory a faithful pre-selection record.
            index: 0,
            filename: part.filename.clone(),
            mime_type: part.mime_type.clone(),
            size: part.body.as_ref().and_then(|body| body.size),
            attachment_id: part
                .body
                .as_ref()
                .and_then(|body| body.attachment_id.clone()),
        });
    } else if mime.starts_with("image/") && content_id(part).is_some() {
        let body = part.body.as_ref();
        let attachment_id = body.and_then(|body| body.attachment_id.as_deref());
        let source =
            if let Some(bytes) = body.and_then(|body| body.data.as_deref()) {
                InlineImageSource::Bytes(decode_base64(bytes, path)?)
            } else if let Some(attachment_id) = attachment_id {
                if let Some(bytes) = fetched.get(message_id, attachment_id) {
                    InlineImageSource::Bytes(bytes.to_vec())
                } else {
                    InlineImageSource::AttachmentId(attachment_id.to_string())
                }
            } else {
                // An unresolved CID without a provider ID cannot be fetched. Preserve its
                // typed source as an empty byte payload so the renderer can report a stub.
                InlineImageSource::Bytes(Vec::new())
            };
        inventory.inline_images.push(InlineImage {
            content_id: content_id(part).map(str::to_string),
            mime_type: part.mime_type.clone(),
            size: body.and_then(|body| body.size),
            filename: part.filename.clone(),
            source,
        });
    }

    for (index, child) in part.parts.iter().enumerate() {
        path.push(index);
        collect_resources(child, message_id, fetched, inventory, path)?;
        path.pop();
    }
    Ok(())
}

fn collect_candidates(
    part: &MimePart,
    message_id: &str,
    fetched: &FetchedBodies,
    inventory: &mut MimeInventory,
    path: &mut Vec<usize>,
    is_body_root: bool,
) -> Result<(), BodyError> {
    let mime = base_mime_type(&part.mime_type);
    if !part.parts.is_empty() {
        let selected_related_root = if mime == "multipart/related" {
            related_root_index(part).unwrap_or(0)
        } else {
            usize::MAX
        };
        for (index, child) in part.parts.iter().enumerate() {
            let selected =
                mime != "multipart/related" || index == selected_related_root;
            if !selected {
                continue;
            }
            path.push(index);
            collect_candidates(
                child,
                message_id,
                fetched,
                inventory,
                path,
                is_body_root,
            )?;
            path.pop();
        }
        return Ok(());
    }

    if !is_body_root || is_attachment(part) {
        return Ok(());
    }
    let kind = match mime.as_str() {
        "text/plain" => CandidateKind::Plain,
        "text/html" => CandidateKind::Html,
        _ => return Ok(()),
    };
    let decoded = decode_part_body(part, message_id, fetched, path)?;
    match decoded {
        DecodedPart::Text(text) => {
            let candidate = BodyCandidate {
                kind,
                part_id: part.part_id.clone(),
                mime_type: part.mime_type.clone(),
                text,
            };
            match kind {
                CandidateKind::Html => {
                    inventory.html_candidates.push(candidate)
                }
                CandidateKind::Plain => {
                    inventory.plain_candidates.push(candidate)
                }
            }
        }
        DecodedPart::NeedsFetch(attachment_id) => {
            inventory.needs_fetch.push(FetchNeed {
                part_id: part.part_id.clone(),
                attachment_id,
            });
        }
        DecodedPart::Omitted => {}
    }
    Ok(())
}

enum DecodedPart {
    Text(String),
    NeedsFetch(Option<String>),
    Omitted,
}

fn decode_part_body(
    part: &MimePart,
    message_id: &str,
    fetched: &FetchedBodies,
    path: &[usize],
) -> Result<DecodedPart, BodyError> {
    let Some(body) = part.body.as_ref() else {
        return Ok(DecodedPart::Omitted);
    };
    if let Some(data) = body.data.as_deref() {
        let bytes = decode_base64(data, path)?;
        return decode_text(bytes, part, path).map(DecodedPart::Text);
    }
    if let Some(attachment_id) = body.attachment_id.as_deref() {
        if let Some(bytes) = fetched.get(message_id, attachment_id) {
            return decode_text(bytes.to_vec(), part, path)
                .map(DecodedPart::Text);
        }
        return Ok(DecodedPart::NeedsFetch(Some(attachment_id.to_string())));
    }
    if body.size == Some(0) {
        return Ok(DecodedPart::Text(String::new()));
    }
    if body.size.is_some() {
        return Ok(DecodedPart::NeedsFetch(None));
    }
    // A present body object with neither bytes nor size is omitted data, not proof of
    // an empty body. Keep it visible to the caller as unresolved.
    Ok(DecodedPart::NeedsFetch(None))
}

fn decode_base64(data: &str, path: &[usize]) -> Result<Vec<u8>, BodyError> {
    let decoded = if data.contains('=') {
        URL_SAFE.decode(data.as_bytes())
    } else {
        URL_SAFE_NO_PAD.decode(data.as_bytes())
    };
    decoded.map_err(|_| {
        BodyError::new(BodyErrorKind::InvalidBase64, path.to_vec())
    })
}

fn decode_text(
    bytes: Vec<u8>,
    part: &MimePart,
    path: &[usize],
) -> Result<String, BodyError> {
    let charset = content_type_parameter(part, "charset");
    let encoding = match charset {
        Some(label) => {
            Encoding::for_label(label.as_bytes()).ok_or_else(|| {
                BodyError::new(BodyErrorKind::UnsupportedCharset, path.to_vec())
            })?
        }
        None => encoding_rs::UTF_8,
    };
    // Invalid sequences are replaced with U+FFFD deterministically. In particular,
    // Gmail's base64-decoded bytes are not subjected to a second quoted-printable pass.
    let (text, _encoding_used, _had_errors) = encoding.decode(&bytes);
    Ok(text.into_owned())
}

fn is_attachment(part: &MimePart) -> bool {
    if base_mime_type(&part.mime_type) == "message/rfc822" {
        return true;
    }
    if disposition(part)
        .is_some_and(|value| value.eq_ignore_ascii_case("attachment"))
    {
        return true;
    }
    if base_mime_type(&part.mime_type).starts_with("image/")
        && content_id(part).is_some()
    {
        return false;
    }
    if part
        .filename
        .as_deref()
        .is_some_and(|name| !name.is_empty())
    {
        return true;
    }
    false
}

fn content_id(part: &MimePart) -> Option<&str> {
    part.headers
        .iter()
        .find(|header| header.name.eq_ignore_ascii_case("Content-ID"))
        .map(|header| header.value.trim())
        .filter(|value| !value.is_empty())
}

fn disposition(part: &MimePart) -> Option<&str> {
    part.headers
        .iter()
        .find(|header| header.name.eq_ignore_ascii_case("Content-Disposition"))
        .and_then(|header| header.value.split(';').next())
        .map(str::trim)
}

fn related_root_index(part: &MimePart) -> Option<usize> {
    let start = content_type_parameter(part, "start")?;
    let start = normalize_content_id(start);
    part.parts.iter().position(|child| {
        content_id(child).is_some_and(|cid| normalize_content_id(cid) == start)
    })
}

fn normalize_content_id(value: &str) -> &str {
    value
        .trim()
        .trim_start_matches('<')
        .trim_end_matches('>')
        .trim()
}

fn base_mime_type(value: &str) -> String {
    value
        .split(';')
        .next()
        .unwrap_or(value)
        .trim()
        .to_ascii_lowercase()
}

fn content_type_parameter<'a>(
    part: &'a MimePart,
    wanted: &str,
) -> Option<&'a str> {
    let raw = part
        .headers
        .iter()
        .find(|header| header.name.eq_ignore_ascii_case("Content-Type"))
        .map(|header| header.value.as_str())
        .unwrap_or(part.mime_type.as_str());
    raw.split(';').skip(1).find_map(|fragment| {
        let (key, value) = fragment.split_once('=')?;
        if !key.trim().eq_ignore_ascii_case(wanted) {
            return None;
        }
        Some(value.trim().trim_matches('"').trim_matches('\''))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native::gmail::model::{GmailThread, MimeBody};

    fn message(json_payload: &str) -> GmailMessage {
        let input = format!(
            r#"{{"thread":{{"id":"t","messages":[{{"id":"m","threadId":"t","payload":{json_payload}}}]}}}}"#
        );
        GmailThread::from_gog_json(input.as_bytes())
            .unwrap()
            .messages
            .remove(0)
    }

    fn collect(payload: &str) -> Result<MimeInventory, BodyError> {
        let message = message(payload);
        collect_message_bodies(&message, &FetchedBodies::default())
    }

    #[test]
    fn walks_nested_alternative_and_mixed_and_excludes_named_text_attachments()
    {
        let result = collect(
            r#"{"mimeType":"multipart/mixed","parts":[{"mimeType":"multipart/alternative","parts":[{"partId":"1","mimeType":"text/plain","body":{"size":5,"data":"cGxhaW4"}},{"partId":"2","mimeType":"text/html","body":{"size":6,"data":"PGI-aGk8L2I-"}}]},{"partId":"3","mimeType":"text/plain","filename":"notes.txt","body":{"size":5,"data":"bm90ZXM"}}]}"#,
        )
        .unwrap();
        assert_eq!(result.plain_candidates[0].text, "plain");
        assert_eq!(result.html_candidates[0].text, "<b>hi</b>");
        assert!(result.attachments[0].filename.as_deref() == Some("notes.txt"));
    }

    #[test]
    fn related_start_selects_root_and_collects_cid_resource() {
        let result = collect(
            r#"{"mimeType":"multipart/related","headers":[{"name":"Content-Type","value":"multipart/related; start=\"<root>\""}],"parts":[{"partId":"1","mimeType":"image/png","headers":[{"name":"Content-ID","value":"<logo>"}],"body":{"size":1,"data":"AA"}},{"partId":"2","mimeType":"text/html","headers":[{"name":"Content-ID","value":"<root>"}],"body":{"size":4,"data":"PGI-aGk8L2I-"}}]}"#,
        )
        .unwrap();
        assert!(result.plain_candidates.is_empty());
        assert_eq!(result.html_candidates[0].text, "<b>hi</b>");
        assert_eq!(result.inline_images.len(), 1);
        assert_eq!(
            result.inline_images[0].content_id.as_deref(),
            Some("<logo>")
        );
    }

    #[test]
    fn named_inline_cid_image_stays_an_inline_resource() {
        let result = collect(
            r#"{"mimeType":"multipart/related","parts":[{"mimeType":"text/html","body":{"size":2,"data":"aGk"}},{"mimeType":"image/png","filename":"logo.png","headers":[{"name":"Content-ID","value":"<logo>"},{"name":"Content-Disposition","value":"inline; filename=\"logo.png\""}],"body":{"size":1,"data":"AA"}}]}"#,
        )
        .unwrap();
        assert!(result.attachments.is_empty());
        assert_eq!(result.inline_images.len(), 1);
        assert_eq!(
            result.inline_images[0].filename.as_deref(),
            Some("logo.png")
        );
    }

    #[test]
    fn attachment_backed_body_is_unresolved_then_decoded_as_raw_fetched_bytes()
    {
        let message = message(
            r#"{"partId":"0","mimeType":"text/plain","headers":[{"name":"Content-Type","value":"text/plain; charset=utf-8"}],"body":{"size":5,"attachmentId":"opaque-id"}}"#,
        );
        let unresolved =
            collect_message_bodies(&message, &FetchedBodies::default())
                .unwrap();
        assert_eq!(unresolved.needs_fetch.len(), 1);
        assert!(unresolved.plain_candidates.is_empty());
        let mut fetched = FetchedBodies::default();
        fetched.insert("m", "opaque-id", b"hello".to_vec());
        let hydrated = collect_message_bodies(&message, &fetched).unwrap();
        assert_eq!(hydrated.plain_candidates[0].text, "hello");
        assert!(hydrated.needs_fetch.is_empty());
    }

    #[test]
    fn base64url_padding_charset_and_legacy_transfer_header_are_handled() {
        let utf8 = collect(
            r#"{"mimeType":"text/plain","body":{"size":2,"data":"w6k="}}"#,
        )
        .unwrap();
        assert_eq!(utf8.plain_candidates[0].text, "é");
        let windows = collect(r#"{"mimeType":"text/plain","headers":[{"name":"Content-Type","value":"text/plain; charset=windows-1252"},{"name":"Content-Transfer-Encoding","value":"quoted-printable"}],"body":{"size":5,"data":"PUU0MQ"}}"#).unwrap();
        assert_eq!(windows.plain_candidates[0].text, "=E41");
        let latin = collect(r#"{"mimeType":"text/plain","headers":[{"name":"Content-Type","value":"text/plain; charset=iso-8859-1"}],"body":{"size":1,"data":"6Q"}}"#).unwrap();
        assert_eq!(latin.plain_candidates[0].text, "é");
    }

    #[test]
    fn empty_body_is_candidate_but_omitted_body_is_not_empty() {
        let empty =
            collect(r#"{"mimeType":"text/plain","body":{"size":0}}"#).unwrap();
        assert_eq!(empty.plain_candidates[0].text, "");
        let omitted = collect(r#"{"mimeType":"text/plain"}"#).unwrap();
        assert!(omitted.plain_candidates.is_empty());
    }

    #[test]
    fn size_mismatch_is_metadata_and_bad_encoding_errors_are_redacted() {
        let mismatch = collect(
            r#"{"mimeType":"text/plain","body":{"size":500,"data":"aGk"}}"#,
        )
        .unwrap();
        assert_eq!(mismatch.plain_candidates[0].text, "hi");
        let error = collect(r#"{"mimeType":"text/plain","body":{"size":8,"data":"SENSITIVE%%%"}}"#).err().unwrap();
        assert_eq!(error.kind, BodyErrorKind::InvalidBase64);
        assert!(!error.to_string().contains("SENSITIVE"));
        let error = collect(r#"{"mimeType":"text/plain","headers":[{"name":"Content-Type","value":"text/plain; charset=no-such-charset"}],"body":{"size":4,"data":"aGV5"}}"#).err().unwrap();
        assert_eq!(error.kind, BodyErrorKind::UnsupportedCharset);
    }

    #[test]
    fn message_rfc822_and_attachment_disposition_are_never_main_text() {
        let result = collect(
            r#"{"mimeType":"multipart/mixed","parts":[{"mimeType":"message/rfc822","body":{"size":4,"data":"bm9wZQ"}},{"mimeType":"text/plain","headers":[{"name":"Content-Disposition","value":"attachment; filename=\"report.txt\""}],"body":{"size":4,"data":"bm9wZQ"}}]}"#,
        )
        .unwrap();
        assert!(result.plain_candidates.is_empty());
        assert_eq!(result.attachments.len(), 2);
    }

    #[test]
    fn traversal_depth_and_node_limits_fail_typed() {
        let mut tree = MimePart {
            part_id: None,
            mime_type: "multipart/mixed".to_string(),
            filename: None,
            headers: Vec::new(),
            body: None,
            parts: Vec::new(),
        };
        for _ in 0..MAX_MIME_DEPTH + 2 {
            tree = MimePart {
                part_id: None,
                mime_type: "multipart/mixed".to_string(),
                filename: None,
                headers: Vec::new(),
                body: None,
                parts: vec![tree],
            };
        }
        let error = validate_tree(&tree, MAX_MIME_DEPTH, MAX_MIME_NODES, &[])
            .unwrap_err();
        assert_eq!(error.kind, BodyErrorKind::TraversalLimitExceeded);
        let mut wide = tree.parts[0].clone();
        wide.parts = (0..MAX_MIME_NODES + 1)
            .map(|_| MimePart {
                part_id: None,
                mime_type: "application/octet-stream".to_string(),
                filename: None,
                headers: Vec::new(),
                body: None,
                parts: Vec::new(),
            })
            .collect();
        let error = validate_tree(&wide, MAX_MIME_DEPTH, MAX_MIME_NODES, &[])
            .unwrap_err();
        assert_eq!(error.kind, BodyErrorKind::TraversalLimitExceeded);
    }

    #[test]
    fn invalid_utf8_uses_replacement_character() {
        let text = decode_text(
            vec![0xff],
            &MimePart {
                part_id: None,
                mime_type: "text/plain".to_string(),
                filename: None,
                headers: Vec::new(),
                body: Some(MimeBody {
                    size: Some(1),
                    data: None,
                    attachment_id: None,
                }),
                parts: Vec::new(),
            },
            &[],
        )
        .unwrap();
        assert_eq!(text, "�");
    }
}
