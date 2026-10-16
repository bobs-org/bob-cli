//! Engine-neutral note snapshot model and projected-Markdown contract.
//!
//! Parsing keeps source metadata separate from the view. The later resolution and
//! syntax phases populate the model, the Bob layer finalizes `anchors`, and this
//! module's serializer is the only producer of projected Markdown.

pub(crate) mod fonts;

mod serialize;
mod view;

#[cfg(test)]
mod tests;

// These crate-visible exports are the integration seam for later snapshot phases;
// their production callers are intentionally added by the create-wiring phase.
#[allow(unused_imports)]
pub(crate) use serialize::serialize_projected_markdown;
#[allow(unused_imports)]
pub(crate) use view::{
    AnchorIdentity, AssetMediaKind, Block, Chip, ChipKind, HeadingClass,
    HeadingLevel, Inline, LinkTarget, ListItem, ListKind, ParsedSource,
    ParsedTaskLine, SnapshotAnchor, SnapshotAsset, SnapshotHeading,
    SnapshotLinkCounts, SnapshotMasthead, SnapshotSection, SnapshotSummary,
    SnapshotTask, SnapshotTaskStatus, SnapshotView, SnapshotWarning,
    SnapshotWarningKind, SourceExtent, SourceFrontmatter, SourceIdentity,
    SourceIndentation, SourceLocation, StatusCount,
};
