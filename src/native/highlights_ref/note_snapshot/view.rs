//! Structured snapshot view and lossless source records.

use std::{collections::BTreeMap, path::PathBuf, sync::Arc};

use serde_yaml::Value as YamlValue;
use sha2::{Digest, Sha256};

/// Stable identity for one note in the vault. Routes are display/resolution names;
/// the canonical path is the identity used to distinguish notes with equal titles.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct SourceIdentity {
    pub canonical_path: PathBuf,
    pub vault_relative_path: String,
    pub route: String,
}

/// A source range with byte offsets and one-based, inclusive line numbers.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct SourceExtent {
    pub byte_start: usize,
    pub byte_end: usize,
    pub first_line: usize,
    pub last_line: usize,
}

/// Parsed frontmatter remains available to the later Bob layer without reparsing the
/// serializer's output or discarding fields the current projection does not use.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct SourceFrontmatter {
    pub extent: SourceExtent,
    pub raw_yaml: String,
    pub metadata: YamlValue,
}

/// Indentation is retained both literally and as a parser-computed display column.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SourceIndentation {
    pub prefix: String,
    pub columns: usize,
}

/// Raw task data for the Bob layer. `body` is copied byte-for-byte from after the
/// checkbox marker through the end of its source line, including fields, tags, and a
/// trailing block ID. `raw_line` also retains the list marker and indentation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ParsedTaskLine {
    pub raw_line: String,
    pub body: String,
    pub extent: SourceExtent,
    pub indentation: SourceIndentation,
    pub block_id: Option<String>,
}

/// Immutable source bytes plus parsed metadata. The digest is computed from the same
/// byte slice retained here, so readers can share one consistent source snapshot.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ParsedSource {
    pub identity: SourceIdentity,
    pub raw_bytes: Arc<[u8]>,
    pub sha256: String,
    pub frontmatter: Option<SourceFrontmatter>,
    pub task_lines: Vec<ParsedTaskLine>,
}

impl ParsedSource {
    pub(crate) fn from_bytes(
        identity: SourceIdentity,
        raw_bytes: impl Into<Arc<[u8]>>,
        frontmatter: Option<SourceFrontmatter>,
        task_lines: Vec<ParsedTaskLine>,
    ) -> Self {
        let raw_bytes = raw_bytes.into();
        let sha256 = hex::encode(Sha256::digest(&raw_bytes));
        Self {
            identity,
            raw_bytes,
            sha256,
            frontmatter,
            task_lines,
        }
    }
}

/// The identity of a source-local heading or block anchor.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) enum AnchorKind {
    Heading,
    Block,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct AnchorIdentity {
    pub source: SourceIdentity,
    pub kind: AnchorKind,
    pub fragment: String,
}

/// A final, emitted anchor. `SnapshotView::anchors` is the authoritative visible set
/// consulted by internal-link serialization after the Bob layer filters the view.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SnapshotAnchor {
    pub identity: AnchorIdentity,
    pub id: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum HeadingLevel {
    H1,
    H2,
    H3,
}

