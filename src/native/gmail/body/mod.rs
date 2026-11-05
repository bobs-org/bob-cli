//! Private MIME candidate collection and the stable body result boundary.

pub(super) mod mime;

use std::collections::BTreeMap;

// Consumer: the later body-integration phase uses this public-to-crate boundary.
#[allow(unused_imports)]
pub(crate) use mime::{collect_message_bodies, MimeInventory};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum BodySource {
    Html,
    Plain,
    Snippet,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum CandidateKind {
    Html,
    Plain,
}

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct BodyCandidate {
    pub(crate) kind: CandidateKind,
    pub(crate) part_id: Option<String>,
    pub(crate) mime_type: String,
    pub(crate) text: String,
}

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct FetchNeed {
    pub(crate) part_id: Option<String>,
    pub(crate) attachment_id: Option<String>,
}

#[derive(Clone, PartialEq, Eq)]
pub(crate) enum InlineImageSource {
    Bytes(Vec<u8>),
    AttachmentId(String),
}

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct InlineImage {
    pub(crate) content_id: Option<String>,
    pub(crate) mime_type: String,
    pub(crate) size: Option<u64>,
    pub(crate) filename: Option<String>,
    pub(crate) source: InlineImageSource,
}

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct Attachment {
    pub(crate) filename: Option<String>,
    pub(crate) mime_type: String,
    pub(crate) size: Option<u64>,
    pub(crate) attachment_id: Option<String>,
}

/// Bytes already fetched by a transport. Keys remain opaque and are never rendered.
#[derive(Default, Clone, PartialEq, Eq)]
pub(crate) struct FetchedBodies {
    bodies: BTreeMap<(String, String), Vec<u8>>,
}

impl FetchedBodies {
    pub(crate) fn insert(
        &mut self,
        message_id: impl Into<String>,
        attachment_id: impl Into<String>,
        bytes: Vec<u8>,
    ) -> Option<Vec<u8>> {
        self.bodies
            .insert((message_id.into(), attachment_id.into()), bytes)
    }

    pub(crate) fn get(
        &self,
        message_id: &str,
        attachment_id: &str,
    ) -> Option<&[u8]> {
        self.bodies
            .get(&(message_id.to_string(), attachment_id.to_string()))
            .map(Vec::as_slice)
    }
}

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct RenderedBody {
    /// Inert main prose; renderer-owned card wrappers are added by the caller.
    pub(crate) markdown: String,
    pub(crate) quoted: Option<String>,
    pub(crate) signature: Option<String>,
    pub(crate) source: BodySource,
    pub(crate) bulletin: bool,
    pub(crate) inline_images: Vec<InlineImage>,
    pub(crate) attachments: Vec<Attachment>,
    pub(crate) needs_fetch: Vec<FetchNeed>,
}
