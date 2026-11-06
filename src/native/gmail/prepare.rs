//! In-memory Gmail save preparation: attachment-body hydration and CID image planning.
//!
//! This is the `prepare-content` step of gmail-save. It takes a confirmed
//! [`GmailThread`] plus the selected account and body mode, and returns
//! [`PreparedThread`]: rendered messages with trusted image embeds applied,
//! proposed binary imports, body-free origin descriptors, and typed skip
//! reasons. Nothing here writes to the vault, the journals, or the network
//! cache: all network work is read-only Gmail fetching, and every produced
//! value stays in memory for the save-transaction phase to plan against and
//! install under the vault lock.
//!
//! Trust boundary, for the save-transaction consumer:
//!
//! * Full mode renders each importable message, fetches every
//!   attachment-backed body part and every plannable CID image at most once
//!   into [`FetchedBodies`], then re-renders. A remaining fetch need or a
//!   transport failure fails preparation outright rather than saving a
//!   silently truncated fallback.
//! * Headers mode performs zero body-part or image downloads. It returns the
//!   attachment metadata descriptors with all prose fields cleared, so no
//!   body text reaches any downstream output through this path.
//! * Only referenced, non-tracking CID images that the pure renderer
//!   supplies are planned; the renderer already drops tracking pixels,
//!   bulletin images, and ambiguous references. Ordinary attachments stay
//!   listing-only descriptors.
//! * Images are capped at 2 MiB (metadata size skips the download, received
//!   length skips the import), deduplicated by sha256 within the thread, and
//!   small repeats across distinct messages are suppressed as signature
//!   logos. Names are sanitized with safe mime-derived extensions under
//!   `img/mail/<thread hex>/`, collisions get deterministic suffixes, and
//!   reuse is by verified hash, never by sender filename.
//! * Embeds are appended structurally in stable descriptor order, never by
//!   replacing sender-text placeholders, so sender strings cannot forge
//!   embeds. Only the writer-owned minted paths pass through
//!   [`verified_local_image_wikilink`], which yields exactly
//!   `![[img/mail/…|400]]`.
//! * Origin descriptors carry a deterministic Gmail identity locator built
//!   only from account/message/part IDs plus the saved path and sha256: no
//!   raw URL token and no bytes.
//!
//! Prepared body values stay private to the write path: [`PreparedThread`]
//! and [`BinaryImport`] deliberately have no `Debug` dumps, no serde
//! exposure, no telemetry, and no persisted body cache. Descriptors that
//! reach diagnostics ([`OriginDescriptor`], [`SkippedImage`]) carry only
//! opaque IDs, paths, hashes, and typed reasons.

use std::collections::{BTreeMap, BTreeSet};

use sha2::{Digest, Sha256};

use super::{
    body::{
        importable_messages, render_message, verified_local_image_wikilink,
        FetchedBodies, InlineImage, InlineImageSource, RenderedBody,
    },
    model::{BodyError, BodyErrorKind, GmailMessage, GmailThread},
    resolve::GmailRead,
    GmailError,
};
use crate::native::config::GmailBodyMode;

/// Largest CID image import: 2 MiB by metadata size and by received length.
pub(crate) const MAX_IMAGE_BYTES: u64 = 2 * 1024 * 1024;

/// Images smaller than this that repeat across distinct messages are
/// signature logos and are suppressed. Exactly this size is kept.
const LOGO_BYTES_BELOW: usize = 4096;

/// Vault directory holding saved mail images, below the configured folder.
const IMAGE_DIR: &str = "img/mail";

/// Longest sanitized image stem; keeps paths readable on every platform.
const MAX_STEM_CHARS: usize = 64;

/// A confirmed thread prepared for the save-transaction phase. Bodies and
/// bytes live here in memory only: nothing is written yet.
pub(crate) struct PreparedThread {
    /// Rendered bodies keyed by message ID, with trusted image embeds
    /// appended in full mode. Prose fields are cleared in headers mode.
    pub(crate) rendered: BTreeMap<String, RenderedBody>,
    /// Proposed binary imports: vault-relative path plus bytes. The
    /// transaction phase rechecks collisions and symlinks before installing.
    pub(crate) imports: Vec<BinaryImport>,
    /// Body-free origin descriptors for `append_origin_events(..., "gmail")`.
    pub(crate) origins: Vec<OriginDescriptor>,
    /// Images the planner skipped, with typed body-free reasons.
    pub(crate) skipped: Vec<SkippedImage>,
}

/// One proposed binary import: destination path and bytes, in memory.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct BinaryImport {
    pub(crate) relative_path: String,
    pub(crate) bytes: Vec<u8>,
}

/// Body-free attachment origin descriptor: saved path, deterministic Gmail
/// identity locator, and content hash. No URL token and no bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OriginDescriptor {
    pub(crate) saved: String,
    pub(crate) source: String,
    pub(crate) sha256: String,
}

/// One skipped CID image with a typed, body-free reason.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SkippedImage {
    pub(crate) message_id: String,
    pub(crate) reason: ImageSkipReason,
}

/// Why a CID image candidate was not imported. Carries no sender bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ImageSkipReason {
    /// Larger than [`MAX_IMAGE_BYTES`], by metadata or received length.
    Oversized,
    /// Not a safely embeddable raster image (SVG and unknown types included).
    UnsupportedType,
    /// Small bytes repeating across distinct messages: a signature logo.
    SignatureLogo,
    /// No bytes to import (unresolvable reference or minted-path failure).
    Unavailable,
}

