//! Engine-neutral note snapshot model and projected-Markdown contract.
//!
//! Parsing keeps source metadata separate from the view. Resolution and syntax
//! projection feed the one-level transclusion assembler; the Bob layer can then
//! finalize `anchors`, and this module's serializer is the only producer of projected
//! Markdown.

pub(crate) mod fonts;
pub(crate) mod profile;

mod bob;
mod classify;
mod obsidian;
mod resolve;
mod serialize;
mod transclusion;
mod view;

#[cfg(test)]
pub(crate) mod tests;

// These crate-visible exports are the integration seam for later snapshot phases;
// their production callers are intentionally added by the create-wiring phase.
#[allow(unused_imports)]
pub(crate) use bob::{apply_bob_task_rows, BobTaskContext};
#[cfg(test)]
#[allow(unused_imports)]
pub(crate) use classify::CountingLoader;
#[allow(unused_imports)]
pub(crate) use classify::{
    classify_note_kind, classify_target, default_parent, sha256_hex,
    source_identity, vault_member_relative, ClassifiedNote, ClassifyError,
    DefaultParentDeferred, FilesystemSourceLoader, NoteKind, SnapshotTarget,
    SourceLoader, SourceStore,
};
#[allow(unused_imports)]
pub(crate) use obsidian::{
    project_obsidian_syntax, status_for_symbol, PendingEmbed, SyntaxProjection,
    TitleCandidate,
};
#[allow(unused_imports)]
pub(crate) use resolve::{
    build_fragment_index, classify_error_from_resolve, normalize_heading_key,
    obsidian_uri, obsidian_uri_with_fragment, parse_attachment_target,
    parse_link_target, percent_encode, render_uri_template,
    resolve_uri_template, width_hint_to_percent, AttachmentIndex,
    AttachmentResolutionError, FragmentIndex, FragmentKind, FragmentLookup,
    FragmentSpan, LinkFragment, NoteResolveError, ParsedAttachmentTarget,
    ParsedLinkTarget, ResolvedAttachment, ResolvedNote, VaultNotes,
    DEFAULT_OBSIDIAN_URI_TEMPLATE, SNAPSHOT_TEXT_WIDTH_PX,
};
#[allow(unused_imports)]
pub(crate) use serialize::serialize_projected_markdown;
#[allow(unused_imports)]
pub(crate) use transclusion::{
    project_snapshot, SnapshotProjection, SnapshotProjectionOptions,
};
#[allow(unused_imports)]
pub(crate) use view::{
    AnchorIdentity, AnchorKind, AssetMediaKind, Block, Chip, ChipKind,
    HeadingClass, HeadingLevel, Inline, LinkTarget, ListItem, ListKind,
    ParsedSource, ParsedTaskLine, SnapshotAnchor, SnapshotAsset,
    SnapshotHeading, SnapshotLinkCounts, SnapshotMasthead, SnapshotSection,
    SnapshotSummary, SnapshotTask, SnapshotTaskStatus, SnapshotView,
    SnapshotWarning, SnapshotWarningKind, SourceExtent, SourceFrontmatter,
    SourceIdentity, SourceIndentation, SourceLocation, StatusCount, TaskSource,
};