impl HeadingLevel {
    pub(crate) const fn marker(self) -> &'static str {
        match self {
            Self::H1 => "#",
            Self::H2 => "##",
            Self::H3 => "###",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum HeadingClass {
    Normal,
    Lane,
    Appendix,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SnapshotHeading {
    pub level: HeadingLevel,
    pub content: Vec<Inline>,
    pub class: HeadingClass,
    pub anchor: SnapshotAnchor,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) enum SnapshotTaskStatus {
    Open,
    Next,
    Wip,
    Blocked,
    Done,
    Canceled,
}

impl SnapshotTaskStatus {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Next => "next",
            Self::Wip => "wip",
            Self::Blocked => "blocked",
            Self::Done => "done",
            Self::Canceled => "canceled",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct StatusCount {
    pub status: SnapshotTaskStatus,
    pub count: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum LinkTarget {
    /// A normal URL, including a mailto or other external URI.
    Url(String),
    /// A note fragment represented structurally until the visible-anchor set is final.
    Internal(AnchorIdentity),
    /// An internal link inside one emitted transclusion occurrence. The source identity
    /// remains stable while the presentation ID distinguishes repeated inclusions.
    ScopedInternal {
        identity: AnchorIdentity,
        presentation_id: String,
    },
    /// An Obsidian URI used when a target is outside the projected slice.
    ObsidianUri(String),
}

/// Inline content stays structured until the final CommonMark serialization pass.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum Inline {
    Text(String),
    Link {
        label: Vec<Inline>,
        target: LinkTarget,
    },
    Emphasis(Vec<Inline>),
    Strong(Vec<Inline>),
    Strike(Vec<Inline>),
    Code(String),
    Chip(Chip),
    Muted(Vec<Inline>),
    Break,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ChipKind {
    Normal,
    Accent,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Chip {
    pub content: Vec<Inline>,
    pub kind: ChipKind,
    pub target: Option<LinkTarget>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ListKind {
    Unordered,
    Ordered { start: u64 },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ListItem {
    pub content: Vec<Inline>,
    pub children: Vec<Block>,
}

/// Body blocks supported by the shared projection contract.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum Block {
    Paragraph(Vec<Inline>),
    Heading(SnapshotHeading),
    List {
        kind: ListKind,
        items: Vec<ListItem>,
    },
    Code {
        language: Option<String>,
        text: String,
    },
    Task(SnapshotTask),
    LabeledRow {
        label: Vec<Inline>,
        items: Vec<Vec<Inline>>,
    },
    LogRow {
        label: Vec<Inline>,
        date: Option<Vec<Inline>>,
        content: Vec<Inline>,
        older_count: Option<u64>,
    },
    Image {
        asset_id: String,
        alt: String,
        width_percent: Option<u8>,
    },
    MissingImage {
        label: Vec<Inline>,
    },
    Embed {
        source: String,
        blocks: Vec<Block>,
    },
    LiveQuery {
        blocks: Vec<Block>,
    },
    Callout {
        kind: String,
        title: Vec<Inline>,
        blocks: Vec<Block>,
    },
    Compact {
        blocks: Vec<Block>,
    },
    EmptyState(Vec<Inline>),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SnapshotTask {
    pub status: SnapshotTaskStatus,
    pub content: Vec<Inline>,
    pub chips: Vec<Chip>,
    pub anchor: Option<SnapshotAnchor>,
    pub children: Vec<Block>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum AssetMediaKind {
    Png,
    Jpeg,
    Gif,
    Webp,
    Svg,
    Other,
}

/// Resolved asset provenance retained for a later render-scratch copy.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SnapshotAsset {
    pub id: String,
    pub canonical_source_path: PathBuf,
    pub relative_path: PathBuf,
    pub media_kind: AssetMediaKind,
    pub width_px_hint: Option<u32>,
    pub width_percent: Option<u8>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct SnapshotLinkCounts {
    pub resolved: u64,
    pub unresolved: u64,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct SnapshotSummary {
    pub sections: u64,
    pub task_counts: BTreeMap<SnapshotTaskStatus, u64>,
    pub links: SnapshotLinkCounts,
    pub assets: u64,
    pub missing_images: u64,
    pub transclusions: u64,
    pub guarded_omissions: u64,
    /// Bob-layer counters remain available without defining their filtering policy here.
    pub omitted_closed_tasks: u64,
    pub omitted_self_snapshot_tasks: u64,
    pub omitted_sections: u64,
    pub schedule_logs: u64,
    pub work_logs: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SnapshotWarningKind {
    ReadFailure,
    UnresolvedLink,
    AmbiguousLink,
    MissingFragment,
    AmbiguousFragment,
    MissingImage,
    AmbiguousImage,
    UnknownTaskStatus,
    NestedTransclusion,
    CyclicTransclusion,
    OversizedTransclusion,
    InvalidSource,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SourceLocation {
    pub source: SourceIdentity,
    pub line: Option<usize>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SnapshotWarning {
    pub kind: SnapshotWarningKind,
    pub origin: Option<SourceLocation>,
    pub message: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SnapshotMasthead {
    /// PDF identity title. It is plain text, not Markdown.
    pub title: String,
    pub kicker: Vec<Inline>,
    pub display_title: Vec<Inline>,
    pub dek: Option<Vec<Inline>>,
    pub meta: Vec<Vec<Inline>>,
    pub stats: Vec<StatusCount>,
    pub omitted: Option<Vec<Inline>>,
    pub snapshot_line: Vec<Inline>,
    pub footer: Vec<Inline>,
    pub glyph_table: String,
    pub return_links: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SnapshotSection {
    pub source: SourceIdentity,
    pub heading: SnapshotHeading,
    pub blocks: Vec<Block>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SnapshotView {
    pub source: SourceIdentity,
    pub masthead: SnapshotMasthead,
    pub sections: Vec<SnapshotSection>,
    pub closed: Option<SnapshotSection>,
    pub assets: Vec<SnapshotAsset>,
    pub summary: SnapshotSummary,
    pub warnings: Vec<SnapshotWarning>,
    /// Final anchors visible in the serialized document. Bob-layer filtering updates
    /// this set before serialization; absent internal targets become plain labels.
    pub anchors: Vec<SnapshotAnchor>,
}