/// Prepare a confirmed thread entirely in memory: hydrate attachment-backed
/// bodies, render messages, and plan CID image imports.
///
/// Full mode fetches every needed attachment body at most once before the
/// vault lock and fails on any remaining fetch need or transport failure.
/// Headers mode performs no downloads and clears all prose fields. Drafts
/// are excluded in both modes.
pub(crate) fn prepare_thread<R: GmailRead>(
    reader: &R,
    account: &str,
    thread: &GmailThread,
    body_mode: GmailBodyMode,
) -> Result<PreparedThread, GmailError> {
    let thread_hex = normalize_thread_hex(&thread.id)?;
    let full = body_mode == GmailBodyMode::Full;
    let mut fetched = FetchedBodies::default();
    let mut bodies: Vec<(String, RenderedBody)> = Vec::new();
    let mut occurrences: Vec<ImageOccurrence> = Vec::new();
    let mut prepared = PreparedThread {
        rendered: BTreeMap::new(),
        imports: Vec::new(),
        origins: Vec::new(),
        skipped: Vec::new(),
    };

    for (message_index, message) in importable_messages(thread).enumerate() {
        let mut rendered =
            render_message(message, &fetched).map_err(render_failed)?;
        if full {
            // Pass 1 fetches attachment-backed body parts; pass 2 fetches
            // the CID image bytes those hydrated bodies reveal. The
            // provisional first render is a snippet fallback with no image
            // descriptors, so image needs only appear after pass 1.
            hydrate_bodies(reader, account, message, &mut fetched, &rendered)?;
            rendered =
                render_message(message, &fetched).map_err(render_failed)?;
            if !rendered.needs_fetch.is_empty() {
                return Err(fetch_failed());
            }
            // Occurrence descriptors come from this render, whose image
            // sources still name their attachment IDs. The image fetch fills
            // in the bytes afterwards; the final render below only supplies
            // the markdown (identical prose either way) and the empty
            // fetch-need check.
            collect_occurrences(
                message_index,
                message,
                &rendered,
                &mut prepared.skipped,
                &mut occurrences,
            );
            hydrate_images(reader, account, message, &mut fetched, &rendered)?;
            rendered =
                render_message(message, &fetched).map_err(render_failed)?;
            if !rendered.needs_fetch.is_empty() {
                return Err(fetch_failed());
            }
        } else {
            strip_prose(&mut rendered);
        }
        bodies.push((message.id.clone(), rendered));
    }

    let mut plan = ImagePlan::default();
    let embeds = plan_images(
        &thread_hex,
        account,
        occurrences,
        &fetched,
        bodies.len(),
        &mut plan,
    );
    prepared.imports = plan.imports;
    prepared.origins = plan.origins;
    // Collection-time skips (unsupported type, oversized metadata,
    // unresolvable stubs) precede planning-time skips in stable order.
    prepared.skipped.append(&mut plan.skipped);
    for ((message_id, mut rendered), message_embeds) in
        bodies.into_iter().zip(embeds)
    {
        if !message_embeds.is_empty() {
            if !rendered.markdown.is_empty() {
                rendered.markdown.push_str("\n\n");
            }
            rendered.markdown.push_str(&message_embeds.join("\n\n"));
        }
        prepared.rendered.insert(message_id, rendered);
    }
    Ok(prepared)
}

/// One plannable CID image occurrence, in stable descriptor order. Bytes
/// resolve post-fetch in [`plan_images`]: pre-fetch descriptors still name
/// their attachment IDs, which the origin locator needs.
struct ImageOccurrence {
    message_index: usize,
    message_id: String,
    image: InlineImage,
}

/// Map a pure-renderer failure to a body-free typed error. Drafts are
/// filtered before this point, so reaching `DraftRefused` is a refusal, not
/// malformed JSON; every other renderer failure is a malformed body.
fn render_failed(error: BodyError) -> GmailError {
    let kind = match error.kind {
        BodyErrorKind::DraftRefused => "refused",
        _ => "bad_json",
    };
    GmailError::runtime(kind, error.to_string())
}

/// A body part with neither inline bytes nor a fetchable attachment ID can
/// never resolve: fail rather than saving a truncated fallback.
fn fetch_failed() -> GmailError {
    GmailError::runtime(
        "fetch_failed",
        "Gmail attachment bodies could not be fully hydrated".to_string(),
    )
}

/// Fetch every attachment-backed body part this message still needs, at most
/// once per `(message, attachment)` pair. Body candidates that can never
/// resolve fail here rather than saving a truncated fallback.
fn hydrate_bodies<R: GmailRead>(
    reader: &R,
    account: &str,
    message: &GmailMessage,
    fetched: &mut FetchedBodies,
    first: &RenderedBody,
) -> Result<(), GmailError> {
    let mut wanted: BTreeSet<&str> = BTreeSet::new();
    for need in &first.needs_fetch {
        match need.attachment_id.as_deref() {
            Some(attachment_id) => {
                wanted.insert(attachment_id);
            }
            None => return Err(fetch_failed()),
        }
    }
    fetch_missing(reader, account, message, fetched, wanted.into_iter())
}

/// Fetch the attachment bytes behind plannable CID images, at most once per
/// `(message, attachment)` pair. Images the planner will skip (unsupported
/// type, oversized metadata) are never downloaded.
fn hydrate_images<R: GmailRead>(
    reader: &R,
    account: &str,
    message: &GmailMessage,
    fetched: &mut FetchedBodies,
    hydrated: &RenderedBody,
) -> Result<(), GmailError> {
    let wanted: BTreeSet<&str> = hydrated
        .inline_images
        .iter()
        .filter(|image| is_plannable(image))
        .filter_map(|image| match &image.source {
            InlineImageSource::AttachmentId(attachment_id) => {
                Some(attachment_id.as_str())
            }
            InlineImageSource::Bytes(_) => None,
        })
        .collect();
    fetch_missing(reader, account, message, fetched, wanted.into_iter())
}

/// Download every wanted attachment ID not already in [`FetchedBodies`].
fn fetch_missing<'a, R: GmailRead>(
    reader: &R,
    account: &str,
    message: &GmailMessage,
    fetched: &mut FetchedBodies,
    wanted: impl Iterator<Item = &'a str>,
) -> Result<(), GmailError> {
    for attachment_id in wanted {
        if fetched.get(&message.id, attachment_id).is_none() {
            let bytes =
                reader.attachment_bytes(account, &message.id, attachment_id)?;
            fetched.insert(
                message.id.clone(),
                attachment_id.to_string(),
                bytes,
            );
        }
    }
    Ok(())
}

/// Images the planner may import: safely embeddable raster type and metadata
/// within the byte cap. Anything else is skipped without downloading.
fn is_plannable(image: &InlineImage) -> bool {
    if image_mime_extension(image).is_none() {
        return false;
    }
    if image.size.is_some_and(|size| size > MAX_IMAGE_BYTES) {
        return false;
    }
    true
}

/// Describe plannable CID image occurrences from the body-hydrated render.
/// Skipped candidates record a typed, body-free reason; nothing is
/// downloaded here. Bytes resolve post-fetch in [`plan_images`].
fn collect_occurrences(
    message_index: usize,
    message: &GmailMessage,
    rendered: &RenderedBody,
    skipped: &mut Vec<SkippedImage>,
    out: &mut Vec<ImageOccurrence>,
) {
    for image in &rendered.inline_images {
        let skip = |reason: ImageSkipReason,
                    skipped: &mut Vec<SkippedImage>| {
            skipped.push(SkippedImage {
                message_id: message.id.clone(),
                reason,
            });
        };
        if !is_plannable(image) {
            skip(
                if image_mime_extension(image).is_none() {
                    ImageSkipReason::UnsupportedType
                } else {
                    ImageSkipReason::Oversized
                },
                skipped,
            );
            continue;
        }
        // An unresolved CID without a provider ID carries an empty stub.
        if let InlineImageSource::Bytes(bytes) = &image.source
            && bytes.is_empty()
        {
            skip(ImageSkipReason::Unavailable, skipped);
            continue;
        }
        out.push(ImageOccurrence {
            message_index,
            message_id: message.id.clone(),
            image: image.clone(),
        });
    }
}

/// Accumulator for image planning outputs, threaded through [`plan_images`].
#[derive(Default)]
struct ImagePlan {
    imports: Vec<BinaryImport>,
    origins: Vec<OriginDescriptor>,
    skipped: Vec<SkippedImage>,
}

/// Resolve occurrence bytes post-fetch, then assign destinations,
/// deduplicate by hash, suppress signature logos, and mint embeds in stable
/// occurrence order. Returns per-message embed lists aligned with the
/// prepared message order.
fn plan_images(
    thread_hex: &str,
    account: &str,
    occurrences: Vec<ImageOccurrence>,
    fetched: &FetchedBodies,
    message_count: usize,
    plan: &mut ImagePlan,
) -> Vec<Vec<String>> {
    let mut embeds: Vec<Vec<String>> = vec![Vec::new(); message_count];

    // Resolve bytes: inline data is carried by the descriptor, fetched
    // attachment bytes come from the hydrator. Empty or over-cap received
    // bytes skip here, catching metadata that lied about the size.
    let mut live: Vec<(usize, Vec<u8>)> = Vec::new();
    let ImagePlan {
        imports,
        origins,
        skipped,
    } = plan;
    for (index, occurrence) in occurrences.iter().enumerate() {
        let bytes = match &occurrence.image.source {
            InlineImageSource::Bytes(bytes) => bytes.clone(),
            InlineImageSource::AttachmentId(attachment_id) => {
                match fetched.get(&occurrence.message_id, attachment_id) {
                    Some(bytes) => bytes.to_vec(),
                    None => {
                        skipped.push(SkippedImage {
                            message_id: occurrence.message_id.clone(),
                            reason: ImageSkipReason::Unavailable,
                        });
                        continue;
                    }
                }
            }
        };
        if bytes.is_empty() {
            skipped.push(SkippedImage {
                message_id: occurrence.message_id.clone(),
                reason: ImageSkipReason::Unavailable,
            });
            continue;
        }
        if bytes.len() as u64 > MAX_IMAGE_BYTES {
            skipped.push(SkippedImage {
                message_id: occurrence.message_id.clone(),
                reason: ImageSkipReason::Oversized,
            });
            continue;
        }
        live.push((index, bytes));
    }

    let mut by_hash: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for (position, (_, bytes)) in live.iter().enumerate() {
        by_hash
            .entry(hex::encode(Sha256::digest(bytes)))
            .or_default()
            .push(position);
    }
    let mut suppressed: BTreeSet<&str> = BTreeSet::new();
    for (sha, positions) in &by_hash {
        let small = live[positions[0]].1.len() < LOGO_BYTES_BELOW;
        if !small {
            continue;
        }
        let mut messages = BTreeSet::new();
        for position in positions {
            messages.insert(occurrences[live[*position].0].message_index);
        }
        // Repeats across distinct messages are signature logos. Repeats
        // within one message stay: the sender placed them deliberately.
        if messages.len() > 1 {
            suppressed.insert(sha.as_str());
        }
    }

    let mut destinations: BTreeMap<String, String> = BTreeMap::new();
    let mut used_names: BTreeMap<String, String> = BTreeMap::new();
    for (index, bytes) in &live {
        let occurrence = &occurrences[*index];
        let sha = hex::encode(Sha256::digest(bytes));
        if suppressed.contains(sha.as_str()) {
            skipped.push(SkippedImage {
                message_id: occurrence.message_id.clone(),
                reason: ImageSkipReason::SignatureLogo,
            });
            continue;
        }
        let relative_path = match destinations.get(&sha) {
            Some(path) => path.clone(),
            None => {
                let path = assign_destination(
                    thread_hex,
                    &occurrence.image,
                    &used_names,
                );
                used_names.insert(path.clone(), sha.clone());
                destinations.insert(sha.clone(), path.clone());
                imports.push(BinaryImport {
                    relative_path: path.clone(),
                    bytes: bytes.clone(),
                });
                path
            }
        };
        let Some(embed) = verified_local_image_wikilink(&relative_path) else {
            skipped.push(SkippedImage {
                message_id: occurrence.message_id.clone(),
                reason: ImageSkipReason::Unavailable,
            });
            continue;
        };
        origins.push(OriginDescriptor {
            saved: relative_path,
            source: origin_source(account, occurrence),
            sha256: sha,
        });
        embeds[occurrence.message_index].push(embed);
    }
    embeds
}

/// Deterministic destination for new bytes: sanitized stem, safe
/// mime-derived extension, and a numeric suffix only on hash collisions.
/// Reuse is by verified hash, never by sender filename.
fn assign_destination(
    thread_hex: &str,
    image: &InlineImage,
    used_names: &BTreeMap<String, String>,
) -> String {
    let extension = image_mime_extension(image).unwrap_or("bin");
    let stem = candidate_stem(image);
    let mut candidate = format!("{IMAGE_DIR}/{thread_hex}/{stem}.{extension}");
    let mut suffix = 2u32;
    while used_names.contains_key(&candidate) {
        candidate =
            format!("{IMAGE_DIR}/{thread_hex}/{stem}_{suffix}.{extension}");
        suffix = suffix.saturating_add(1);
    }
    candidate
}

/// Deterministic Gmail identity locator from opaque IDs only: account,
/// message, and attachment or content ID. No URL token and no bytes.
fn origin_source(account: &str, occurrence: &ImageOccurrence) -> String {
    let part = match &occurrence.image.source {
        InlineImageSource::AttachmentId(attachment_id) => attachment_id.clone(),
        InlineImageSource::Bytes(_) => normalize_cid_text(
            occurrence.image.content_id.as_deref().unwrap_or("image"),
        ),
    };
    format!("gmail:{account}/{}/{part}", occurrence.message_id)
}

/// Safe raster extension for an inline image MIME type, or `None` for
/// anything that must stay a listing (SVG and unknown types included).
fn image_mime_extension(image: &InlineImage) -> Option<&'static str> {
    let base = image
        .mime_type
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    match base.as_str() {
        "image/png" => Some("png"),
        "image/jpeg" => Some("jpg"),
        "image/gif" => Some("gif"),
        "image/webp" => Some("webp"),
        _ => None,
    }
}

/// Sanitized image stem: sender filename stem, else content ID, else
/// `image`. Lowercased ASCII slug, truncated, never empty.
fn candidate_stem(image: &InlineImage) -> String {
    if let Some(filename) = image.filename.as_deref() {
        let base = filename.rsplit('/').next().unwrap_or(filename);
        let stem = base.rsplit_once('.').map(|(stem, _)| stem).unwrap_or(base);
        let clean = sanitize_stem(stem);
        if !clean.is_empty() {
            return clean;
        }
    }
    if let Some(content_id) = image.content_id.as_deref() {
        let clean = sanitize_stem(&normalize_cid_text(content_id));
        if !clean.is_empty() {
            return clean;
        }
    }
    "image".to_string()
}

/// Lowercase ASCII slug: runs of unsafe characters fold to one `_`,
/// trimmed and truncated at a char boundary (output is pure ASCII).
fn sanitize_stem(raw: &str) -> String {
    let mut out = String::new();
    let mut separator = true;
    for ch in raw.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            separator = false;
        } else if !separator {
            out.push('_');
            separator = true;
        }
    }
    while out.ends_with('_') {
        out.pop();
    }
    if out.len() > MAX_STEM_CHARS {
        out.truncate(MAX_STEM_CHARS);
        while out.ends_with('_') {
            out.pop();
        }
    }
    out
}

/// Content-ID text without brackets or surrounding whitespace.
fn normalize_cid_text(value: &str) -> String {
    value
        .trim()
        .trim_start_matches('<')
        .trim_end_matches('>')
        .trim()
        .to_string()
}

/// Lowercase hex thread ID for the image directory. Confirmed threads carry
/// canonical hex; anything else refuses without echoing the value.
fn normalize_thread_hex(thread_id: &str) -> Result<String, GmailError> {
    let hex = thread_id.to_ascii_lowercase();
    if !hex.is_empty() && hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        Ok(hex)
    } else {
        Err(GmailError::runtime(
            "unresolvable",
            "confirmed thread has no usable id".to_string(),
        ))
    }
}

/// Headers mode keeps attachment metadata but exposes no body prose: prose
/// fields and unplanned image descriptors are cleared.
fn strip_prose(rendered: &mut RenderedBody) {
    rendered.markdown.clear();
    rendered.quoted = None;
    rendered.signature = None;
    rendered.inline_images.clear();
    rendered.needs_fetch.clear();
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};

    use super::super::resolve::GmailRead;

    const THREAD_HEX: &str = "aabbccdd00112233";
    const ACCOUNT: &str = "me@example.com";
    const MSG1: &str = "1000000000000001";
    const MSG2: &str = "1000000000000002";

    /// In-memory attachment transport: bytes by `(message, attachment)` pair
    /// plus the call log, so tests assert fetch-once behavior and zero-fetch
    /// headers mode without a subprocess.
    #[derive(Default)]
    struct MockFetch {
        bodies: BTreeMap<(String, String), Vec<u8>>,
        calls: RefCell<Vec<(String, String)>>,
        error: Option<GmailError>,
    }

    impl MockFetch {
        fn with_body(
            mut self,
            message: &str,
            attachment: &str,
            bytes: Vec<u8>,
        ) -> Self {
            self.bodies
                .insert((message.to_string(), attachment.to_string()), bytes);
            self
        }

        fn failing(mut self, error: GmailError) -> Self {
            self.error = Some(error);
            self
        }

        fn calls(&self) -> Vec<(String, String)> {
            self.calls.borrow().clone()
        }
    }

    impl GmailRead for MockFetch {
        fn attachment_bytes(
            &self,
            _: &str,
            message_id: &str,
            attachment_id: &str,
        ) -> Result<Vec<u8>, GmailError> {
            self.calls
                .borrow_mut()
                .push((message_id.to_string(), attachment_id.to_string()));
            if let Some(error) = &self.error {
                return Err(error.clone());
            }
            self.bodies
                .get(&(message_id.to_string(), attachment_id.to_string()))
                .cloned()
                .ok_or_else(|| {
                    GmailError::runtime(
                        "gone",
                        "mock transport has no such attachment".to_string(),
                    )
                })
        }

        fn thread_get(
            &self,
            _: &str,
            _: &str,
        ) -> Result<GmailThread, GmailError> {
            unreachable!()
        }

        fn message_metadata(
            &self,
            _: &str,
            _: &str,
        ) -> Result<serde_json::Value, GmailError> {
            unreachable!()
        }

        fn search_page(
            &self,
            _: &str,
            _: &str,
            _: u32,
            _: Option<&str>,
        ) -> Result<serde_json::Value, GmailError> {
            unreachable!()
        }

        fn labels_list(
            &self,
            _: &str,
        ) -> Result<Vec<super::super::gog::GmailLabel>, GmailError> {
            unreachable!()
        }
    }

    fn encode(bytes: &[u8]) -> String {
        URL_SAFE_NO_PAD.encode(bytes)
    }

    fn payload_headers(pairs: &[(&str, &str)]) -> serde_json::Value {
        serde_json::Value::Array(
            pairs
                .iter()
                .map(|(name, value)| {
                    serde_json::json!({"name": name, "value": value})
                })
                .collect(),
        )
    }

    fn html_part(part_id: &str, body: serde_json::Value) -> serde_json::Value {
        serde_json::json!({
            "partId": part_id,
            "mimeType": "text/html",
            "headers": payload_headers(&[(
                "Content-Type",
                "text/html; charset=utf-8",
            )]),
            "body": body,
        })
    }

    fn plain_part(part_id: &str, body: serde_json::Value) -> serde_json::Value {
        serde_json::json!({
            "partId": part_id,
            "mimeType": "text/plain",
            "headers": payload_headers(&[(
                "Content-Type",
                "text/plain; charset=utf-8",
            )]),
            "body": body,
        })
    }

    fn image_part(
        cid: &str,
        mime: &str,
        filename: Option<&str>,
        body: serde_json::Value,
    ) -> serde_json::Value {
        let mut part = serde_json::json!({
            "partId": format!("img-{cid}"),
            "mimeType": mime,
            "headers": payload_headers(&[("Content-ID", cid)]),
            "body": body,
        });
        if let Some(name) = filename {
            part["filename"] = serde_json::Value::from(name);
        }
        part
    }

    fn attachment_part(
        filename: &str,
        attachment_id: &str,
        size: u64,
    ) -> serde_json::Value {
        serde_json::json!({
            "partId": "att-1",
            "mimeType": "application/pdf",
            "filename": filename,
            "headers": payload_headers(&[(
                "Content-Disposition",
                "attachment",
            )]),
            "body": {"size": size, "attachmentId": attachment_id},
        })
    }

    fn message(
        id: &str,
        labels: &[&str],
        parts: Vec<serde_json::Value>,
        headers: Vec<(&str, &str)>,
    ) -> serde_json::Value {
        serde_json::json!({
            "id": id,
            "threadId": THREAD_HEX,
            "labelIds": labels,
            "internalDate": "1793908800000",
            "snippet": "snippet line",
            "payload": {
                "mimeType": "multipart/related",
                "headers": payload_headers(
                    &headers
                        .iter()
                        .map(|(name, value)| (*name, *value))
                        .collect::<Vec<_>>(),
                ),
                "parts": parts,
            },
        })
    }

    fn make_thread(messages: Vec<serde_json::Value>) -> GmailThread {
        make_thread_with_id(THREAD_HEX, messages)
    }

    fn make_thread_with_id(
        id: &str,
        messages: Vec<serde_json::Value>,
    ) -> GmailThread {
        let envelope = serde_json::json!({
            "thread": {"id": id, "historyId": "1", "messages": messages},
        });
        GmailThread::from_gog_json(envelope.to_string().as_bytes())
            .expect("fixture must parse")
    }

    fn full(reader: &MockFetch, thread: &GmailThread) -> PreparedThread {
        prepare_thread(reader, ACCOUNT, thread, GmailBodyMode::Full)
            .expect("full preparation must succeed")
    }

    #[test]
    fn full_hydrates_attachment_backed_bodies_without_double_decoding() {
        let html = r#"<p>Hello <img src="cid:logo" alt="Brand"></p>"#;
        let image = vec![0x89, b'P', b'N', b'G', 1, 2, 3, 4];
        let thread = make_thread(vec![message(
            MSG1,
            &["INBOX"],
            vec![
                html_part(
                    "body-1",
                    serde_json::json!({
                        "size": html.len(),
                        "attachmentId": "body-1",
                    }),
                ),
                image_part(
                    "<logo>",
                    "image/png",
                    Some("Logo.PNG"),
                    serde_json::json!({
                        "size": image.len(),
                        "attachmentId": "img-1",
                    }),
                ),
                attachment_part("report.pdf", "att-9", 1024),
            ],
            vec![("From", "Ada <ada@example.com>")],
        )]);
        let reader = MockFetch::default()
            .with_body(MSG1, "body-1", html.as_bytes().to_vec())
            .with_body(MSG1, "img-1", image.clone());
        let prepared = full(&reader, &thread);

        // Each attachment body fetched exactly once; the unrelated ordinary
        // attachment is listed, never downloaded.
        let mut calls = reader.calls();
        calls.sort();
        assert_eq!(
            calls,
            vec![
                (MSG1.to_string(), "body-1".to_string()),
                (MSG1.to_string(), "img-1".to_string()),
            ]
        );

        let rendered = prepared.rendered.get(MSG1).expect("message rendered");
        assert!(rendered.markdown.contains("Hello"), "{}", rendered.markdown);
        // Raw bytes stored once: import bytes equal the original image bytes,
        // proving no double base64 decoding.
        assert_eq!(prepared.imports.len(), 1);
        assert_eq!(prepared.imports[0].bytes, image);
        let expected = format!("![[img/mail/{THREAD_HEX}/logo.png|400]]");
        assert!(
            prepared.rendered[MSG1].markdown.ends_with(&expected),
            "markdown:\n{}",
            prepared.rendered[MSG1].markdown
        );
        assert_eq!(rendered.attachments.len(), 1);
        assert_eq!(
            rendered.attachments[0].filename.as_deref(),
            Some("report.pdf")
        );

        assert_eq!(prepared.origins.len(), 1);
        assert_eq!(
            prepared.origins[0].saved,
            format!("img/mail/{THREAD_HEX}/logo.png")
        );
        assert_eq!(
            prepared.origins[0].source,
            format!("gmail:{ACCOUNT}/{MSG1}/img-1")
        );
        assert_eq!(
            prepared.origins[0].sha256,
            hex::encode(Sha256::digest(&image))
        );
        assert!(prepared.skipped.is_empty());
    }

    #[test]
    fn duplicated_fetch_need_downloads_only_once() {
        // Two identical plain candidates behind one attachment ID: the union
        // set fetches it once, then both hydrate.
        let thread = make_thread(vec![message(
            MSG1,
            &["INBOX"],
            vec![
                plain_part(
                    "a",
                    serde_json::json!({"size": 2, "attachmentId": "dup"}),
                ),
                plain_part(
                    "b",
                    serde_json::json!({"size": 2, "attachmentId": "dup"}),
                ),
            ],
            vec![("From", "Ada <ada@example.com>")],
        )]);
        let reader =
            MockFetch::default().with_body(MSG1, "dup", b"hi".to_vec());
        let prepared = full(&reader, &thread);
        assert_eq!(reader.calls().len(), 1);
        assert!(prepared.rendered[MSG1].markdown.contains("hi"));
    }

    #[test]
    fn transport_failure_fails_preparation_without_truncation() {
        let thread = make_thread(vec![message(
            MSG1,
            &["INBOX"],
            vec![plain_part(
                "a",
                serde_json::json!({"size": 5, "attachmentId": "missing"}),
            )],
            vec![("From", "Ada <ada@example.com>")],
        )]);
        // Transport errors surface with their kinds intact so later workers
        // can tell transient failures from permanent ones. The transport
        // boundary is body-free by contract; preparation adds no body bytes.
        let reader = MockFetch::default().failing(GmailError::runtime(
            "timeout",
            "gog timed out".to_string(),
        ));
        let error =
            prepare_thread(&reader, ACCOUNT, &thread, GmailBodyMode::Full)
                .err()
                .expect("failure must fail");
        assert_eq!(error.kind(), "timeout");

        // Preparation's own failures are static typed strings: an
        // unresolvable fetch need fails without echoing anything sender-side.
        let thread = make_thread(vec![message(
            MSG1,
            &["INBOX"],
            vec![plain_part("a", serde_json::json!({"size": 5}))],
            vec![("From", "Ada <ada@example.com>")],
        )]);
        let error = prepare_thread(
            &MockFetch::default(),
            ACCOUNT,
            &thread,
            GmailBodyMode::Full,
        )
        .err()
        .expect("unresolvable need must fail");
        assert_eq!(error.kind(), "fetch_failed");
        assert!(!error.message().contains("PRIVATE_SENTINEL"));
    }

    #[test]
    fn unresolvable_bodies_fail_instead_of_truncating() {
        // A body part with neither bytes nor an attachment ID can never be
        // fetched: preparation fails with a body-free typed error.
        let thread = make_thread(vec![message(
            MSG1,
            &["INBOX"],
            vec![plain_part("a", serde_json::json!({"size": 5}))],
            vec![("From", "Ada <ada@example.com>")],
        )]);
        let error = prepare_thread(
            &MockFetch::default(),
            ACCOUNT,
            &thread,
            GmailBodyMode::Full,
        )
        .err()
        .expect("unresolvable need must fail");
        assert_eq!(error.kind(), "fetch_failed");

        // A missing attachment on the transport fails the same way.
        let thread = make_thread(vec![message(
            MSG1,
            &["INBOX"],
            vec![plain_part(
                "a",
                serde_json::json!({"size": 5, "attachmentId": "gone"}),
            )],
            vec![("From", "Ada <ada@example.com>")],
        )]);
        let error = prepare_thread(
            &MockFetch::default(),
            ACCOUNT,
            &thread,
            GmailBodyMode::Full,
        )
        .err()
        .expect("missing attachment must fail");
        assert_eq!(error.kind(), "gone");
    }

    #[test]
    fn headers_mode_downloads_nothing_and_exposes_no_prose() {
        let html = r#"<p>Hello <img src="cid:logo" alt="Brand"></p>"#;
        let image = vec![0x89, b'P', b'N', b'G', 1, 2, 3, 4];
        let thread = make_thread(vec![message(
            MSG1,
            &["INBOX"],
            vec![
                html_part(
                    "body-1",
                    serde_json::json!({
                        "size": html.len(),
                        "attachmentId": "body-1",
                    }),
                ),
                image_part(
                    "<logo>",
                    "image/png",
                    Some("logo.png"),
                    serde_json::json!({
                        "size": image.len(),
                        "attachmentId": "img-1",
                    }),
                ),
                attachment_part("report.pdf", "att-9", 1024),
            ],
            vec![("From", "Ada <ada@example.com>")],
        )]);
        let reader = MockFetch::default()
            .with_body(MSG1, "body-1", html.as_bytes().to_vec())
            .with_body(MSG1, "img-1", image);
        let prepared =
            prepare_thread(&reader, ACCOUNT, &thread, GmailBodyMode::Headers)
                .expect("headers preparation must succeed");

        // Zero network use, even though bodies sit behind attachment IDs.
        assert!(reader.calls().is_empty());
        assert!(prepared.imports.is_empty());
        assert!(prepared.origins.is_empty());
        assert!(prepared.skipped.is_empty());

        let rendered = prepared.rendered.get(MSG1).expect("message rendered");
        assert!(rendered.markdown.is_empty());
        assert!(rendered.quoted.is_none());
        assert!(rendered.signature.is_none());
        assert!(rendered.inline_images.is_empty());
        assert!(rendered.needs_fetch.is_empty());
        // Attachment metadata survives for the card listing.
        assert_eq!(rendered.attachments.len(), 1);
        assert_eq!(
            rendered.attachments[0].filename.as_deref(),
            Some("report.pdf")
        );
    }

    #[test]
    fn drafts_are_excluded_without_fetching() {
        let thread = make_thread(vec![
            message(
                MSG1,
                &["INBOX", "DRAFT"],
                vec![plain_part(
                    "a",
                    serde_json::json!({
                        "size": 6,
                        "data": encode(b"secret"),
                    }),
                )],
                vec![("From", "Ada <ada@example.com>")],
            ),
            message(
                MSG2,
                &["INBOX"],
                vec![plain_part(
                    "a",
                    serde_json::json!({"size": 4, "data": encode(b"real")}),
                )],
                vec![("From", "Ada <ada@example.com>")],
            ),
        ]);
        let reader = MockFetch::default();
        let prepared = full(&reader, &thread);
        assert!(reader.calls().is_empty());
        assert_eq!(prepared.rendered.keys().collect::<Vec<_>>(), vec![MSG2]);
        assert!(prepared.rendered[MSG2].markdown.contains("real"));
        assert!(prepared.imports.is_empty());
    }

    #[test]
    fn bulletin_messages_plan_no_images() {
        let html = r#"<p>Sale <img src="cid:logo" alt="Brand"></p>"#;
        let image = vec![1, 2, 3, 4, 5, 6, 7, 8];
        let thread = make_thread(vec![message(
            MSG1,
            &["INBOX", "CATEGORY_PROMOTIONS"],
            vec![
                html_part(
                    "body-1",
                    serde_json::json!({"size": html.len(), "data": encode(html.as_bytes())}),
                ),
                image_part(
                    "<logo>",
                    "image/png",
                    Some("logo.png"),
                    serde_json::json!({
                        "size": image.len(),
                        "data": encode(&image),
                    }),
                ),
            ],
            vec![
                ("From", "Ada <ada@example.com>"),
                ("List-Unsubscribe", "<mailto:leave@example.com>"),
            ],
        )]);
        let prepared = full(&MockFetch::default(), &thread);
        assert!(prepared.imports.is_empty());
        assert!(prepared.origins.is_empty());
        assert!(!prepared.rendered[MSG1].markdown.contains("![["));
    }

    fn inline_image_message(
        id: &str,
        html: &str,
        parts: Vec<serde_json::Value>,
    ) -> serde_json::Value {
        let mut all = vec![html_part(
            "body",
            serde_json::json!({"size": html.len(), "data": encode(html.as_bytes())}),
        )];
        all.extend(parts);
        message(id, &["INBOX"], all, vec![("From", "Ada <ada@example.com>")])
    }

    #[test]
    fn duplicate_names_collide_but_identical_bytes_share_a_destination() {
        let bytes_a = vec![0x41; 100];
        let bytes_b = vec![0x42; 100];
        let html_a = r#"<p>A <img src="cid:a" alt="A"></p>"#;
        let html_b = r#"<p>B <img src="cid:b" alt="B"></p>"#;
        let thread = make_thread(vec![
            inline_image_message(
                MSG1,
                html_a,
                vec![image_part(
                    "<a>",
                    "image/png",
                    Some("photo.png"),
                    serde_json::json!({
                        "size": bytes_a.len(),
                        "data": encode(&bytes_a),
                    }),
                )],
            ),
            inline_image_message(
                MSG2,
                html_b,
                vec![image_part(
                    "<b>",
                    "image/png",
                    Some("photo.png"),
                    serde_json::json!({
                        "size": bytes_b.len(),
                        "data": encode(&bytes_b),
                    }),
                )],
            ),
        ]);
        let prepared = full(&MockFetch::default(), &thread);

        // Same sender name, different bytes: deterministic collision suffix.
        assert_eq!(prepared.imports.len(), 2);
        let first = format!("img/mail/{THREAD_HEX}/photo.png");
        let second = format!("img/mail/{THREAD_HEX}/photo_2.png");
        assert_eq!(prepared.imports[0].relative_path, first);
        assert_eq!(prepared.imports[0].bytes, bytes_a);
        assert_eq!(prepared.imports[1].relative_path, second);
        assert_eq!(prepared.imports[1].bytes, bytes_b);
        assert!(
            prepared.rendered[MSG1]
                .markdown
                .ends_with(&format!("![[{first}|400]]")),
            "{}",
            prepared.rendered[MSG1].markdown
        );
        assert!(
            prepared.rendered[MSG2]
                .markdown
                .ends_with(&format!("![[{second}|400]]")),
            "{}",
            prepared.rendered[MSG2].markdown
        );
        assert_eq!(prepared.origins.len(), 2);
        assert_eq!(prepared.origins[0].saved, first);
        assert_eq!(prepared.origins[1].saved, second);
        assert!(prepared.skipped.is_empty());

        // Identical bytes under different names share one destination.
        // (Sized above the logo threshold so the cross-message repeat is
        // kept and deduplicated rather than suppressed.)
        let shared = vec![0x41; 5000];
        let thread = make_thread(vec![
            inline_image_message(
                MSG1,
                html_a,
                vec![image_part(
                    "<a>",
                    "image/png",
                    Some("photo.png"),
                    serde_json::json!({
                        "size": shared.len(),
                        "data": encode(&shared),
                    }),
                )],
            ),
            inline_image_message(
                MSG2,
                html_b,
                vec![image_part(
                    "<b>",
                    "image/png",
                    Some("other.png"),
                    serde_json::json!({
                        "size": shared.len(),
                        "data": encode(&shared),
                    }),
                )],
            ),
        ]);
        let prepared = full(&MockFetch::default(), &thread);
        assert_eq!(prepared.imports.len(), 1);
        assert_eq!(prepared.imports[0].bytes, shared);
        assert_eq!(prepared.origins.len(), 2);
        assert_eq!(prepared.origins[0].saved, prepared.origins[1].saved);
        let shared = format!("![[{}|400]]", prepared.origins[0].saved);
        assert!(prepared.rendered[MSG1].markdown.ends_with(&shared));
        assert!(prepared.rendered[MSG2].markdown.ends_with(&shared));
    }

    #[test]
    fn small_repeats_across_messages_are_logos_but_not_within_one() {
        let logo = vec![0x4c; 100];
        let html = r#"<p>Hi <img src="cid:logo" alt="L"></p>"#;
        let logo_message = |id: &str| {
            inline_image_message(
                id,
                html,
                vec![image_part(
                    "<logo>",
                    "image/png",
                    Some("logo.png"),
                    serde_json::json!({
                        "size": logo.len(),
                        "data": encode(&logo),
                    }),
                )],
            )
        };
        let thread = make_thread(vec![logo_message(MSG1), logo_message(MSG2)]);
        let prepared = full(&MockFetch::default(), &thread);
        assert!(prepared.imports.is_empty());
        assert!(prepared.origins.is_empty());
        assert_eq!(
            prepared.skipped,
            vec![
                SkippedImage {
                    message_id: MSG1.to_string(),
                    reason: ImageSkipReason::SignatureLogo,
                },
                SkippedImage {
                    message_id: MSG2.to_string(),
                    reason: ImageSkipReason::SignatureLogo,
                },
            ]
        );
        assert!(!prepared.rendered[MSG1].markdown.contains("![["));

        // Two references in one message are deliberate placement, not a logo.
        let html_twice = r#"<p>Hi <img src="cid:logo" alt="L"> and <img src="cid:logo" alt="L"></p>"#;
        let thread = make_thread(vec![inline_image_message(
            MSG1,
            html_twice,
            vec![image_part(
                "<logo>",
                "image/png",
                Some("logo.png"),
                serde_json::json!({
                    "size": logo.len(),
                    "data": encode(&logo),
                }),
            )],
        )]);
        let prepared = full(&MockFetch::default(), &thread);
        assert_eq!(prepared.imports.len(), 1);
        assert_eq!(prepared.origins.len(), 2);
        assert!(prepared.skipped.is_empty());
        let embed = format!("![[img/mail/{THREAD_HEX}/logo.png|400]]");
        assert_eq!(
            prepared.rendered[MSG1].markdown.matches(&embed).count(),
            2,
            "{}",
            prepared.rendered[MSG1].markdown
        );
    }

    #[test]
    fn image_size_boundaries_hold_at_4kib_and_2mib() {
        // Exactly 4 KiB repeated across messages is kept: suppression needs
        // strictly smaller bytes.
        let big = vec![0x41; 4096];
        let html = r#"<p>Hi <img src="cid:big" alt="B"></p>"#;
        let big_message = |id: &str| {
            inline_image_message(
                id,
                html,
                vec![image_part(
                    "<big>",
                    "image/png",
                    Some("big.png"),
                    serde_json::json!({
                        "size": big.len(),
                        "data": encode(&big),
                    }),
                )],
            )
        };
        let thread = make_thread(vec![big_message(MSG1), big_message(MSG2)]);
        let prepared = full(&MockFetch::default(), &thread);
        assert_eq!(prepared.imports.len(), 1);
        assert!(prepared.skipped.is_empty());

        // Exactly 2 MiB is kept; one byte more is skipped.
        let max = vec![0x42; MAX_IMAGE_BYTES as usize];
        let html_max = r#"<p>Hi <img src="cid:max" alt="M"></p>"#;
        let thread = make_thread(vec![inline_image_message(
            MSG1,
            html_max,
            vec![image_part(
                "<max>",
                "image/png",
                Some("max.png"),
                serde_json::json!({
                    "size": max.len(),
                    "data": encode(&max),
                }),
            )],
        )]);
        let prepared = full(&MockFetch::default(), &thread);
        assert_eq!(prepared.imports.len(), 1);
        assert_eq!(prepared.imports[0].bytes.len(), MAX_IMAGE_BYTES as usize);

        let over = vec![0x43; MAX_IMAGE_BYTES as usize + 1];
        let thread = make_thread(vec![inline_image_message(
            MSG1,
            html_max,
            vec![image_part(
                "<max>",
                "image/png",
                Some("max.png"),
                serde_json::json!({
                    "size": over.len(),
                    "data": encode(&over),
                }),
            )],
        )]);
        let prepared = full(&MockFetch::default(), &thread);
        assert!(prepared.imports.is_empty());
        assert_eq!(
            prepared.skipped,
            vec![SkippedImage {
                message_id: MSG1.to_string(),
                reason: ImageSkipReason::Oversized,
            }]
        );

        // Huge metadata skips the download entirely.
        let small = vec![0x44; 16];
        let meta_html = r#"<p>Hi <img src="cid:meta" alt="M"></p>"#;
        let thread = make_thread(vec![message(
            MSG1,
            &["INBOX"],
            vec![
                html_part(
                    "body-1",
                    serde_json::json!({
                        "size": meta_html.len(),
                        "data": encode(meta_html.as_bytes()),
                    }),
                ),
                image_part(
                    "<meta>",
                    "image/png",
                    Some("meta.png"),
                    serde_json::json!({
                        "size": 5 * 1024 * 1024,
                        "attachmentId": "meta-1",
                    }),
                ),
            ],
            vec![("From", "Ada <ada@example.com>")],
        )]);
        let reader =
            MockFetch::default().with_body(MSG1, "meta-1", small.clone());
        let prepared = full(&reader, &thread);
        assert!(reader.calls().is_empty());
        assert!(prepared.imports.is_empty());
        assert_eq!(
            prepared.skipped,
            vec![SkippedImage {
                message_id: MSG1.to_string(),
                reason: ImageSkipReason::Oversized,
            }]
        );
    }

    #[test]
    fn hostile_names_become_safe_slugs_under_the_thread_directory() {
        let bytes = vec![0x45; 64];
        let html_x = r#"<p>Hi <img src="cid:x" alt="X"></p>"#;
        let html_y = r#"<p>Hi <img src="cid:y" alt="Y"></p>"#;
        let thread = make_thread(vec![
            inline_image_message(
                MSG1,
                html_x,
                vec![image_part(
                    "<x>",
                    "image/png",
                    Some("../../etc/passwd"),
                    serde_json::json!({
                        "size": bytes.len(),
                        "data": encode(&bytes),
                    }),
                )],
            ),
            inline_image_message(
                MSG2,
                html_y,
                vec![image_part(
                    "<y>",
                    "image/jpeg",
                    Some("a/b\\c:d?.EXE"),
                    serde_json::json!({
                        "size": bytes.len() + 1,
                        "data": encode(&[bytes.clone(), vec![0]].concat()),
                    }),
                )],
            ),
        ]);
        let other = [bytes.clone(), vec![0]].concat();
        let prepared = full(&MockFetch::default(), &thread);
        assert_eq!(prepared.imports.len(), 2);
        let first = format!("img/mail/{THREAD_HEX}/passwd.png");
        // Basename `b\c:d?` sanitizes to `b_c_d`, extension comes from MIME.
        let second = format!("img/mail/{THREAD_HEX}/b_c_d.jpg");
        assert_eq!(prepared.imports[0].relative_path, first);
        assert_eq!(prepared.imports[1].relative_path, second);
        assert_eq!(prepared.imports[1].bytes, other);
        for import in &prepared.imports {
            assert!(
                import
                    .relative_path
                    .starts_with(&format!("img/mail/{THREAD_HEX}/")),
                "{}",
                import.relative_path
            );
            assert!(!import.relative_path.contains(".."));
            assert!(
                verified_local_image_wikilink(&import.relative_path).is_some()
            );
        }

        // No filename falls back to the content ID; JPEG maps to `jpg`.
        let html_id = r#"<p>Hi <img src="cid:ii_1234" alt="I"></p>"#;
        let thread = make_thread(vec![inline_image_message(
            MSG1,
            html_id,
            vec![image_part(
                "<ii_1234>",
                "image/jpeg",
                None,
                serde_json::json!({
                    "size": bytes.len(),
                    "data": encode(&bytes),
                }),
            )],
        )]);
        let prepared = full(&MockFetch::default(), &thread);
        assert_eq!(
            prepared.imports[0].relative_path,
            format!("img/mail/{THREAD_HEX}/ii_1234.jpg")
        );
    }

    #[test]
    fn unsupported_types_and_unresolved_references_are_skipped() {
        let bytes = vec![0x46; 64];
        let html = r#"<p>Hi <img src="cid:v" alt="V"></p>"#;
        let thread = make_thread(vec![message(
            MSG1,
            &["INBOX"],
            vec![
                html_part(
                    "body-1",
                    serde_json::json!({"size": html.len(), "data": encode(html.as_bytes())}),
                ),
                image_part(
                    "<v>",
                    "image/svg+xml",
                    Some("vector.svg"),
                    serde_json::json!({
                        "size": bytes.len(),
                        "attachmentId": "vec-1",
                    }),
                ),
            ],
            vec![("From", "Ada <ada@example.com>")],
        )]);
        let reader = MockFetch::default().with_body(MSG1, "vec-1", bytes);
        let prepared = full(&reader, &thread);
        // SVG is never downloaded and never embedded.
        assert!(reader.calls().is_empty());
        assert!(prepared.imports.is_empty());
        assert_eq!(
            prepared.skipped,
            vec![SkippedImage {
                message_id: MSG1.to_string(),
                reason: ImageSkipReason::UnsupportedType,
            }]
        );

        // A CID with neither bytes nor a provider ID is an unresolvable stub.
        let ghost_html = r#"<p>Hi <img src="cid:ghost" alt="G"></p>"#;
        let thread = make_thread(vec![message(
            MSG1,
            &["INBOX"],
            vec![
                html_part(
                    "body-1",
                    serde_json::json!({"size": ghost_html.len(), "data": encode(ghost_html.as_bytes())}),
                ),
                image_part(
                    "<ghost>",
                    "image/png",
                    Some("ghost.png"),
                    serde_json::json!({"size": 10}),
                ),
            ],
            vec![("From", "Ada <ada@example.com>")],
        )]);
        let prepared = full(&MockFetch::default(), &thread);
        assert!(prepared.imports.is_empty());
        assert_eq!(
            prepared.skipped,
            vec![SkippedImage {
                message_id: MSG1.to_string(),
                reason: ImageSkipReason::Unavailable,
            }]
        );
    }

    #[test]
    fn embeds_follow_stable_order_with_identity_only_origins() {
        let a = vec![0x41; 64];
        let b = vec![0x42; 64];
        let html = r#"<p><img src="cid:b" alt="B"> then <img src="cid:a" alt="A"></p>"#;
        let thread = make_thread(vec![inline_image_message(
            MSG1,
            html,
            vec![
                image_part(
                    "<a>",
                    "image/png",
                    Some("a.png"),
                    serde_json::json!({"size": a.len(), "data": encode(&a)}),
                ),
                image_part(
                    "<b>",
                    "image/gif",
                    Some("b.gif"),
                    serde_json::json!({"size": b.len(), "data": encode(&b)}),
                ),
            ],
        )]);
        let prepared = full(&MockFetch::default(), &thread);
        // Descriptor order follows the HTML reference order (b, then a).
        let embed_b = format!("img/mail/{THREAD_HEX}/b.gif");
        let embed_a = format!("img/mail/{THREAD_HEX}/a.png");
        let markdown = prepared.rendered[MSG1].markdown.clone();
        let position_b = markdown.find(&embed_b).expect("b embed");
        let position_a = markdown.find(&embed_a).expect("a embed");
        assert!(position_b < position_a, "{markdown}");
        assert_eq!(prepared.origins.len(), 2);
        // Bytes arrive inline, so locators use the normalized content IDs.
        assert_eq!(
            prepared.origins[0].source,
            format!("gmail:{ACCOUNT}/{MSG1}/b")
        );
        assert_eq!(
            prepared.origins[1].source,
            format!("gmail:{ACCOUNT}/{MSG1}/a")
        );
        for origin in &prepared.origins {
            assert!(!origin.source.contains("http"));
            assert!(!origin.source.contains("token"));
        }
    }

    #[test]
    fn sender_text_cannot_forge_embeds() {
        let forged = format!("![[img/mail/{THREAD_HEX}/evil.png|400]]");
        let html = format!(
            "<p>Totally real {forged} <img src=\"cid:logo\" alt=\"L\"></p>"
        );
        let image = vec![0x47; 64];
        let thread = make_thread(vec![inline_image_message(
            MSG1,
            &html,
            vec![image_part(
                "<logo>",
                "image/png",
                Some("logo.png"),
                serde_json::json!({
                    "size": image.len(),
                    "data": encode(&image),
                }),
            )],
        )]);
        let prepared = full(&MockFetch::default(), &thread);
        let markdown = prepared.rendered[MSG1].markdown.clone();
        // The only intact wikilink embed is the planned one; the sender's
        // literal text was neutralized by inert serialization.
        let expected = format!("![[img/mail/{THREAD_HEX}/logo.png|400]]");
        assert_eq!(markdown.matches("![[").count(), 1, "{markdown}");
        assert!(markdown.ends_with(&expected), "{markdown}");
    }

    #[test]
    fn non_hex_thread_ids_refuse_without_echo() {
        let thread = make_thread_with_id(
            "not-hex!!",
            vec![message(
                MSG1,
                &["INBOX"],
                vec![plain_part(
                    "a",
                    serde_json::json!({"size": 2, "data": encode(b"hi")}),
                )],
                vec![("From", "Ada <ada@example.com>")],
            )],
        );
        let error = prepare_thread(
            &MockFetch::default(),
            ACCOUNT,
            &thread,
            GmailBodyMode::Full,
        )
        .err()
        .expect("bad id must fail");
        assert_eq!(error.kind(), "unresolvable");
        assert!(!error.message().contains("not-hex"));
    }
}
